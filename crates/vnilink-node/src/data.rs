//! Session-scoped binary data plane. Never takes the JSON bridge's State mutex.
use super::{core_error, parse_id, Command, CommandShape, Engine, EngineBridge};
use napi::{
    bindgen_prelude::{AsyncTask, Buffer},
    Env, Error, Result, Task,
};
use napi_derive::napi;
use std::sync::{Arc, Mutex};
use vnilink_app::{FfiTerminalEvent, FfiTerminalSize, TerminalService, VideoPoller, VnilinkCore};

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
    video: Mutex<Option<VideoPoller>>,
    terminal: Mutex<Option<Arc<TerminalService>>>,
    closed: Arc<std::sync::atomic::AtomicBool>,
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
                metadata: serde_json::json!({"keyframe":frame.keyframe,"timestampUs":frame.timestamp_us.to_string(),"displayId":frame.display_id.to_string(),"revision":frame.revision.to_string()}).to_string(),
            }));
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
        EngineBridge::input(&self.core, &decoded)?;
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

    #[napi]
    pub fn close(&self) -> AsyncTask<CloseTask> {
        AsyncTask::new(self.take_cleanup())
    }
}
impl DataChannel {
    fn take_cleanup(&self) -> CloseTask {
        let already = self.closed.swap(true, std::sync::atomic::Ordering::AcqRel);
        if !already
            && self.kind == "input"
            && self
                .input_used
                .swap(false, std::sync::atomic::Ordering::AcqRel)
        {
            let _ = self
                .core
                .release_session_input(self.session_id, self.generation);
        }
        let jobs = if already {
            Vec::new()
        } else {
            self.jobs
                .lock()
                .map(|mut jobs| jobs.drain(..).collect())
                .unwrap_or_default()
        };
        if !already {
            if let Ok(mut terminal) = self.terminal.lock() {
                if let Some(terminal) = terminal.take() {
                    terminal.close();
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
            jobs,
            files,
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
    jobs: Vec<u64>,
    files: Option<Arc<vnilink_app::FilesService>>,
}
impl Task for CloseTask {
    type Output = ();
    type JsValue = ();
    fn compute(&mut self) -> Result<()> {
        for job in self.jobs.drain(..) {
            let _ = self.core.job_cancel(job);
        }
        if let Some((id, generation)) = self.owner.take() {
            let _ = self.core.close_controller_service(id, generation);
        }
        self.files.take();
        Ok(())
    }
    fn resolve(&mut self, _: Env, _: ()) -> Result<()> {
        Ok(())
    }
}

pub struct OpenChannelTask {
    core: Arc<VnilinkCore>,
    id: u64,
    generation: u64,
    kind: String,
    rows: u16,
    columns: u16,
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
        let video = if self.kind == "video" {
            Some(
                self.core
                    .subscribe_session_video(self.id, self.generation)
                    .map_err(core_error)?,
            )
        } else {
            None
        };
        let kind = match self.kind.as_str() {
            "terminal" => Some(vnilink_app::FfiServiceKind::Terminal),
            "files" => Some(vnilink_app::FfiServiceKind::Files),
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
                    let _ = self
                        .core
                        .close_controller_service(owner.id, owner.generation);
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
                    let _ = self
                        .core
                        .close_controller_service(owner.id, owner.generation);
                    return Err(core_error(error));
                }
            }
        } else {
            None
        };
        Ok(DataChannel {
            core: self.core.clone(),
            session_id: self.id,
            generation: self.generation,
            kind: self.kind.clone(),
            input_used: std::sync::atomic::AtomicBool::new(false),
            owner: owner.map(|owner| (owner.id, owner.generation)),
            files: Mutex::new(files),
            jobs: Arc::new(Mutex::new(Vec::new())),
            video: Mutex::new(video),
            terminal: Mutex::new(terminal),
            closed: Arc::new(std::sync::atomic::AtomicBool::new(false)),
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
    ) -> Result<AsyncTask<OpenChannelTask>> {
        if !matches!(kind.as_str(), "video" | "terminal" | "input" | "files") {
            return Err(Error::from_reason("Invalid channel kind"));
        }
        Ok(AsyncTask::new(OpenChannelTask {
            core: self.core.clone(),
            id: parse_id(&session_id, "Invalid session ID")?,
            generation: parse_id(&generation, "Invalid generation")?,
            kind,
            rows,
            columns,
        }))
    }
}

#[derive(serde::Deserialize)]
#[serde(tag = "type", rename_all = "camelCase", deny_unknown_fields)]
enum FileOperation {
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
    Poll {
        #[serde(rename = "jobId")]
        job_id: String,
    },
    Cancel {
        #[serde(rename = "jobId")]
        job_id: String,
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
        let snapshot = match &self.operation {
            FileOperation::Submit {
                direction,
                src_root,
                src_path,
                dst_root,
                dst_path,
            } => {
                if self
                    .jobs
                    .lock()
                    .map_err(|_| Error::from_reason("Channel unavailable"))?
                    .len()
                    >= 8
                {
                    return Err(Error::from_reason("File job limit"));
                }
                let src = parse_id(src_root, "Invalid root")?;
                let dst = parse_id(dst_root, "Invalid root")?;
                let job = match direction.as_str() {
                    "upload" => self
                        .files
                        .upload(src, src_path.clone(), dst, dst_path.clone()),
                    "download" => self
                        .files
                        .download(src, src_path.clone(), dst, dst_path.clone()),
                    _ => return Err(Error::from_reason("Invalid direction")),
                }
                .map_err(core_error)?;
                let mut jobs = self
                    .jobs
                    .lock()
                    .map_err(|_| Error::from_reason("Channel unavailable"))?;
                if self.closed.load(std::sync::atomic::Ordering::Acquire) {
                    drop(jobs);
                    let _ = self.core.job_cancel(job);
                    return Err(Error::from_reason("Channel closed"));
                }
                jobs.push(job);
                drop(jobs);
                self.files.job_snapshot(job).map_err(core_error)?
            }
            FileOperation::Poll { job_id } | FileOperation::Cancel { job_id } => {
                let job = parse_id(job_id, "Invalid job")?;
                if !self
                    .jobs
                    .lock()
                    .map_err(|_| Error::from_reason("Channel unavailable"))?
                    .contains(&job)
                {
                    return Err(Error::from_reason("Job belongs to another channel"));
                }
                match &self.operation {
                    FileOperation::Poll { .. } => self.files.job_poll(job),
                    _ => self.files.job_cancel(job),
                }
                .map_err(core_error)?
            }
        };
        let state = format!("{:?}", snapshot.state);
        if matches!(
            snapshot.state,
            vnilink_app::FfiJobState::Cancelled
                | vnilink_app::FfiJobState::Failed
                | vnilink_app::FfiJobState::Complete
        ) {
            self.jobs
                .lock()
                .map_err(|_| Error::from_reason("Channel unavailable"))?
                .retain(|id| *id != snapshot.id);
        }
        Ok(serde_json::json!({"jobId":snapshot.id.to_string(),"state":state,"doneBytes":snapshot.done_bytes.to_string(),"totalBytes":snapshot.total_bytes.to_string(),"filesDone":snapshot.files_done.to_string(),"filesTotal":snapshot.files_total.to_string()}).to_string())
    }
    fn resolve(&mut self, _: Env, value: String) -> Result<String> {
        Ok(value)
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
        host.authorize_service(
            controller.endpoint_id(),
            vnilink_app::FfiServiceKind::Files,
            true,
        )
        .unwrap_err();
        std::fs::create_dir_all(root.join("controller-blobs")).unwrap();
        std::fs::create_dir_all(root.join("host-blobs")).unwrap();
        controller
            .set_blob_directory(root.join("controller-blobs").to_string_lossy().into())
            .unwrap();
        host.set_blob_directory(root.join("host-blobs").to_string_lossy().into())
            .unwrap();
        pair(&host, &controller);
        pair(&controller, &host);
        host.authorize(controller.endpoint_id(), true, true, false, false)
            .unwrap();
        host.authorize_service(
            controller.endpoint_id(),
            vnilink_app::FfiServiceKind::Terminal,
            true,
        )
        .unwrap();
        host.authorize_service(
            controller.endpoint_id(),
            vnilink_app::FfiServiceKind::Files,
            true,
        )
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
            id: active.id,
            generation: active.generation,
            kind: "terminal".into(),
            rows: 24,
            columns: 80,
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
            id: active.id,
            generation: active.generation,
            kind: "files".into(),
            rows: 24,
            columns: 80,
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
        let progress: serde_json::Value =
            serde_json::from_str(&file_task.compute().unwrap()).unwrap();
        assert!(
            progress.get("data").is_none(),
            "File payload crossed the UI boundary"
        );
        file_task.operation = FileOperation::Cancel {
            job_id: progress["jobId"].as_str().unwrap().into(),
        };
        assert!(file_task.compute().unwrap().contains("Cancelled"));
        files.take_cleanup().compute().unwrap();
        channel.take_cleanup().compute().unwrap();
        channel.take_cleanup().compute().unwrap();
        assert!(channel.read().is_err());
        assert!(channel.write(vec![1].into()).is_err());
        controller.shutdown();
        host.shutdown();
        std::fs::remove_dir_all(root).unwrap();
    }
}
