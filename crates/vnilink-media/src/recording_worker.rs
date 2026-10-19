//! Bounded local recording worker. Storage never executes on a live-media task.
use crate::{RecordingError, RecordingFailure, RecordingLimits, RecordingRetention, SegmentWriter};
use std::{
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecordingPhase {
    Starting,
    Recording,
    Finished,
    Failed,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordingStatus {
    pub phase: RecordingPhase,
    pub failure: Option<RecordingFailure>,
    pub frames: u64,
    pub bytes: u64,
    pub duration_ms: u64,
    /// Finished segments. Frames, bytes and duration are cumulative across
    /// finished segments plus the live one.
    pub segments: u32,
}
/// A local recording of a paired computer's desktop or camera, written by its own worker.
pub struct Recorder {
    stop: Arc<AtomicBool>,
    status: Arc<Mutex<RecordingStatus>>,
}
impl Recorder {
    /// The caller locally selects a new .mkv destination. This engine never
    /// reads a remote path or uses the Files grant/root as a recording permission.
    /// Recording consent is checked independently from the desktop viewing grant.
    pub fn start(
        desktop: &vnilink::RemoteDesktopService,
        path: &Path,
        limits: RecordingLimits,
    ) -> Result<Self, RecordingError> {
        Self::start_with_retention(desktop, path, limits, None)
    }
    /// Segmented recording with retention: byte, duration and format limits
    /// roll to `stem.000001.mkv`, `stem.000002.mkv`, ... while retention caps
    /// the finished totals, oldest first.
    pub fn start_with_retention(
        desktop: &vnilink::RemoteDesktopService,
        path: &Path,
        limits: RecordingLimits,
        retention: Option<RecordingRetention>,
    ) -> Result<Self, RecordingError> {
        let segments = SegmentWriter::create(path, limits, retention)?;
        if !desktop
            .recording_allowed()
            .map_err(RecordingError::Source)?
        {
            return Err(RecordingError::Source(vnilink::Error::Unauthorized));
        }
        let tap = desktop
            .recording_video_tap()
            .map_err(RecordingError::Source)?;
        Self::record(tap, segments, limits)
    }
    /// Records a paired computer's camera, with the same limits and
    /// retention. The host's recording consent is checked as for a desktop,
    /// and its withdrawal ends the recording.
    pub fn start_camera(
        camera: &vnilink::RemoteCameraService,
        path: &Path,
        limits: RecordingLimits,
        retention: Option<RecordingRetention>,
    ) -> Result<Self, RecordingError> {
        let segments = SegmentWriter::create(path, limits, retention)?;
        if !camera.recording_allowed() {
            return Err(RecordingError::Source(vnilink::Error::Unauthorized));
        }
        let tap = camera
            .recording_video_tap()
            .map_err(RecordingError::Source)?;
        Self::record(tap, segments, limits)
    }
    fn record(
        tap: vnilink::RecordingVideoTap,
        mut segments: SegmentWriter,
        limits: RecordingLimits,
    ) -> Result<Self, RecordingError> {
        let stop = Arc::new(AtomicBool::new(false));
        let status = Arc::new(Mutex::new(RecordingStatus {
            phase: RecordingPhase::Starting,
            failure: None,
            frames: 0,
            bytes: 0,
            duration_ms: 0,
            segments: 0,
        }));
        let worker_stop = stop.clone();
        let worker_status = status.clone();
        let spawn = std::thread::Builder::new()
            .name("vnilink-recording".into())
            .spawn(move || {
                let mut run = || -> Result<crate::RecordingSnapshot, RecordingFailure> {
                    let began = Instant::now();
                    let mut segment_began = Instant::now();
                    loop {
                        if tap.has_failed() {
                            return Err(RecordingFailure::SlowStorage);
                        }
                        if worker_stop.load(Ordering::Acquire) {
                            break;
                        }
                        if let Some(frame) = tap.try_recv() {
                            segments.push(&frame)?;
                            publish_snapshot(
                                &worker_status,
                                RecordingPhase::Recording,
                                segments.snapshot(),
                            );
                        } else if tap.is_closed() {
                            break;
                        } else {
                            let snapshot = segments.snapshot();
                            if !segments.primed()
                                && snapshot.segments == 0
                                && began.elapsed() > Duration::from_secs(5)
                            {
                                return Err(RecordingFailure::Startup);
                            }
                            std::thread::sleep(Duration::from_millis(2));
                        }
                        if segment_began.elapsed().as_millis() >= u128::from(limits.max_duration_ms)
                        {
                            if segments.primed() {
                                segments.roll_wall()?;
                                publish_snapshot(
                                    &worker_status,
                                    RecordingPhase::Recording,
                                    segments.snapshot(),
                                );
                            }
                            segment_began = Instant::now();
                        }
                    }
                    segments.finalize()
                };
                let result = run();
                let mut state = worker_status.lock().unwrap_or_else(|e| e.into_inner());
                match result {
                    Ok(snapshot) => *state = status_snapshot(RecordingPhase::Finished, snapshot),
                    Err(error) => {
                        *state = status_snapshot(RecordingPhase::Failed, segments.snapshot());
                        state.failure = Some(error);
                    }
                }
            });
        if spawn.is_err() {
            return Err(RecordingError::Storage(std::io::Error::other(
                "recording worker",
            )));
        }
        Ok(Self { stop, status })
    }
    /// Requests finalization without blocking on storage. Poll status for the
    /// terminal result; closing a recorder never closes the service it records.
    pub fn stop(&self) {
        self.stop.store(true, Ordering::Release);
    }
    pub fn status(&self) -> RecordingStatus {
        self.status
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }
}
impl Drop for Recorder {
    fn drop(&mut self) {
        self.stop();
    }
}
fn paths(path: &Path) -> Result<(PathBuf, PathBuf), RecordingError> {
    if !path.is_absolute()
        || path.extension().is_none_or(|e| e != "mkv")
        || path.file_name().is_none()
    {
        return Err(RecordingError::InvalidMedia);
    }
    let parent = path
        .parent()
        .ok_or(RecordingError::InvalidMedia)?
        .canonicalize()?;
    let destination = parent.join(path.file_name().unwrap());
    if destination.try_exists()? {
        return Err(RecordingError::Storage(std::io::Error::from(
            std::io::ErrorKind::AlreadyExists,
        )));
    }
    let mut name = path.file_name().unwrap().to_os_string();
    name.push(".part");
    Ok((destination, parent.join(name)))
}

/// Explicit recovery of the matching `.mkv.part`, never automatic directory
/// sweeping. Trim only an incomplete tail, repair duration, fsync and publish
/// without replacing an existing destination. Recovered output has no cue index.
/// Pass one segment path per interrupted segment.
pub fn recover_desktop_recording(path: &Path) -> Result<crate::RecordingRecovery, RecordingError> {
    use std::io::{Seek, SeekFrom, Write};
    let (destination, partial) = paths(path)?;
    let mut file = vnilink_rename::open_single_link_file_rw(&partial)?;
    let recovered = crate::scan_recording_prefix(&mut file)?;
    file.set_len(recovered.complete_bytes)?;
    file.seek(SeekFrom::Start(recovered.duration_position))?;
    file.write_all(&(recovered.last_timestamp_ms as f64).to_be_bytes())?;
    file.sync_all()?;
    drop(file);
    vnilink_rename::rename_noreplace(&partial, &destination)?;
    #[cfg(unix)]
    std::fs::File::open(destination.parent().unwrap())?.sync_all()?;
    Ok(recovered)
}

fn status_snapshot(phase: RecordingPhase, snapshot: crate::RecordingSnapshot) -> RecordingStatus {
    RecordingStatus {
        phase,
        failure: None,
        frames: snapshot.frames,
        bytes: snapshot.bytes,
        duration_ms: snapshot.duration_ms,
        segments: snapshot.segments,
    }
}
fn publish_snapshot(
    status: &Mutex<RecordingStatus>,
    phase: RecordingPhase,
    snapshot: crate::RecordingSnapshot,
) {
    *status.lock().unwrap_or_else(|e| e.into_inner()) = status_snapshot(phase, snapshot);
}
