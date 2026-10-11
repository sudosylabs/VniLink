//! Segmented recording: bounded segments, rollover, retention. No capture,
//! decode, UI, or remote paths. Each finished segment is a standalone file.
//! Retention deletes only segments this recording finalized: a matching
//! filename the recording did not create (a prior recording, a user file)
//! is never swept, and a mid-recording index collision fails as Storage
//! instead of overwriting.
use crate::{h264_picture_size, parse_annex_b, MatroskaRecorder, RecordingError, RecordingLimits};
use std::{
    ffi::OsString,
    fs::File,
    path::{Path, PathBuf},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecordingFailure {
    Startup,
    Media,
    Storage,
    Limit,
    SlowStorage,
    FormatChanged,
}

/// Totals budget across finished segments. The limits on [`RecordingLimits`]
/// bound one segment; retention bounds the whole recording. The newest
/// finished segment is always kept, even when it alone exceeds the budget.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RecordingRetention {
    pub max_segments: u32,
    pub max_total_bytes: u64,
}

impl RecordingRetention {
    pub fn validate(self) -> Result<(), RecordingError> {
        if !(1..=65_535).contains(&self.max_segments)
            || !(1_048_576..=1_099_511_627_776).contains(&self.max_total_bytes)
        {
            return Err(RecordingError::Limit);
        }
        Ok(())
    }
}

/// Cumulative totals across finished segments plus the live one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RecordingSnapshot {
    pub frames: u64,
    pub bytes: u64,
    pub duration_ms: u64,
    pub segments: u32,
}

/// Frame-driven segmented writer. Byte, duration and format limits roll to a
/// new segment instead of failing the recording; only a corrupt stream, dead
/// storage, or a single frame larger than the segment budget is terminal.
pub struct SegmentWriter {
    dir: PathBuf,
    stem: OsString,
    limits: RecordingLimits,
    retention: Option<RecordingRetention>,
    index: u32,
    writer: Option<MatroskaRecorder<File>>,
    partial: Option<PathBuf>,
    epoch: u64,
    last: u64,
    // A consumed muxer can leave recoverable partial output on finalization
    // failure. Preserve its accepted frames and known bytes for status.
    unfinished: (u64, u64),
    finished_frames: u64,
    finished_bytes: u64,
    finished_duration: u64,
    segments: u32,
    /// Open identities and content metadata for finalized files. Retention
    /// isolates candidates before deleting unchanged matching objects.
    /// Replaced, changed or missing entries lose authority; retained handles
    /// are pruned with the live retention window.
    owned: std::collections::BTreeMap<u32, vnilink_rename::recording_retention::RecordedFile>,
}

impl SegmentWriter {
    /// Validates the destination stem, limits and retention without creating
    /// files. Segment files appear only once a priming IDR arrives, so a
    /// failed start leaves no orphan partials behind.
    pub fn create(
        path: &Path,
        limits: RecordingLimits,
        retention: Option<RecordingRetention>,
    ) -> Result<Self, RecordingError> {
        limits.validate()?;
        if let Some(retention) = retention {
            retention.validate()?;
        }
        if !path.is_absolute()
            || path.extension().is_none_or(|e| e != "mkv")
            || path.file_name().is_none()
        {
            return Err(RecordingError::InvalidMedia);
        }
        let dir = path
            .parent()
            .ok_or(RecordingError::InvalidMedia)?
            .canonicalize()?;
        let stem = path.file_stem().ok_or(RecordingError::InvalidMedia)?;
        let first = Self::segment_paths(&dir, stem, 1);
        if first.0.try_exists()? || first.1.try_exists()? {
            return Err(RecordingError::Storage(std::io::Error::from(
                std::io::ErrorKind::AlreadyExists,
            )));
        }
        Ok(Self {
            dir,
            stem: stem.to_os_string(),
            limits,
            retention,
            index: 1,
            writer: None,
            partial: None,
            epoch: 0,
            last: 0,
            unfinished: (0, 0),
            finished_frames: 0,
            finished_bytes: 0,
            finished_duration: 0,
            segments: 0,
            owned: std::collections::BTreeMap::new(),
        })
    }

    /// Pushes one frame, rolling to a new segment on byte, duration or format
    /// limits. Before the first prime the stream must open with a usable IDR;
    /// after a roll, clean mid-GOP frames are skipped until the next IDR
    /// while corrupt frames still fail the recording.
    pub fn push(&mut self, frame: &vnilink::EncodedFrame) -> Result<(), RecordingFailure> {
        if self.writer.is_none() {
            let strict = self.segments == 0 && self.finished_frames == 0;
            return self.prime(frame, strict);
        }
        let time = frame
            .timestamp_us
            .checked_sub(self.epoch)
            .ok_or(RecordingFailure::Media)?
            / 1000;
        if time < self.last {
            return Err(RecordingFailure::Media);
        }
        match self.writer.as_mut().unwrap().push_video(frame, time) {
            Ok(()) => {
                self.last = time;
                Ok(())
            }
            Err(RecordingError::Limit | RecordingError::FormatChanged) => {
                self.roll()?;
                self.prime(frame, false)
            }
            Err(error) => Err(classify(error)),
        }
    }

    /// Rolls the live segment on the wall-clock backstop. No segments are
    /// created while waiting for the first IDR.
    pub fn roll_wall(&mut self) -> Result<(), RecordingFailure> {
        if self.writer.is_none() {
            return Ok(());
        }
        self.roll()
    }

    /// Finalizes the live segment. Startup when nothing was ever recorded;
    /// stopping in the gap after a roll keeps the finished segments.
    pub fn finish(self) -> Result<(), RecordingFailure> {
        self.finish_snapshot().map(|_| ())
    }
    /// Finalize and return cumulative totals including the final muxing bytes.
    /// Retention does not subtract previously produced segments from totals.
    pub fn finish_snapshot(mut self) -> Result<RecordingSnapshot, RecordingFailure> {
        self.finalize()
    }

    /// Keep the writer available for a terminal snapshot even if storage fails.
    pub(crate) fn finalize(&mut self) -> Result<RecordingSnapshot, RecordingFailure> {
        if self.writer.is_some() {
            self.roll()?;
        }
        if self.segments == 0 {
            return Err(RecordingFailure::Startup);
        }
        Ok(self.snapshot())
    }

    pub fn snapshot(&self) -> RecordingSnapshot {
        let (frames, bytes) = self
            .writer
            .as_ref()
            .map(|w| (w.frames(), w.bytes()))
            .unwrap_or(self.unfinished);
        RecordingSnapshot {
            frames: self.finished_frames + frames,
            bytes: self.finished_bytes + bytes,
            duration_ms: self.finished_duration + self.last,
            segments: self.segments,
        }
    }

    pub fn primed(&self) -> bool {
        self.writer.is_some()
    }

    fn prime(
        &mut self,
        frame: &vnilink::EncodedFrame,
        strict: bool,
    ) -> Result<(), RecordingFailure> {
        let parsed = parse_annex_b(&frame.payload).map_err(|_| RecordingFailure::Media)?;
        if !(frame.keyframe && parsed.has_idr() && parsed.has_sps() && parsed.has_pps()) {
            if strict {
                return Err(RecordingFailure::Media);
            }
            return Ok(());
        }
        let sps = parsed
            .units()
            .iter()
            .find(|n| n.nal_type == 7)
            .ok_or(RecordingFailure::Media)?;
        let (width, height) =
            h264_picture_size(sps.bytes()).map_err(|_| RecordingFailure::Media)?;
        let partial = Self::segment_paths(&self.dir, &self.stem, self.index).1;
        let file = vnilink_rename::create_recording_file(&partial)
            .map_err(|_| RecordingFailure::Storage)?;
        let original = file.try_clone().map_err(|_| RecordingFailure::Storage)?;
        #[cfg(test)]
        if let Some(hook) = V14_AFTER_CREATE.with(|h| h.borrow_mut().take()) {
            hook(&partial);
        }
        self.epoch = frame.timestamp_us;
        self.last = 0;
        match MatroskaRecorder::start(file, width, height, frame, self.limits) {
            Ok(writer) => {
                self.writer = Some(writer);
                self.partial = Some(partial);
                Ok(())
            }
            Err(error) => {
                // Initialization consumed the writer, but this handle still
                // identifies the object created above, including written headers.
                let ownership = vnilink_rename::recording_retention::RecordedFile::new(&original)
                    .map_err(|_| RecordingFailure::Storage)?;
                drop(original);
                ownership
                    .remove(&partial)
                    .map_err(|_| RecordingFailure::Storage)?;
                Err(classify_prime(error))
            }
        }
    }

    /// A fresh segment rejects only a frame that cannot fit any segment; the
    /// byte budget maps to Limit while corrupt parameters stay Media.
    fn roll(&mut self) -> Result<(), RecordingFailure> {
        let Some(writer) = self.writer.take() else {
            return Ok(());
        };
        let frames = writer.frames();
        self.unfinished = (frames, writer.bytes());
        let partial = self.partial.take().ok_or(RecordingFailure::Storage)?;
        let destination = Self::segment_paths(&self.dir, &self.stem, self.index).0;
        let file = writer.finish().map_err(classify)?;
        let bytes = file
            .metadata()
            .map_err(|_| RecordingFailure::Storage)?
            .len();
        self.unfinished.1 = bytes;
        file.sync_all().map_err(|_| RecordingFailure::Storage)?;
        // Keep object identity and the original handle through publication.
        // No public pathname check can authorize the later source rename.
        let ownership = vnilink_rename::recording_retention::RecordedFile::new(&file)
            .map_err(|_| RecordingFailure::Storage)?;
        ownership
            .publish(&partial, &destination)
            .map_err(|_| RecordingFailure::Storage)?;
        drop(file);
        self.unfinished = (0, 0);
        self.finished_frames += frames;
        self.finished_bytes += bytes;
        self.finished_duration += self.last;
        self.last = 0;
        self.segments += 1;
        if self.retention.is_some() {
            self.owned.insert(self.index, ownership);
        }
        // Durable rename where the OS supports synchronizing directories.
        #[cfg(unix)]
        File::open(&self.dir)
            .and_then(|dir| dir.sync_all())
            .map_err(|_| RecordingFailure::Storage)?;
        self.sweep(self.index)?;
        self.index = self.index.saturating_add(1);
        Ok(())
    }

    /// Oldest-first sweep after finalize. The newest segment, live partial
    /// and foreign objects survive. Native deletion/restoration failures
    /// surface immediately as Storage. Usage includes only verified owned objects.
    fn sweep(&mut self, keep: u32) -> Result<(), RecordingFailure> {
        let Some(retention) = self.retention else {
            return Ok(());
        };
        let mut found: Vec<(u32, u64, PathBuf)> = Vec::new();
        let mut stale = Vec::new();
        for (&index, ownership) in &self.owned {
            let path = Self::segment_paths(&self.dir, &self.stem, index).0;
            match ownership
                .verified_size(&path)
                .map_err(|_| RecordingFailure::Storage)?
            {
                Some(bytes) => found.push((index, bytes, path)),
                None => stale.push(index),
            }
        }
        for index in stale {
            self.owned.remove(&index);
        }
        let mut count = found.len();
        let mut total: u64 = found.iter().map(|f| f.1).sum();
        for (index, len, path) in found {
            if count <= retention.max_segments as usize && total <= retention.max_total_bytes {
                break;
            }
            if index == keep || count <= 1 {
                continue;
            }
            let Some(ownership) = self.owned.remove(&index) else {
                continue;
            };
            ownership
                .remove(&path)
                .map_err(|_| RecordingFailure::Storage)?;
            // Even if public churn made removal refuse, this reservation
            // no longer belongs to our retained verified output window.
            count -= 1;
            total = total.saturating_sub(len);
        }
        Ok(())
    }

    fn segment_paths(dir: &Path, stem: &std::ffi::OsStr, index: u32) -> (PathBuf, PathBuf) {
        let mut name = OsString::from(stem);
        name.push(format!(".{index:06}.mkv"));
        let destination = dir.join(&name);
        name.push(".part");
        (destination, dir.join(name))
    }
}

fn classify(error: RecordingError) -> RecordingFailure {
    match error {
        RecordingError::Source(_) => RecordingFailure::Startup,
        RecordingError::Storage(_) => RecordingFailure::Storage,
        RecordingError::Limit => RecordingFailure::Limit,
        RecordingError::FormatChanged => RecordingFailure::FormatChanged,
        RecordingError::InvalidMedia => RecordingFailure::Media,
    }
}

fn classify_prime(error: RecordingError) -> RecordingFailure {
    match error {
        RecordingError::FormatChanged => RecordingFailure::Media,
        error => classify(error),
    }
}

#[cfg(test)]
thread_local! { pub(crate) static V14_AFTER_CREATE: std::cell::RefCell<Option<Box<dyn FnOnce(&Path)>>> = const { std::cell::RefCell::new(None) }; }
