//! Session-scoped binary data plane. Never takes the JSON bridge's State mutex.
use super::{core_error, parse_id, Command, CommandShape, Engine};
use napi::{
    bindgen_prelude::{AsyncTask, Buffer},
    Env, Error, Result, Task,
};
use napi_derive::napi;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, Weak};
use vnilink_app::{
    AudioService, CameraService, FfiTerminalEvent, FfiTerminalSize, TerminalService, VideoPoller,
    VnilinkCore,
};

/// Channel owners are private bridge resources; do not retain their closed records.
fn retire_service(core: &VnilinkCore, id: u64, generation: u64) {
    if core.close_controller_service(id, generation).is_ok() {
        let _ = core.forget_controller_service(id);
    }
}

/// Video and input share one Desktop owner, without holding the command mutex.
#[derive(Default)]
pub(crate) struct DesktopChannels(Mutex<HashMap<(u64, u64), Weak<DesktopOwner>>>);
/// The camera stream each connection last opened, so a recording can be
/// started on the pictures the viewer is already receiving.
#[derive(Default)]
pub(crate) struct CameraChannels(Mutex<HashMap<(u64, u64), Weak<CameraService>>>);
impl CameraChannels {
    fn register(&self, session: u64, generation: u64, camera: &Arc<CameraService>) {
        if let Ok(mut cameras) = self.0.lock() {
            cameras.retain(|_, camera| camera.strong_count() > 0);
            cameras.insert((session, generation), Arc::downgrade(camera));
        }
    }
    /// The live camera stream of this connection.
    pub(crate) fn get(&self, session: u64, generation: u64) -> Result<Arc<CameraService>> {
        self.0
            .lock()
            .map_err(|_| Error::from_reason("Camera channels unavailable"))?
            .get(&(session, generation))
            .and_then(Weak::upgrade)
            .filter(|camera| !camera.is_cancelled())
            .ok_or_else(|| Error::from_reason("NoSession: no camera is open on this connection"))
    }
}
pub(super) struct DesktopOwner {
    core: Arc<VnilinkCore>,
    pub(super) service: Arc<vnilink_app::DesktopService>,
    audio: Arc<vnilink_app::DesktopAudio>,
}
impl Drop for DesktopOwner {
    fn drop(&mut self) {
        self.audio.close();
        self.service.close();
        let info = self.service.info();
        retire_service(&self.core, info.id, info.generation);
    }
}
impl DesktopChannels {
    pub(super) fn open(
        &self,
        core: &Arc<VnilinkCore>,
        id: u64,
        generation: u64,
    ) -> Result<Arc<DesktopOwner>> {
        let mut channels = self
            .0
            .lock()
            .map_err(|_| Error::from_reason("Desktop channels unavailable"))?;
        channels.retain(|_, handle| handle.strong_count() > 0);
        if let Some(handle) = channels.get(&(id, generation)).and_then(Weak::upgrade) {
            if !handle.service.is_cancelled() {
                return Ok(handle);
            }
        }
        let owner = core
            .attach_controller_service(id, generation, vnilink_app::FfiServiceKind::Desktop)
            .map_err(core_error)?;
        let service = match core.open_desktop_service(owner.id, owner.generation) {
            Ok(service) => service,
            Err(error) => {
                retire_service(core, owner.id, owner.generation);
                return Err(core_error(error));
            }
        };
        let audio = vnilink_app::DesktopAudio::start(core.clone(), &service);
        let handle = Arc::new(DesktopOwner {
            core: core.clone(),
            audio,
            service,
        });
        channels.insert((id, generation), Arc::downgrade(&handle));
        Ok(handle)
    }
}

#[napi(object)]
pub struct DataEvent {
    pub kind: String,
    pub data: Option<Buffer>,
    pub metadata: String,
}

#[napi]
pub struct DataChannel {
    core: Arc<VnilinkCore>,
    session_id: u64,
    generation: u64,
    kind: String,
    input_used: std::sync::atomic::AtomicBool,
    owner: Option<(u64, u64)>,
    files: Mutex<Option<Arc<vnilink_app::FilesService>>>,
    jobs: Arc<Mutex<Vec<u64>>>,
    /// What this channel last told JavaScript about its jobs.
    job_view: Mutex<JobView>,
    /// The watched folder's change count last reported to the window.
    folder_seen: std::sync::atomic::AtomicU64,
    video: Mutex<Option<VideoPoller>>,
    desktop: Mutex<Option<Arc<DesktopOwner>>>,
    terminal: Mutex<Option<Arc<TerminalService>>>,
    camera: Mutex<Option<Arc<CameraService>>>,
    audio: Mutex<Option<Arc<AudioService>>>,
    closed: Arc<std::sync::atomic::AtomicBool>,
    snapshot_cancel: Arc<std::sync::atomic::AtomicBool>,
    snapshot: Arc<Mutex<SnapshotState>>,
}

/// Jobs as last sent: each one's last snapshot, and the revision read.
#[derive(Default)]
struct JobView {
    revision: Option<u64>,
    sent: HashMap<u64, String>,
    /// Changes not yet sent because one event filled up.
    more: bool,
    at: Option<std::time::Instant>,
}

/// Job updates go out at most this often; state changes wait no longer.
const JOB_UPDATE_INTERVAL: std::time::Duration = std::time::Duration::from_millis(250);

/// Room for job snapshots in one event, under the 16 KiB packet metadata.
const JOB_UPDATE_BYTES: usize = 12 * 1024;

enum SnapshotState {
    Idle,
    Pending,
    Ready(DataEvent),
}

fn snapshot_failure(outcome: &str) -> DataEvent {
    DataEvent {
        kind: "snapshot".into(),
        data: None,
        metadata: serde_json::json!({ "outcome": outcome }).to_string(),
    }
}

fn snapshot_event(
    shot: vnilink_app::FfiScreenSnapshot,
    endpoint_id: &str,
    session_id: u64,
    generation: u64,
) -> DataEvent {
    match shot {
        vnilink_app::FfiScreenSnapshot::Ready {
            jpeg,
            width,
            height,
            captured_unix_ms,
            display_id,
        } => DataEvent {
            kind: "snapshot".into(),
            data: Some(jpeg.into()),
            metadata: serde_json::json!({
                "outcome": "ready",
                "width": width,
                "height": height,
                "capturedUnixMs": captured_unix_ms.to_string(),
                "displayId": display_id,
                "mime": "image/jpeg",
                "endpointId": endpoint_id,
                "sessionId": session_id.to_string(),
                "generation": generation.to_string(),
            })
            .to_string(),
        },
        other => snapshot_failure(snapshot_outcome(&other)),
    }
}

fn snapshot_outcome(shot: &vnilink_app::FfiScreenSnapshot) -> &'static str {
    match shot {
        vnilink_app::FfiScreenSnapshot::Ready { .. } => "ready",
        vnilink_app::FfiScreenSnapshot::NeedsPermission => "needsPermission",
        vnilink_app::FfiScreenSnapshot::NeedsSourceSelection => "needsSourceSelection",
        vnilink_app::FfiScreenSnapshot::Unavailable => "unavailable",
        vnilink_app::FfiScreenSnapshot::TimedOut => "timedOut",
        vnilink_app::FfiScreenSnapshot::Cancelled => "cancelled",
    }
}

fn publish_snapshot(
    core: Arc<VnilinkCore>,
    id: u64,
    generation: u64,
    cancel: Arc<std::sync::atomic::AtomicBool>,
    mailbox: Arc<Mutex<SnapshotState>>,
    prompt: bool,
    access: bool,
) {
    let event = if access {
        match core.screen_capture_access(id, generation) {
            Ok(vnilink_app::FfiScreenAccess::Allowed) => snapshot_failure("allowed"),
            Ok(vnilink_app::FfiScreenAccess::Denied) => snapshot_failure("sharingDisabled"),
            Ok(vnilink_app::FfiScreenAccess::Unavailable) => snapshot_failure("unavailable"),
            Err(error) => snapshot_failure(vnilink_app::snapshot_error_outcome(&error)),
        }
    } else {
        match core.request_screen_snapshot(id, generation, &cancel, prompt) {
            Ok(shot) => match core.controller_session(id) {
                Ok(session) if session.generation == generation => {
                    snapshot_event(shot, &session.peer_id, id, generation)
                }
                Ok(_) => snapshot_failure("replaced"),
                Err(error) => snapshot_failure(vnilink_app::snapshot_error_outcome(&error)),
            },
            Err(error) => snapshot_failure(vnilink_app::snapshot_error_outcome(&error)),
        }
    };
    if cancel.load(std::sync::atomic::Ordering::Acquire) {
        return;
    }
    if let Ok(mut slot) = mailbox.lock() {
        if cancel.load(std::sync::atomic::Ordering::Acquire) {
            return;
        }
        *slot = SnapshotState::Ready(event);
    }
}

impl DataChannel {
    fn check(&self) -> Result<()> {
        self.check_for(false)
    }
    fn check_for(&self, drain_terminal: bool) -> Result<()> {
        if self.closed.load(std::sync::atomic::Ordering::Acquire) {
            return Err(Error::from_reason("Data channel closed"));
        }
        let session = self
            .core
            .controller_session(self.session_id)
            .map_err(core_error)?;
        if session.generation != self.generation
            || !matches!(session.phase, vnilink_app::FfiSessionPhase::Active)
        {
            return Err(Error::from_reason("Stale session generation"));
        }
        if let Some((id, generation)) = self.owner {
            let service = self.core.controller_service(id).map_err(core_error)?;
            let readable_exit = drain_terminal
                && self.kind == "terminal"
                && matches!(
                    service.phase,
                    vnilink_app::FfiServicePhase::Closed {
                        cause: vnilink_app::FfiCloseCause::PeerClose
                    }
                );
            if service.generation != generation
                || !(matches!(service.phase, vnilink_app::FfiServicePhase::Active) || readable_exit)
            {
                return Err(Error::from_reason("Service closed"));
            }
        }
        if let Some(desktop) = self
            .desktop
            .lock()
            .map_err(|_| Error::from_reason("Channel unavailable"))?
            .as_ref()
        {
            if desktop.service.is_cancelled() {
                return Err(Error::from_reason("Desktop service closed"));
            }
        }
        Ok(())
    }
}

#[napi]
impl DataChannel {
    /// Nonblocking bounded drain; JS pushes the resulting bytes and waits for
    /// acknowledgement before pulling again. No libuv worker per frame.
    #[napi]
    pub fn read(&self) -> Result<Option<DataEvent>> {
        self.check_for(true)?;
        if self.kind == "video" {
            let video = self
                .video
                .lock()
                .map_err(|_| Error::from_reason("Channel unavailable"))?;
            return Ok(video.as_ref().and_then(VideoPoller::try_recv).map(|frame| DataEvent {
                kind: "video".into(),
                data: Some(frame.payload.into()),
                metadata: serde_json::json!({"codec":frame.codec.label(),"keyframe":frame.keyframe,"timestampUs":frame.timestamp_us.to_string(),"displayId":frame.display_id.to_string(),"revision":frame.revision.to_string()}).to_string(),
            }));
        }
        if self.kind == "camera" {
            let camera = self
                .camera
                .lock()
                .map_err(|_| Error::from_reason("Channel unavailable"))?;
            let Some(camera) = camera.as_ref() else {
                return Ok(None);
            };
            if let Some(frame) = camera.poll() {
                let config = camera.config();
                return Ok(Some(DataEvent { kind: "camera".into(), data: Some(frame.payload.into()), metadata: serde_json::json!({"codec":frame.codec.label(),"keyframe":frame.keyframe,"timestampUs":frame.timestamp_us.to_string(),"width":config.width,"height":config.height}).to_string() }));
            }
            // The last packet says why the picture stopped.
            return Ok(camera.take_end().map(|end| DataEvent {
                kind: "exited".into(),
                data: None,
                metadata: serde_json::json!({ "reason": match end {
                    vnilink_app::FfiCameraEnd::Stopped => "stopped",
                    vnilink_app::FfiCameraEnd::Withdrawn => "withdrawn",
                    vnilink_app::FfiCameraEnd::Lost => "lost",
                } })
                .to_string(),
            }));
        }
        if self.kind == "snapshot" {
            let mut slot = self
                .snapshot
                .lock()
                .map_err(|_| Error::from_reason("Channel unavailable"))?;
            return Ok(match std::mem::replace(&mut *slot, SnapshotState::Idle) {
                SnapshotState::Ready(event) => Some(event),
                pending => {
                    *slot = pending;
                    None
                }
            });
        }
        if self.kind == "audio" {
            let audio = self
                .audio
                .lock()
                .map_err(|_| Error::from_reason("Channel unavailable"))?;
            if let Some(audio) = audio.as_ref() {
                if let Some(block) = audio.poll_playback().map_err(core_error)? {
                    let bytes: Vec<u8> = block
                        .samples
                        .into_iter()
                        .flat_map(f32::to_le_bytes)
                        .collect();
                    return Ok(Some(DataEvent { kind: "audio".into(), data: Some(bytes.into()), metadata: serde_json::json!({"timestamp":block.timestamp.to_string(),"channels":audio.config().channels,"sampleRate":48000,"concealed":block.concealed}).to_string() }));
                }
            }
            return Ok(None);
        }
        if self.kind == "files" {
            if let Some(changed) = self.folder_update()? {
                return Ok(Some(changed));
            }
            return self.job_updates();
        }
        if self.kind == "terminal" {
            let terminal = self
                .terminal
                .lock()
                .map_err(|_| Error::from_reason("Channel unavailable"))?;
            return Ok(terminal.as_ref().and_then(|service| service.poll()).map(
                |event| match event {
                    FfiTerminalEvent::Output { bytes } => DataEvent {
                        kind: "terminal".into(),
                        data: Some(bytes.into()),
                        metadata: "{}".into(),
                    },
                    FfiTerminalEvent::Exited { code } => DataEvent {
                        kind: "exited".into(),
                        data: None,
                        metadata: serde_json::json!({"code":code}).to_string(),
                    },
                    FfiTerminalEvent::OutputGap { lost_bytes } => DataEvent {
                        kind: "gap".into(),
                        data: None,
                        metadata: serde_json::json!({"lostBytes":lost_bytes.to_string()})
                            .to_string(),
                    },
                    FfiTerminalEvent::Failed { .. } => DataEvent {
                        kind: "failed".into(),
                        data: None,
                        metadata: "{}".into(),
                    },
                },
            ));
        }
        Ok(None)
    }

    /// Says, unasked, that the folder the window watches on the other
    /// computer changed since it was last said.
    fn folder_update(&self) -> Result<Option<DataEvent>> {
        let Some(files) = self
            .files
            .lock()
            .map_err(|_| Error::from_reason("Channel unavailable"))?
            .clone()
        else {
            return Ok(None);
        };
        let revision = files.folder_revision();
        if self
            .folder_seen
            .swap(revision, std::sync::atomic::Ordering::AcqRel)
            == revision
        {
            return Ok(None);
        }
        Ok(Some(DataEvent {
            kind: "folderChanged".into(),
            data: None,
            metadata: "{}".into(),
        }))
    }

    /// This channel's jobs that changed since they were last sent, unasked.
    /// Reads them again only when the engine's job revision moved.
    fn job_updates(&self) -> Result<Option<DataEvent>> {
        let Some(files) = self
            .files
            .lock()
            .map_err(|_| Error::from_reason("Channel unavailable"))?
            .clone()
        else {
            return Ok(None);
        };
        let mut view = self
            .job_view
            .lock()
            .map_err(|_| Error::from_reason("Channel unavailable"))?;
        let revision = files.job_revision().map_err(core_error)?;
        if (view.revision == Some(revision) && !view.more)
            || view.at.is_some_and(|at| at.elapsed() < JOB_UPDATE_INTERVAL)
        {
            return Ok(None);
        }
        view.revision = Some(revision);
        view.at = Some(std::time::Instant::now());
        view.more = false;
        let mut changed = Vec::new();
        let mut bytes = 0;
        for job in files.jobs().map_err(core_error)? {
            let shape = crate::transfer_job(&job);
            let text =
                serde_json::to_string(&shape).map_err(|_| Error::from_reason("Invalid job"))?;
            if view.sent.get(&job.id) == Some(&text) {
                continue;
            }
            if !changed.is_empty() && bytes + text.len() > JOB_UPDATE_BYTES {
                view.more = true;
                break;
            }
            bytes += text.len();
            view.sent.insert(job.id, text);
            changed.push(shape);
        }
        if changed.is_empty() {
            return Ok(None);
        }
        Ok(Some(DataEvent {
            kind: "jobs".into(),
            data: None,
            metadata: serde_json::json!({ "jobs": changed }).to_string(),
        }))
    }

    #[napi]
    pub fn write(&self, bytes: Buffer) -> Result<()> {
        self.check()?;
        if self.kind != "terminal" || bytes.is_empty() || bytes.len() > 16 * 1024 {
            return Err(Error::from_reason("Invalid terminal input"));
        }
        let terminal = self
            .terminal
            .lock()
            .map_err(|_| Error::from_reason("Channel unavailable"))?;
        terminal
            .as_ref()
            .ok_or_else(|| Error::from_reason("Channel closed"))?
            .input(bytes.to_vec())
            .map_err(core_error)
    }

    #[napi]
    pub fn resize(&self, rows: u16, columns: u16) -> Result<()> {
        self.check()?;
        let terminal = self
            .terminal
            .lock()
            .map_err(|_| Error::from_reason("Channel unavailable"))?;
        terminal
            .as_ref()
            .ok_or_else(|| Error::from_reason("Not a terminal channel"))?
            .resize(FfiTerminalSize { rows, columns })
            .map_err(core_error)
    }

    #[napi]
    pub fn input(&self, command: String) -> Result<()> {
        self.check()?;
        if self.kind != "input" || command.len() > 8192 {
            return Err(Error::from_reason("Invalid input"));
        }
        let raw: serde_json::Value =
            serde_json::from_str(&command).map_err(|_| Error::from_reason("Invalid input"))?;
        if !CommandShape::check(&raw)
            || raw["sessionId"] != self.session_id.to_string()
            || raw["generation"] != self.generation.to_string()
        {
            return Err(Error::from_reason("Invalid input target"));
        }
        let decoded: Command =
            serde_json::from_value(raw).map_err(|_| Error::from_reason("Invalid input"))?;
        if !matches!(
            decoded,
            Command::SubmitKey { .. }
                | Command::SubmitPointer { .. }
                | Command::SubmitButton { .. }
                | Command::SubmitWheel { .. }
                | Command::SubmitMotion { .. }
                | Command::SubmitPreciseWheel { .. }
                | Command::SubmitText { .. }
                | Command::SubmitLocalKey { .. }
                | Command::ReleaseSessionInput { .. }
        ) {
            return Err(Error::from_reason("Not an interactive input command"));
        }
        self.input_used
            .store(true, std::sync::atomic::Ordering::Release);
        let desktop = self
            .desktop
            .lock()
            .map_err(|_| Error::from_reason("Channel unavailable"))?;
        let service = &desktop
            .as_ref()
            .ok_or_else(|| Error::from_reason("Missing desktop owner"))?
            .service;
        match decoded {
            Command::SubmitKey { usage, down, .. } => service.submit_key(usage, down),
            Command::SubmitPointer {
                x,
                y,
                display,
                revision,
                ..
            } => service.submit_pointer(
                x,
                y,
                parse_id(&display, "Invalid display ID")?,
                parse_id(&revision, "Invalid topology revision")?,
            ),
            Command::SubmitButton { button, down, .. } => {
                service.submit_button(super::pointer_button(&button)?, down)
            }
            Command::SubmitWheel { dx, dy, .. } => service.submit_wheel(dx, dy),
            Command::SubmitMotion { dx, dy, .. } => service.submit_motion(dx, dy),
            Command::SubmitPreciseWheel { dx, dy, .. } => service.submit_precise_wheel(dx, dy),
            Command::SubmitText { text, .. } => service.submit_text(text),
            Command::SubmitLocalKey {
                usage, down, text, ..
            } => service.submit_local_key(usage, down, text),
            Command::ReleaseSessionInput { .. } => service.release_input(),
            _ => unreachable!("input allowlist checked above"),
        }
        .map_err(core_error)?;
        Ok(())
    }

    #[napi]
    pub fn file_operation(&self, command: String) -> Result<AsyncTask<FileTask>> {
        self.check()?;
        if self.kind != "files" || command.len() > 16384 {
            return Err(Error::from_reason("Invalid file operation"));
        }
        let operation: FileOperation = serde_json::from_str(&command)
            .map_err(|_| Error::from_reason("Invalid file operation"))?;
        Ok(AsyncTask::new(FileTask {
            core: self.core.clone(),
            session: self.session_id,
            generation: self.generation,
            files: self
                .files
                .lock()
                .map_err(|_| Error::from_reason("Channel unavailable"))?
                .as_ref()
                .cloned()
                .ok_or_else(|| Error::from_reason("Missing file owner"))?,
            jobs: self.jobs.clone(),
            closed: self.closed.clone(),
            operation,
        }))
    }

    /// Control/status only; decoded desktop audio never crosses the renderer boundary.
    #[napi]
    pub fn desktop_audio(&self, operation: String) -> Result<String> {
        self.check()?;
        if !matches!(self.kind.as_str(), "video" | "input") || operation.len() > 128 {
            return Err(Error::from_reason("Invalid desktop audio target"));
        }
        let raw: serde_json::Value = serde_json::from_str(&operation)
            .map_err(|_| Error::from_reason("Invalid desktop audio operation"))?;
        let keys = if raw["type"] == "status" { 1 } else { 2 };
        if raw.as_object().is_none_or(|object| object.len() != keys) {
            return Err(Error::from_reason("Invalid desktop audio operation"));
        }
        let operation: DesktopAudioOperation = serde_json::from_str(&operation)
            .map_err(|_| Error::from_reason("Invalid desktop audio operation"))?;
        let desktop = self
            .desktop
            .lock()
            .map_err(|_| Error::from_reason("Desktop unavailable"))?;
        let audio = &desktop
            .as_ref()
            .ok_or_else(|| Error::from_reason("Desktop unavailable"))?
            .audio;
        match operation {
            DesktopAudioOperation::Status => {}
            DesktopAudioOperation::Enabled { enabled } => audio.set_enabled(enabled),
            DesktopAudioOperation::Muted { muted } => audio.set_muted(muted),
        }
        let status = audio.status();
        let phase = match status.phase {
            vnilink_app::DesktopAudioPhase::Starting => "starting",
            vnilink_app::DesktopAudioPhase::Playing => "playing",
            vnilink_app::DesktopAudioPhase::Muted => "muted",
            vnilink_app::DesktopAudioPhase::Unavailable => "unavailable",
            vnilink_app::DesktopAudioPhase::Stopped => "stopped",
        };
        Ok(serde_json::json!({"phase":phase,"enabled":status.enabled,"muted":status.muted,
            "error":status.error.map(|error| serde_json::json!({"code":super::ffi_error_name(&error),"message":error.to_string()}))}).to_string())
    }

    #[napi]
    pub fn close(&self) -> AsyncTask<CloseTask> {
        AsyncTask::new(self.take_cleanup())
    }
}
impl DataChannel {
    fn take_cleanup(&self) -> CloseTask {
        self.snapshot_cancel
            .store(true, std::sync::atomic::Ordering::Release);
        if let Ok(mut slot) = self.snapshot.lock() {
            *slot = SnapshotState::Idle;
        }
        let already = self.closed.swap(true, std::sync::atomic::Ordering::AcqRel);
        if !already
            && self.kind == "input"
            && self
                .input_used
                .swap(false, std::sync::atomic::Ordering::AcqRel)
        {
            if let Ok(desktop) = self.desktop.lock() {
                if let Some(desktop) = desktop.as_ref() {
                    let _ = desktop.service.release_input();
                }
            }
        }
        // Unfinished transfers are not cancelled: retiring the owner leaves
        // them interrupted, for the next channel to this computer to resume.
        if !already {
            if let Ok(mut jobs) = self.jobs.lock() {
                jobs.clear();
            }
        }
        if !already {
            if let Ok(mut terminal) = self.terminal.lock() {
                if let Some(terminal) = terminal.take() {
                    terminal.close();
                }
            }
            if let Ok(mut camera) = self.camera.lock() {
                if let Some(camera) = camera.take() {
                    camera.close();
                }
            }
            if let Ok(mut audio) = self.audio.lock() {
                if let Some(audio) = audio.take() {
                    audio.close();
                }
            }
            if let Ok(mut video) = self.video.lock() {
                video.take();
            }
        }
        let files = if already {
            None
        } else {
            self.files.lock().ok().and_then(|mut files| files.take())
        };
        CloseTask {
            core: self.core.clone(),
            owner: if already { None } else { self.owner },
            files,
            desktop: if already {
                None
            } else {
                self.desktop
                    .lock()
                    .ok()
                    .and_then(|mut desktop| desktop.take())
            },
        }
    }
}
impl Drop for DataChannel {
    fn drop(&mut self) {
        if !self.closed.load(std::sync::atomic::Ordering::Acquire) {
            let _ = self.take_cleanup().compute();
        }
    }
}
pub struct CloseTask {
    core: Arc<VnilinkCore>,
    owner: Option<(u64, u64)>,
    files: Option<Arc<vnilink_app::FilesService>>,
    desktop: Option<Arc<DesktopOwner>>,
}
impl Task for CloseTask {
    type Output = ();
    type JsValue = ();
    fn compute(&mut self) -> Result<()> {
        if let Some((id, generation)) = self.owner.take() {
            retire_service(&self.core, id, generation);
        }
        self.files.take();
        self.desktop.take();
        Ok(())
    }
    fn resolve(&mut self, _: Env, _: ()) -> Result<()> {
        Ok(())
    }
}

#[derive(serde::Deserialize)]
#[serde(tag = "type", rename_all = "camelCase", deny_unknown_fields)]
enum DesktopAudioOperation {
    Status,
    Enabled { enabled: bool },
    Muted { muted: bool },
}

#[derive(serde::Deserialize)]
#[serde(tag = "type", rename_all = "camelCase", deny_unknown_fields)]
pub enum MediaConfig {
    Camera {
        #[serde(rename = "deviceId")]
        device_id: String,
        width: u16,
        height: u16,
        fps: u16,
    },
    Audio {
        #[serde(rename = "deviceId")]
        device_id: String,
        #[serde(rename = "sourceKind")]
        source_kind: String,
        channels: u8,
        voice: bool,
    },
    /// `access` asks whether a screenshot would be admitted. It does not capture.
    /// `prompt` asks the host to show its own consent or source dialog.
    Snapshot { prompt: bool, access: bool },
}
pub struct OpenChannelTask {
    core: Arc<VnilinkCore>,
    desktops: Arc<DesktopChannels>,
    cameras: Arc<CameraChannels>,
    id: u64,
    generation: u64,
    kind: String,
    rows: u16,
    columns: u16,
    media: Option<MediaConfig>,
}
impl Task for OpenChannelTask {
    type Output = DataChannel;
    type JsValue = DataChannel;
    fn compute(&mut self) -> Result<DataChannel> {
        let session = self.core.controller_session(self.id).map_err(core_error)?;
        if session.generation != self.generation
            || !matches!(session.phase, vnilink_app::FfiSessionPhase::Active)
        {
            return Err(Error::from_reason("Stale session generation"));
        }
        let desktop = if matches!(self.kind.as_str(), "video" | "input") {
            Some(self.desktops.open(&self.core, self.id, self.generation)?)
        } else {
            None
        };
        let video = if self.kind == "video" {
            Some(
                desktop
                    .as_ref()
                    .unwrap()
                    .service
                    .subscribe_video()
                    .map_err(core_error)?,
            )
        } else {
            None
        };
        let kind = match self.kind.as_str() {
            "terminal" => Some(vnilink_app::FfiServiceKind::Terminal),
            "files" => Some(vnilink_app::FfiServiceKind::Files),
            "camera" => Some(vnilink_app::FfiServiceKind::Camera),
            "audio" => Some(match &self.media {
                Some(MediaConfig::Audio { source_kind, .. }) if source_kind == "Microphone" => {
                    vnilink_app::FfiServiceKind::Microphone
                }
                Some(MediaConfig::Audio { source_kind, .. }) if source_kind == "System" => {
                    vnilink_app::FfiServiceKind::SystemAudio
                }
                _ => return Err(Error::from_reason("Invalid audio source kind")),
            }),
            _ => None,
        };
        let owner = kind
            .map(|kind| {
                self.core
                    .attach_controller_service(self.id, self.generation, kind)
            })
            .transpose()
            .map_err(core_error)?;
        let terminal = if let Some(owner) = owner.as_ref().filter(|_| self.kind == "terminal") {
            match self.core.open_terminal_service(
                owner.id,
                owner.generation,
                FfiTerminalSize {
                    rows: self.rows,
                    columns: self.columns,
                },
            ) {
                Ok(service) => Some(service),
                Err(error) => {
                    retire_service(&self.core, owner.id, owner.generation);
                    return Err(core_error(error));
                }
            }
        } else {
            None
        };
        let files = if let Some(owner) = owner.as_ref().filter(|_| self.kind == "files") {
            match self.core.open_files_service(owner.id, owner.generation) {
                Ok(service) => Some(service),
                Err(error) => {
                    retire_service(&self.core, owner.id, owner.generation);
                    return Err(core_error(error));
                }
            }
        } else {
            None
        };
        let camera = if let Some(owner) = owner.as_ref().filter(|_| self.kind == "camera") {
            let Some(MediaConfig::Camera {
                device_id,
                width,
                height,
                fps,
            }) = &self.media
            else {
                retire_service(&self.core, owner.id, owner.generation);
                return Err(Error::from_reason("Invalid camera configuration"));
            };
            match self.core.open_camera_service(
                owner.id,
                owner.generation,
                vnilink_app::FfiCameraConfig {
                    device_id: device_id.clone(),
                    width: *width,
                    height: *height,
                    fps: *fps,
                },
            ) {
                Ok(service) => {
                    self.cameras.register(self.id, self.generation, &service);
                    Some(service)
                }
                Err(error) => {
                    retire_service(&self.core, owner.id, owner.generation);
                    return Err(core_error(error));
                }
            }
        } else {
            None
        };
        let audio = if let Some(owner) = owner.as_ref().filter(|_| self.kind == "audio") {
            let Some(MediaConfig::Audio {
                device_id,
                source_kind,
                channels,
                voice,
            }) = &self.media
            else {
                retire_service(&self.core, owner.id, owner.generation);
                return Err(Error::from_reason("Invalid audio configuration"));
            };
            let kind = if source_kind == "Microphone" {
                vnilink_app::FfiAudioKind::Microphone
            } else {
                vnilink_app::FfiAudioKind::System
            };
            match self.core.open_audio_service(
                owner.id,
                owner.generation,
                vnilink_app::FfiAudioConfig {
                    device_id: device_id.clone(),
                    kind,
                    channels: *channels,
                    voice: *voice,
                },
            ) {
                Ok(service) => Some(service),
                Err(error) => {
                    retire_service(&self.core, owner.id, owner.generation);
                    return Err(core_error(error));
                }
            }
        } else {
            None
        };
        let snapshot_cancel = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let snapshot = Arc::new(Mutex::new(if self.kind == "snapshot" {
            SnapshotState::Pending
        } else {
            SnapshotState::Idle
        }));
        if self.kind == "snapshot" {
            let core = Arc::clone(&self.core);
            let cancel = Arc::clone(&snapshot_cancel);
            let mailbox = Arc::clone(&snapshot);
            let id = self.id;
            let generation = self.generation;
            let (prompt, access) = match &self.media {
                Some(MediaConfig::Snapshot { prompt, access }) => (*prompt, *access),
                _ => (false, false),
            };
            if std::thread::Builder::new()
                .name("vnilink-snapshot".into())
                .spawn(move || {
                    publish_snapshot(core, id, generation, cancel, mailbox, prompt, access)
                })
                .is_err()
            {
                if let Ok(mut slot) = snapshot.lock() {
                    *slot = SnapshotState::Ready(snapshot_failure("failed"));
                }
            }
        }
        Ok(DataChannel {
            core: self.core.clone(),
            session_id: self.id,
            generation: self.generation,
            kind: self.kind.clone(),
            input_used: std::sync::atomic::AtomicBool::new(false),
            owner: owner.map(|owner| (owner.id, owner.generation)),
            files: Mutex::new(files),
            jobs: Arc::new(Mutex::new(Vec::new())),
            job_view: Mutex::new(JobView::default()),
            folder_seen: std::sync::atomic::AtomicU64::new(0),
            video: Mutex::new(video),
            desktop: Mutex::new(desktop),
            terminal: Mutex::new(terminal),
            camera: Mutex::new(camera),
            audio: Mutex::new(audio),
            closed: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            snapshot_cancel,
            snapshot,
        })
    }
    fn resolve(&mut self, _: Env, channel: DataChannel) -> Result<DataChannel> {
        Ok(channel)
    }
}
#[napi]
impl Engine {
    #[napi]
    pub fn open_data_channel(
        &self,
        kind: String,
        session_id: String,
        generation: String,
        rows: u16,
        columns: u16,
        media: Option<String>,
    ) -> Result<AsyncTask<OpenChannelTask>> {
        if !matches!(
            kind.as_str(),
            "video" | "terminal" | "input" | "files" | "camera" | "audio" | "snapshot"
        ) {
            return Err(Error::from_reason("Invalid channel kind"));
        }
        let config = media
            .map(|raw| {
                if raw.len() > 4096 {
                    return Err(Error::from_reason("Media configuration too large"));
                }
                serde_json::from_str::<MediaConfig>(&raw)
                    .map_err(|_| Error::from_reason("Invalid media configuration"))
            })
            .transpose()?;
        match (&*kind, &config) {
            ("camera", Some(MediaConfig::Camera { .. }))
            | ("audio", Some(MediaConfig::Audio { .. }))
            | ("snapshot", Some(MediaConfig::Snapshot { .. })) => {}
            ("camera" | "audio" | "snapshot", _) => {
                return Err(Error::from_reason("Media configuration required"))
            }
            (_, Some(_)) => return Err(Error::from_reason("Unexpected media configuration")),
            _ => {}
        }
        Ok(AsyncTask::new(OpenChannelTask {
            core: self.core.clone(),
            desktops: self.desktops.clone(),
            cameras: self.cameras.clone(),
            id: parse_id(&session_id, "Invalid session ID")?,
            generation: parse_id(&generation, "Invalid generation")?,
            kind,
            rows,
            columns,
            media: config,
        }))
    }
}

/// A folder on the other computer, by its root and path under the root.
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct WatchFolder {
    root: String,
    path: String,
}

#[derive(serde::Deserialize)]
#[serde(tag = "type", rename_all = "camelCase", deny_unknown_fields)]
enum FileOperation {
    Places,
    List {
        root: String,
        path: String,
        offset: String,
    },
    Mkdir {
        root: String,
        path: String,
        name: String,
    },
    Rename {
        root: String,
        path: String,
        name: String,
        token: Option<crate::files::TransferToken>,
        #[serde(rename = "newName")]
        new_name: String,
    },
    /// To the other computer's Trash when `trash`, else permanently.
    Delete {
        root: String,
        path: String,
        name: String,
        token: Option<crate::files::TransferToken>,
        trash: bool,
    },
    Submit {
        direction: String,
        #[serde(rename = "srcRoot")]
        src_root: String,
        #[serde(rename = "srcPath")]
        src_path: String,
        #[serde(rename = "dstRoot")]
        dst_root: String,
        #[serde(rename = "dstPath")]
        dst_path: String,
    },
    Jobs,
    /// Takes over and resumes the transfers a lost channel left interrupted.
    Adopt,
    /// Watches the folder the window shows of the other computer, or none.
    Watch {
        folder: Option<WatchFolder>,
    },
    Cancel {
        #[serde(rename = "jobId")]
        job_id: String,
    },
    Resume {
        #[serde(rename = "jobId")]
        job_id: String,
    },
    Forget {
        #[serde(rename = "jobId")]
        job_id: String,
    },
    Resolve {
        #[serde(rename = "jobId")]
        job_id: String,
        choice: String,
        #[serde(rename = "applyToRemaining")]
        apply_to_remaining: bool,
    },
}
pub struct FileTask {
    files: Arc<vnilink_app::FilesService>,
    core: Arc<VnilinkCore>,
    session: u64,
    generation: u64,
    jobs: Arc<Mutex<Vec<u64>>>,
    closed: Arc<std::sync::atomic::AtomicBool>,
    operation: FileOperation,
}
impl Task for FileTask {
    type Output = String;
    type JsValue = String;
    fn compute(&mut self) -> Result<String> {
        if self.closed.load(std::sync::atomic::Ordering::Acquire) {
            return Err(Error::from_reason("Channel closed"));
        }
        let session = self
            .core
            .controller_session(self.session)
            .map_err(core_error)?;
        if session.generation != self.generation
            || !matches!(session.phase, vnilink_app::FfiSessionPhase::Active)
        {
            return Err(Error::from_reason("Stale file session"));
        }
        // What the engine refused is an answer, not a broken channel.
        Ok(match self.answer() {
            Ok(value) => serde_json::json!({ "ok": value }),
            Err(Refused(error)) => serde_json::json!({ "error": error }),
        }
        .to_string())
    }
    fn resolve(&mut self, _: Env, value: String) -> Result<String> {
        Ok(value)
    }
}

/// An operation the engine refused, as `Category: message`.
struct Refused(String);

impl From<vnilink_app::FfiError> for Refused {
    fn from(error: vnilink_app::FfiError) -> Self {
        Self(core_error(error).reason)
    }
}

impl From<Error> for Refused {
    fn from(error: Error) -> Self {
        Self(error.reason)
    }
}

impl FileTask {
    fn owned(&self, job_id: &str) -> std::result::Result<u64, Refused> {
        let job = parse_id(job_id, "Invalid job")?;
        if !self
            .jobs
            .lock()
            .map_err(|_| Error::from_reason("Channel unavailable"))?
            .contains(&job)
        {
            return Err(Refused(
                "Unauthorized: the job belongs to another window".into(),
            ));
        }
        Ok(job)
    }

    fn json<T: serde::Serialize>(value: T) -> std::result::Result<serde_json::Value, Refused> {
        serde_json::to_value(value)
            .map_err(|_| Refused("Malformed: the reply could not be encoded".into()))
    }

    fn answer(&self) -> std::result::Result<serde_json::Value, Refused> {
        let files = &self.files;
        match &self.operation {
            FileOperation::Places => Self::json(crate::files::transfer_places(files.places()?)),
            FileOperation::List { root, path, offset } => {
                Self::json(crate::files::transfer_page(files.list(
                    parse_id(root, "Invalid root")?,
                    path.clone(),
                    parse_id(offset, "Invalid offset")?,
                    500,
                )?))
            }
            FileOperation::Mkdir { root, path, name } => {
                files.mkdir(parse_id(root, "Invalid root")?, path.clone(), name.clone())?;
                Ok(serde_json::Value::Null)
            }
            FileOperation::Rename {
                root,
                path,
                name,
                token,
                new_name,
            } => {
                files.rename(
                    parse_id(root, "Invalid root")?,
                    path.clone(),
                    name.clone(),
                    token.as_ref().map(crate::files::entry_token).transpose()?,
                    new_name.clone(),
                )?;
                Ok(serde_json::Value::Null)
            }
            FileOperation::Delete {
                root,
                path,
                name,
                token,
                trash,
            } => {
                files.delete(
                    parse_id(root, "Invalid root")?,
                    path.clone(),
                    name.clone(),
                    token.as_ref().map(crate::files::entry_token).transpose()?,
                    *trash,
                )?;
                Ok(serde_json::Value::Null)
            }
            FileOperation::Submit {
                direction,
                src_root,
                src_path,
                dst_root,
                dst_path,
            } => {
                let owned = self
                    .jobs
                    .lock()
                    .map_err(|_| Error::from_reason("Channel unavailable"))?
                    .clone();
                // Bound both retained history and simultaneous active transfers.
                if owned.len() >= 256
                    || owned
                        .iter()
                        .filter_map(|id| files.job_snapshot(*id).ok())
                        .filter(|job| {
                            !matches!(
                                job.state,
                                vnilink_app::FfiJobState::Cancelled
                                    | vnilink_app::FfiJobState::Failed
                                    | vnilink_app::FfiJobState::Complete
                            )
                        })
                        .count()
                        >= 8
                {
                    return Err(Refused("Busy: too many transfers at once".into()));
                }
                let src = parse_id(src_root, "Invalid root")?;
                let dst = parse_id(dst_root, "Invalid root")?;
                let job = match direction.as_str() {
                    "upload" => files.upload(src, src_path.clone(), dst, dst_path.clone()),
                    "download" => files.download(src, src_path.clone(), dst, dst_path.clone()),
                    _ => return Err(Refused("InvalidSetting: unknown direction".into())),
                }?;
                let mut jobs = self
                    .jobs
                    .lock()
                    .map_err(|_| Error::from_reason("Channel unavailable"))?;
                if self.closed.load(std::sync::atomic::Ordering::Acquire) {
                    drop(jobs);
                    let _ = self.core.job_cancel(job);
                    return Err(Refused("NoSession: the window closed".into()));
                }
                jobs.push(job);
                drop(jobs);
                Self::json(crate::transfer_job(&files.job_snapshot(job)?))
            }
            FileOperation::Jobs => Self::json(
                files
                    .jobs()?
                    .iter()
                    .map(crate::transfer_job)
                    .collect::<Vec<_>>(),
            ),
            FileOperation::Watch { folder } => {
                files.watch(
                    folder
                        .as_ref()
                        .map(|folder| {
                            Ok::<_, Refused>((
                                parse_id(&folder.root, "Invalid root")?,
                                folder.path.clone(),
                            ))
                        })
                        .transpose()?,
                );
                Ok(serde_json::Value::Null)
            }
            FileOperation::Adopt => {
                let adopted = files.resume_orphans()?;
                let mut jobs = self
                    .jobs
                    .lock()
                    .map_err(|_| Error::from_reason("Channel unavailable"))?;
                jobs.extend(adopted.iter().map(|job| job.id));
                drop(jobs);
                Self::json(adopted.iter().map(crate::transfer_job).collect::<Vec<_>>())
            }
            FileOperation::Cancel { job_id } => {
                Self::json(crate::transfer_job(&files.job_cancel(self.owned(job_id)?)?))
            }
            FileOperation::Resume { job_id } => {
                Self::json(crate::transfer_job(&files.job_resume(self.owned(job_id)?)?))
            }
            FileOperation::Resolve {
                job_id,
                choice,
                apply_to_remaining,
            } => {
                let choice = match choice.as_str() {
                    "skip" => vnilink_app::FfiConflictChoice::Skip,
                    "replace" => vnilink_app::FfiConflictChoice::Replace,
                    "keepBoth" => vnilink_app::FfiConflictChoice::KeepBoth,
                    _ => return Err(Refused("InvalidSetting: unknown choice".into())),
                };
                Self::json(crate::transfer_job(&files.job_resolve(
                    self.owned(job_id)?,
                    choice,
                    *apply_to_remaining,
                )?))
            }
            FileOperation::Forget { job_id } => {
                let job = self.owned(job_id)?;
                files.job_forget(job)?;
                self.jobs
                    .lock()
                    .map_err(|_| Error::from_reason("Channel unavailable"))?
                    .retain(|id| *id != job);
                Ok(serde_json::Value::Null)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};
    fn pair(from: &VnilinkCore, to: &VnilinkCore) {
        let file = from.export_pairing_file(60, true).unwrap();
        let preview = to.open_pairing_file(file.bytes).unwrap();
        to.confirm_pairing(preview.id, file.check).unwrap();
    }
    #[test]
    fn neutral_desktop_channels_share_owner_and_retire_independently() {
        use std::sync::atomic::Ordering;
        let controller = Arc::new(VnilinkCore::create_loopback_for_test().unwrap());
        let host = VnilinkCore::create_loopback_for_test().unwrap();
        let applied = host.install_test_desktop().unwrap();
        let caps = vnilink_app::FfiVideoCodecCaps {
            encode_baseline: true,
            encode_high: true,
            decode_baseline: true,
            decode_high: true,
            max_width: 4096,
            max_height: 4096,
            hardware: false,
            device: "test".into(),
            chroma444: false,
            additional: Vec::new(),
        };
        controller.set_video_codecs(caps.clone()).unwrap();
        host.set_video_codecs(caps).unwrap();
        pair(&host, &controller);
        pair(&controller, &host);
        host.set_desktop_sharing(true, true, false, false).unwrap();
        host.start_hosting().unwrap();
        let pending = controller
            .begin_connect_services(host.endpoint_id())
            .unwrap();
        let end = Instant::now() + Duration::from_secs(10);
        while controller.controller_session(pending.id).unwrap().phase
            != vnilink_app::FfiSessionPhase::Active
        {
            assert!(Instant::now() < end);
            std::thread::sleep(Duration::from_millis(5));
        }
        let session = controller.controller_session(pending.id).unwrap();
        let desktops = Arc::new(DesktopChannels::default());
        let mut open = OpenChannelTask {
            core: controller.clone(),
            desktops,
            cameras: Arc::new(CameraChannels::default()),
            id: session.id,
            generation: session.generation,
            kind: "video".into(),
            rows: 24,
            columns: 80,
            media: None,
        };
        host.set_service_sharing(vnilink_app::FfiServiceKind::Desktop, false)
            .unwrap();
        assert!(
            open.compute().is_err(),
            "a neutral connection must not bypass Desktop authorization"
        );
        host.set_service_sharing(vnilink_app::FfiServiceKind::Desktop, true)
            .unwrap();
        let video = open.compute().unwrap();
        assert!(
            open.compute().is_err(),
            "duplicate video subscribers must be rejected"
        );
        open.generation += 1;
        assert!(
            open.compute().is_err(),
            "a stale generation must be rejected"
        );
        open.generation = session.generation;
        open.kind = "input".into();
        let input = open.compute().unwrap();
        let status: serde_json::Value =
            serde_json::from_str(&video.desktop_audio(r#"{"type":"status"}"#.into()).unwrap())
                .unwrap();
        assert_eq!(status["enabled"], true);
        assert!(video
            .desktop_audio(r#"{"type":"status","extra":true}"#.into())
            .is_err());
        assert!(video
            .desktop_audio(r#"{"type":"muted","muted":1}"#.into())
            .is_err());
        input
            .desktop_audio(r#"{"type":"muted","muted":true}"#.into())
            .unwrap();
        let status: serde_json::Value =
            serde_json::from_str(&video.desktop_audio(r#"{"type":"status"}"#.into()).unwrap())
                .unwrap();
        assert_eq!(
            status["muted"], true,
            "video and input share one audio companion"
        );

        let owner = video
            .desktop
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .service
            .info();
        assert_eq!(
            input
                .desktop
                .lock()
                .unwrap()
                .as_ref()
                .unwrap()
                .service
                .info()
                .id,
            owner.id
        );
        let remote = host
            .hosted_services()
            .unwrap()
            .into_iter()
            .find(|s| s.kind == vnilink_app::FfiServiceKind::Desktop)
            .unwrap();
        host.publish_desktop_video(remote.id, remote.generation, true, 10, vec![0, 255, 7])
            .unwrap();
        let end = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(packet) = video.read().unwrap() {
                assert_eq!(packet.data.unwrap().as_ref(), &[0, 255, 7]);
                break;
            }
            assert!(Instant::now() < end);
            std::thread::sleep(Duration::from_millis(5));
        }
        let key = serde_json::json!({"type":"submitKey", "sessionId":session.id.to_string(), "generation":session.generation.to_string(), "usage":4, "down":true}).to_string();
        input.input(key.clone()).unwrap();
        let end = Instant::now() + Duration::from_secs(5);
        while applied.load(Ordering::Relaxed) < 1 {
            assert!(Instant::now() < end);
            std::thread::sleep(Duration::from_millis(5));
        }
        input.take_cleanup().compute().unwrap();
        assert!(input.input(key.clone()).is_err());
        assert!(input.desktop_audio(r#"{"type":"status"}"#.into()).is_err());
        assert_eq!(
            controller.controller_service(owner.id).unwrap().phase,
            vnilink_app::FfiServicePhase::Active
        );
        // Closing input releases pressed keys, while video retains the Desktop owner.
        let end = Instant::now() + Duration::from_secs(5);
        while applied.load(Ordering::Relaxed) < 2 {
            assert!(Instant::now() < end);
            std::thread::sleep(Duration::from_millis(5));
        }
        let replacement = open.compute().unwrap();
        video.take_cleanup().compute().unwrap();
        replacement.input(key).unwrap();
        replacement.take_cleanup().compute().unwrap();
        assert!(controller.controller_service(owner.id).is_err());
        assert_eq!(
            controller.controller_session(session.id).unwrap().phase,
            vnilink_app::FfiSessionPhase::Active
        );
        open.kind = "video".into();
        let reopened = open.compute().unwrap();
        assert_ne!(
            reopened
                .desktop
                .lock()
                .unwrap()
                .as_ref()
                .unwrap()
                .service
                .info()
                .id,
            owner.id
        );
        reopened.take_cleanup().compute().unwrap();
        assert!(controller.controller_services().unwrap().is_empty());
        controller.shutdown();
        host.shutdown();
    }

    #[cfg(unix)]
    #[test]
    fn terminal_bytes_use_owned_channel_and_stop_after_close() {
        let controller = Arc::new(VnilinkCore::create_loopback_for_test().unwrap());
        let host = VnilinkCore::create_loopback_for_test().unwrap();
        let root = std::env::temp_dir().join(format!(
            "vnilink-data-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let source = root.join("source");
        let destination = root.join("destination");
        std::fs::create_dir_all(&source).unwrap();
        std::fs::create_dir_all(&destination).unwrap();
        std::fs::write(source.join("test.bin"), [0, 255, 17, 42]).unwrap();
        controller
            .set_file_roots(vec![source.to_string_lossy().into()])
            .unwrap();
        host.set_file_roots(vec![destination.to_string_lossy().into()])
            .unwrap();
        host.set_service_sharing(vnilink_app::FfiServiceKind::Files, true)
            .unwrap();
        std::fs::create_dir_all(root.join("controller-blobs")).unwrap();
        std::fs::create_dir_all(root.join("host-blobs")).unwrap();
        controller
            .set_blob_directory(root.join("controller-blobs").to_string_lossy().into())
            .unwrap();
        host.set_blob_directory(root.join("host-blobs").to_string_lossy().into())
            .unwrap();
        pair(&host, &controller);
        pair(&controller, &host);
        host.set_desktop_sharing(true, true, false, false).unwrap();
        host.set_service_sharing(vnilink_app::FfiServiceKind::Terminal, true)
            .unwrap();
        host.set_service_sharing(vnilink_app::FfiServiceKind::Files, true)
            .unwrap();
        controller
            .set_service_sharing(vnilink_app::FfiServiceKind::Files, true)
            .unwrap();
        host.enable_native_terminal(
            "/bin/sh".into(),
            std::env::temp_dir().to_string_lossy().into(),
        )
        .unwrap();
        host.start_hosting().unwrap();
        let pending = controller
            .begin_connect_services(host.endpoint_id())
            .unwrap();
        let end = Instant::now() + Duration::from_secs(15);
        loop {
            if matches!(
                controller.controller_session(pending.id).unwrap().phase,
                vnilink_app::FfiSessionPhase::Active
            ) {
                break;
            }
            assert!(
                Instant::now() < end,
                "neutral connection did not become active: {:?}",
                controller.controller_session(pending.id).unwrap().phase
            );
            std::thread::sleep(Duration::from_millis(5));
        }
        let active = controller.controller_session(pending.id).unwrap();
        let mut task = OpenChannelTask {
            core: controller.clone(),
            desktops: Arc::new(DesktopChannels::default()),
            cameras: Arc::new(CameraChannels::default()),
            id: active.id,
            generation: active.generation,
            kind: "terminal".into(),
            rows: 24,
            columns: 80,
            media: None,
        };
        let channel = task.compute().unwrap();
        channel
            .write(
                b"printf '%s%s\\n' 'native-' 'binary-channel'\n"
                    .to_vec()
                    .into(),
            )
            .unwrap();
        let end = Instant::now() + Duration::from_secs(10);
        let mut output: Vec<u8> = Vec::new();
        loop {
            if let Some(packet) = channel.read().unwrap() {
                if let Some(bytes) = packet.data {
                    output.extend(bytes.as_ref());
                }
                if output
                    .windows(b"native-binary-channel".len())
                    .any(|part| part == b"native-binary-channel")
                {
                    break;
                }
            }
            assert!(Instant::now() < end, "terminal output did not arrive");
            std::thread::sleep(Duration::from_millis(5));
        }
        channel.write(b"exit 7\n".to_vec().into()).unwrap();
        let end = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(packet) = channel.read().unwrap() {
                if packet.kind == "exited" {
                    assert!(packet.metadata.contains('7'));
                    break;
                }
            }
            assert!(Instant::now() < end, "terminal exit did not arrive");
            std::thread::sleep(Duration::from_millis(5));
        }
        let mut file_open = OpenChannelTask {
            core: controller.clone(),
            desktops: Arc::new(DesktopChannels::default()),
            cameras: Arc::new(CameraChannels::default()),
            id: active.id,
            generation: active.generation,
            kind: "files".into(),
            rows: 24,
            columns: 80,
            media: None,
        };
        let files = file_open.compute().unwrap();
        let mut file_task = FileTask {
            core: controller.clone(),
            session: active.id,
            generation: active.generation,
            files: files.files.lock().unwrap().as_ref().unwrap().clone(),
            jobs: files.jobs.clone(),
            closed: files.closed.clone(),
            operation: FileOperation::Submit {
                direction: "upload".into(),
                src_root: controller.file_roots().unwrap()[0].id.to_string(),
                src_path: "test.bin".into(),
                dst_root: host.file_roots().unwrap()[0].id.to_string(),
                dst_path: "received.bin".into(),
            },
        };
        let answer = |task: &mut FileTask| -> serde_json::Value {
            serde_json::from_str(&task.compute().unwrap()).unwrap()
        };
        let progress = answer(&mut file_task)["ok"].clone();
        assert!(
            progress.get("data").is_none(),
            "File payload crossed the UI boundary"
        );
        let job_id = progress["id"].as_str().unwrap().to_owned();
        file_task.operation = FileOperation::Cancel {
            job_id: job_id.clone(),
        };
        assert_eq!(answer(&mut file_task)["ok"]["state"], "Cancelled");
        file_task.operation = FileOperation::Forget { job_id };
        assert_eq!(answer(&mut file_task)["ok"], serde_json::Value::Null);
        // The other computer's folders are listed, made, renamed and deleted
        // through the channel; a refusal is an answer, and the channel lives.
        let host_root = host.file_roots().unwrap()[0].id.to_string();
        file_task.operation = FileOperation::Places;
        let places = answer(&mut file_task)["ok"].clone();
        assert_eq!(places["roots"][0]["id"], host_root);
        for name in ["made", "taken"] {
            file_task.operation = FileOperation::Mkdir {
                root: host_root.clone(),
                path: "".into(),
                name: name.into(),
            };
            assert_eq!(answer(&mut file_task)["ok"], serde_json::Value::Null);
        }
        file_task.operation = FileOperation::List {
            root: host_root.clone(),
            path: "".into(),
            offset: "0".into(),
        };
        let page = answer(&mut file_task)["ok"].clone();
        let made = page["entries"]
            .as_array()
            .unwrap()
            .iter()
            .find(|entry| entry["name"] == "made")
            .unwrap()
            .clone();
        assert_eq!(made["kind"], "dir");
        let token: crate::files::TransferToken =
            serde_json::from_value(made["token"].clone()).unwrap();
        file_task.operation = FileOperation::Rename {
            root: host_root.clone(),
            path: "".into(),
            name: "made".into(),
            token: Some(token.clone()),
            new_name: "taken".into(),
        };
        let refused = answer(&mut file_task);
        assert!(
            refused["error"].as_str().unwrap().starts_with("FileExists"),
            "{refused}"
        );
        file_task.operation = FileOperation::Rename {
            root: host_root.clone(),
            path: "".into(),
            name: "made".into(),
            token: Some(token),
            new_name: "renamed".into(),
        };
        assert_eq!(answer(&mut file_task)["ok"], serde_json::Value::Null);
        assert!(destination.join("renamed").is_dir());
        file_task.operation = FileOperation::Delete {
            root: host_root.clone(),
            path: "".into(),
            name: "renamed".into(),
            token: None,
            trash: false,
        };
        assert_eq!(answer(&mut file_task)["ok"], serde_json::Value::Null);
        assert!(!destination.join("renamed").exists());
        // Both cores' drivers finish the transfer; the channel reports it
        // without being asked.
        controller.start_file_job_driver();
        host.start_file_job_driver();
        file_task.operation = FileOperation::Submit {
            direction: "upload".into(),
            src_root: controller.file_roots().unwrap()[0].id.to_string(),
            src_path: "test.bin".into(),
            dst_root: host_root.clone(),
            dst_path: "completed.bin".into(),
        };
        let submitted = answer(&mut file_task)["ok"].clone();
        let end = Instant::now() + Duration::from_secs(15);
        let mut states = Vec::new();
        loop {
            if let Some(event) = files.read().unwrap() {
                assert_eq!(event.kind, "jobs");
                let update: serde_json::Value = serde_json::from_str(&event.metadata).unwrap();
                for job in update["jobs"].as_array().unwrap() {
                    if job["id"] == submitted["id"] {
                        states.push(job["state"].as_str().unwrap().to_owned());
                    }
                }
                if states.last().is_some_and(|state| state == "Complete") {
                    break;
                }
            }
            assert!(
                Instant::now() < end,
                "the transfer's completion was never pushed: {states:?}"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(
            std::fs::read(destination.join("completed.bin/test.bin")).unwrap(),
            [0, 255, 17, 42]
        );
        files.take_cleanup().compute().unwrap();
        assert_eq!(
            controller
                .job_snapshot(submitted["id"].as_str().unwrap().parse().unwrap())
                .unwrap()
                .state,
            vnilink_app::FfiJobState::Complete
        );
        channel.take_cleanup().compute().unwrap();
        channel.take_cleanup().compute().unwrap();
        assert!(channel.read().is_err());
        assert!(channel.write(vec![1].into()).is_err());
        controller.shutdown();
        host.shutdown();
        std::fs::remove_dir_all(root).unwrap();
    }
}
