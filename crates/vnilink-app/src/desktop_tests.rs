use crate::tests::filesystem_location::FilesystemLocation;
use crate::*;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Debug, PartialEq)]
enum Applied {
    Input(vnilink::InputEvent),
    Text(String),
}
struct Desktop(Arc<Mutex<Vec<Applied>>>);
impl vnilink::HostDesktop for Desktop {
    fn apply(&mut self, event: vnilink::InputEvent) -> Result<(), vnilink::Error> {
        self.0.lock().unwrap().push(Applied::Input(event));
        Ok(())
    }
    fn type_text(&mut self, text: &str) -> Result<(), vnilink::Error> {
        self.0.lock().unwrap().push(Applied::Text(text.into()));
        Ok(())
    }
    fn monitor(&self) -> (u16, u16) {
        (1920, 1080)
    }
}
struct Capture;
impl vnilink::HostCapture for Capture {
    fn monitor(&self) -> (u16, u16) {
        (1920, 1080)
    }
}
struct Pasteboard(Arc<Mutex<Option<String>>>);
impl ClipboardWriter for Pasteboard {
    fn set_text(&self, text: String) -> Result<(), FfiError> {
        *self.0.lock().unwrap() = Some(text);
        Ok(())
    }
}
fn core() -> VnilinkCore {
    VnilinkCore {
        inner: Arc::new(
            Core::start(CoreConfig {
                listen: Listen::Loopback,
                relay: RelaySettings::custom(vec![CustomRelay::new("http://127.0.0.1:9").unwrap()])
                    .unwrap(),
                ..CoreConfig::default()
            })
            .unwrap(),
        ),
        previews: Mutex::new(HashMap::new()),
        next_preview: AtomicU64::new(1),
        audio_coordinator: Arc::new(audio_native::AudioCoordinator::default()),
    }
}

fn full_codecs() -> FfiVideoCodecCaps {
    vnilink::VideoCodecCaps::h264_full(4096, 4096, "test").into()
}

fn empty_codecs() -> FfiVideoCodecCaps {
    vnilink::VideoCodecCaps::default().into()
}
fn pair(host: &VnilinkCore, controller: &VnilinkCore, clipboard: bool, transfer: bool) {
    host.set_video_codecs(full_codecs()).unwrap();
    controller.set_video_codecs(full_codecs()).unwrap();
    host.inner.set_capture(Box::new(Capture)).unwrap();
    controller
        .accept_invitation(host.create_invitation(60).unwrap())
        .unwrap();
    host.accept_invitation(controller.create_invitation(60).unwrap())
        .unwrap();
    host.set_desktop_sharing(true, true, clipboard, transfer)
        .unwrap();
    host.start_hosting().unwrap();
}

#[test]
fn service_handles_and_cancellation_cross_the_ffi_boundary() {
    let host = core();
    let controller = core();
    pair(&host, &controller, false, true);
    struct Scratch(std::path::PathBuf);
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    let scratch = Scratch(std::env::temp_dir().join(format!(
            "vnilink-app-service-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        )));
    std::fs::create_dir(&scratch.0).unwrap();
    host.inner.enable_filesystem_access().unwrap();
    let host_files = FilesystemLocation::new(host.inner.file_roots().unwrap(), &scratch.0);
    let session = controller.connect(host.endpoint_id()).unwrap();
    let events = controller.subscribe_events().unwrap();
    let desktop = controller
        .attach_controller_service(session.id, session.generation, FfiServiceKind::Desktop)
        .unwrap();
    let files = controller
        .attach_controller_service(session.id, session.generation, FfiServiceKind::Files)
        .unwrap();
    let old = controller
        .service_lease(desktop.id, desktop.generation)
        .unwrap();
    let kept = controller
        .service_lease(files.id, files.generation)
        .unwrap();
    assert!(
        controller
            .remote_service_support(session.id, session.generation)
            .unwrap()
            .files
    );
    let pipe = controller
        .open_files_service(files.id, files.generation)
        .unwrap();
    pipe.mkdir(host_files.root, host_files.path(""), "folder".into())
        .unwrap();
    let page = pipe
        .list(host_files.root, host_files.path(""), 0, 20)
        .unwrap();
    assert_eq!(page.entries.len(), 1);
    pipe.rename(
        host_files.root,
        host_files.path(""),
        "folder".into(),
        Some(page.entries[0].token),
        "renamed".into(),
    )
    .unwrap();
    assert!(scratch.0.join("renamed").is_dir());
    assert_eq!(host.hosted_services().unwrap().len(), 1);
    assert_eq!(old.info(), desktop);
    assert_eq!(
        controller
            .service_connection(files.id, files.generation)
            .unwrap(),
        session
    );
    assert!(matches!(
        controller.attach_controller_service(
            session.id,
            session.generation,
            FfiServiceKind::Terminal
        ),
        Err(FfiError::Unsupported { .. })
    ));
    assert!(matches!(
        controller.attach_controller_service(
            session.id,
            session.generation,
            FfiServiceKind::Camera
        ),
        Err(FfiError::Unsupported { .. })
    ));
    controller
        .close_controller_service(desktop.id, desktop.generation)
        .unwrap();
    assert!(old.is_cancelled());
    assert!(!kept.is_cancelled());
    assert_eq!(
        controller.controller_service(desktop.id).unwrap().phase,
        FfiServicePhase::Closed {
            cause: FfiCloseCause::OwnerClose,
        }
    );
    assert_eq!(controller.controller_session(session.id).unwrap(), session);
    let changed: Vec<_> = events
        .poll(64)
        .into_iter()
        .filter_map(|event| {
            if let FfiEvent::ServiceChanged { service } = event {
                Some(service)
            } else {
                None
            }
        })
        .collect();
    assert_eq!(changed.len(), 3);
    assert_eq!(changed[0], desktop);
    assert_eq!(changed[1], files);
    assert_eq!(
        changed[2].phase,
        FfiServicePhase::Closed {
            cause: FfiCloseCause::OwnerClose,
        }
    );
    pipe.close();
    let until = Instant::now() + Duration::from_secs(3);
    while !pipe.is_cancelled() || !host.hosted_services().unwrap().is_empty() {
        assert!(Instant::now() < until, "service close did not settle");
        std::thread::sleep(Duration::from_millis(10));
    }
    controller.forget_controller_service(desktop.id).unwrap();
    controller.shutdown();
    assert!(kept.is_cancelled());
}

#[test]
fn native_desktop_contract_covers_shortcuts_clipboard_and_bidirectional_files() {
    let host = core();
    let controller = core();
    let input = Arc::new(Mutex::new(Vec::new()));
    let pasteboard = Arc::new(Mutex::new(None));
    host.inner
        .set_desktop(Box::new(Desktop(input.clone())))
        .unwrap();
    host.install_clipboard(Box::new(Pasteboard(pasteboard.clone())))
        .unwrap();
    let root = std::env::temp_dir().join(format!(
        "vnilink-desktop-{}-{}",
        std::process::id(),
        host.endpoint_id()
    ));
    let host_dir = root.join("host");
    let controller_dir = root.join("controller");
    std::fs::create_dir_all(&host_dir).unwrap();
    std::fs::create_dir_all(&controller_dir).unwrap();
    host.inner
        .set_transfer_directory(root.join("host-blobs"))
        .unwrap();
    controller
        .inner
        .set_transfer_directory(root.join("controller-blobs"))
        .unwrap();
    host.inner.enable_filesystem_access().unwrap();
    let host_files = FilesystemLocation::new(host.inner.file_roots().unwrap(), &host_dir);
    controller.inner.enable_filesystem_access().unwrap();
    let controller_files =
        FilesystemLocation::new(controller.inner.file_roots().unwrap(), &controller_dir);
    pair(&host, &controller, true, true);
    controller
        .set_desktop_sharing(false, false, false, true)
        .unwrap();
    let session = controller.connect(host.endpoint_id()).unwrap();
    // All transitions follow the same path as the Mac native key adapter.
    for (usage, down, text) in [
        (227, true, ""),
        (6, true, "c"),
        (6, false, ""),
        (227, false, ""),
        (4, true, "é"),
        (4, false, ""),
    ] {
        controller
            .submit_local_key(
                session.id,
                session.generation,
                usage,
                down,
                text.into(),
                false,
            )
            .unwrap();
    }
    let expected = vec![
        Applied::Input(vnilink::InputEvent::key_down(227)),
        Applied::Input(vnilink::InputEvent::key_down(6)),
        Applied::Input(vnilink::InputEvent::key_up(6)),
        Applied::Input(vnilink::InputEvent::key_up(227)),
        Applied::Text("é".into()),
    ];
    let deadline = Instant::now() + Duration::from_secs(5);
    while *input.lock().unwrap() != expected {
        assert!(
            Instant::now() < deadline,
            "native shortcut/text sequence was not applied"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    controller
        .submit_clipboard(
            session.id,
            session.generation,
            "clipboard fixture 📝".into(),
        )
        .unwrap();
    assert_eq!(
        *pasteboard.lock().unwrap(),
        Some("clipboard fixture 📝".into())
    );
    let incoming = host.snapshot().unwrap().incoming.unwrap();
    host.publish_clipboard(incoming.id, "return fixture".into())
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while controller.clipboard_text().unwrap().as_deref() != Some("return fixture") {
        assert!(
            Instant::now() < deadline,
            "host clipboard publication was not received"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(
        controller.clipboard_text().unwrap(),
        Some("return fixture".into())
    );
    std::fs::write(controller_dir.join("sent.bin"), [0, 255, 10]).unwrap();
    std::fs::write(host_dir.join("returned.bin"), [4, 2]).unwrap();
    let up = controller
        .job_submit_upload(
            host.endpoint_id(),
            controller_files.root,
            controller_files.path("sent.bin"),
            host_files.root,
            host_files.path(""),
        )
        .unwrap();
    let down = controller
        .job_submit_download(
            host.endpoint_id(),
            host_files.root,
            host_files.path("returned.bin"),
            controller_files.root,
            controller_files.path(""),
        )
        .unwrap();
    let host_jobs: Vec<u64> = host
        .inner
        .job_list()
        .unwrap()
        .iter()
        .map(|job| job.id)
        .collect();
    assert_eq!(host_jobs.len(), 2);
    let deadline = Instant::now() + Duration::from_secs(10);
    while controller.inner.job_snapshot(up).unwrap().state != vnilink::JobState::Complete
        || controller.inner.job_snapshot(down).unwrap().state != vnilink::JobState::Complete
    {
        assert!(Instant::now() < deadline, "ffi jobs did not complete");
        for id in &host_jobs {
            host.inner.job_poll(*id).unwrap();
        }
        controller.inner.job_poll(up).unwrap();
        controller.inner.job_poll(down).unwrap();
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(
        std::fs::read(host_dir.join("sent.bin")).unwrap(),
        [0, 255, 10]
    );
    assert_eq!(
        std::fs::read(controller_dir.join("returned.bin")).unwrap(),
        [4, 2]
    );
    controller.disconnect().unwrap();
    controller.shutdown();
    host.shutdown();
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn native_clipboard_and_file_delivery_still_require_their_own_grants() {
    let host = core();
    let controller = core();
    let pasteboard = Arc::new(Mutex::new(None));
    host.inner
        .set_desktop(Box::new(Desktop(Arc::new(Mutex::new(Vec::new())))))
        .unwrap();
    host.install_clipboard(Box::new(Pasteboard(pasteboard.clone())))
        .unwrap();
    pair(&host, &controller, false, false);
    let session = controller.connect(host.endpoint_id()).unwrap();
    assert_eq!(
        controller.submit_clipboard(session.id, session.generation, "denied".into()),
        Err(FfiError::ClipboardDenied)
    );
    assert_eq!(*pasteboard.lock().unwrap(), None);
    assert_eq!(
        controller.fs_list(session.id, session.generation, 0, "".into(), 0, 20),
        Err(FfiError::TransferDenied)
    );
    controller.disconnect().unwrap();
    controller.shutdown();
    host.shutdown();
}

#[test]
fn service_jobs_cross_the_ffi_boundary() {
    let host = core();
    let controller = core();
    struct Scratch(std::path::PathBuf);
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    let scratch = Scratch(std::env::temp_dir().join(format!(
            "vnilink-app-owned-jobs-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        )));
    let host_root = scratch.0.join("host-root");
    let controller_root = scratch.0.join("controller-root");
    std::fs::create_dir_all(&host_root).unwrap();
    std::fs::create_dir_all(&controller_root).unwrap();
    host.inner
        .set_transfer_directory(scratch.0.join("host-blobs"))
        .unwrap();
    controller
        .inner
        .set_transfer_directory(scratch.0.join("controller-blobs"))
        .unwrap();
    host.inner.enable_filesystem_access().unwrap();
    let host_files = FilesystemLocation::new(host.inner.file_roots().unwrap(), &host_root);
    controller.inner.enable_filesystem_access().unwrap();
    let controller_files =
        FilesystemLocation::new(controller.inner.file_roots().unwrap(), &controller_root);
    pair(&host, &controller, false, true);
    controller
        .set_desktop_sharing(false, false, false, true)
        .unwrap();
    let session = controller.connect(host.endpoint_id()).unwrap();
    let owner = controller
        .attach_controller_service(session.id, session.generation, FfiServiceKind::Files)
        .unwrap();
    let pipe = controller
        .open_files_service(owner.id, owner.generation)
        .unwrap();
    std::fs::write(controller_root.join("up.txt"), b"ffi upload").unwrap();
    std::fs::write(host_root.join("down.txt"), b"ffi download").unwrap();
    let up = controller
        .files_service_upload(
            owner.id,
            owner.generation,
            controller_files.root,
            controller_files.path(""),
            vec!["up.txt".to_owned()],
            host_files.root,
            host_files.path("uploaded"),
        )
        .unwrap();
    let up_rx = host.inner.job_list().unwrap()[0].id;
    for _ in 0..10 {
        host.inner.job_poll(up_rx).unwrap();
        controller.inner.job_poll(up).unwrap();
    }
    assert_eq!(
        controller.inner.job_snapshot(up).unwrap().state,
        vnilink::JobState::Complete
    );
    assert_eq!(
        std::fs::read(host_root.join("uploaded/up.txt")).unwrap(),
        b"ffi upload"
    );
    let down = controller
        .files_service_download(
            owner.id,
            owner.generation,
            host_files.root,
            host_files.path(""),
            vec!["down.txt".to_owned()],
            controller_files.root,
            controller_files.path("cancelled"),
        )
        .unwrap();
    assert_eq!(
        controller
            .files_service_jobs(owner.id, owner.generation)
            .unwrap()
            .len(),
        2
    );
    pipe.close();
    let until = Instant::now() + Duration::from_secs(5);
    while controller.inner.job_snapshot(down).unwrap().state != vnilink::JobState::Interrupted {
        assert!(Instant::now() < until, "owned FFI job was not interrupted");
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(!controller_root.join("cancelled").exists());
    assert!(controller
        .files_service_download(
            owner.id,
            owner.generation,
            host_files.root,
            host_files.path(""),
            vec!["down.txt".to_owned()],
            controller_files.root,
            controller_files.path("later")
        )
        .is_err());
    assert_eq!(
        controller.controller_session(session.id).unwrap().phase,
        FfiSessionPhase::Active
    );
}

#[test]
fn service_permissions_cross_ffi_without_enabling_missing_backends() {
    let host = core();
    let controller = core();
    pair(&host, &controller, false, true);
    let id = controller.endpoint_id();
    assert_eq!(
        host.service_permissions(id.clone()).unwrap(),
        FfiServicePermissions {
            desktop: true,
            files: true,
            terminal: false,
            camera: false,
            system_audio: false,
            microphone: false,
            recording: false,
            system_actions: false
        }
    );
    host.set_service_sharing(FfiServiceKind::Terminal, true)
        .unwrap();
    host.set_service_sharing(FfiServiceKind::Camera, true)
        .unwrap();
    host.set_service_sharing(FfiServiceKind::Files, false)
        .unwrap();
    assert_eq!(
        host.service_permissions(id.clone()).unwrap(),
        FfiServicePermissions {
            desktop: true,
            files: false,
            terminal: true,
            camera: true,
            system_audio: false,
            microphone: false,
            recording: false,
            system_actions: false
        }
    );
    // Legacy native authorization only changes its three legacy fields.
    host.set_desktop_sharing(true, true, false, true).unwrap();
    assert!(host.service_permissions(id.clone()).unwrap().terminal);
    let session = controller.connect(host.endpoint_id()).unwrap();
    let support = controller
        .remote_service_support(session.id, session.generation)
        .unwrap();
    assert!(!support.terminal && !support.camera);
    assert!(matches!(
        controller.attach_controller_service(
            session.id,
            session.generation,
            FfiServiceKind::Terminal
        ),
        Err(FfiError::Unsupported { .. })
    ));
    host.unpair(id.clone()).unwrap();
    assert!(matches!(
        host.service_permissions(id),
        Err(FfiError::NotPaired)
    ));
    assert!(host.sharing_policy().unwrap().terminal);
}

#[test]
fn neutral_connections_cross_ffi_without_desktop_or_transfer_grants() {
    let host = core();
    let controller = core();
    pair(&host, &controller, false, false);
    host.set_desktop_sharing(false, false, false, false)
        .unwrap();
    host.set_video_codecs(empty_codecs()).unwrap();
    controller.set_video_codecs(empty_codecs()).unwrap();
    let pending = controller
        .begin_connect_services(host.endpoint_id())
        .unwrap();
    let until = Instant::now() + Duration::from_secs(5);
    loop {
        let session = controller.controller_session(pending.id).unwrap();
        if session.phase == FfiSessionPhase::Active {
            assert!(session.quality.is_none());
            break;
        }
        assert!(
            Instant::now() < until,
            "neutral FFI connection did not activate: {:?}",
            session.phase
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    let session = controller.controller_session(pending.id).unwrap();
    assert!(
        controller
            .remote_service_support(session.id, session.generation)
            .unwrap()
            .files
    );
    let owner = controller
        .attach_controller_service(session.id, session.generation, FfiServiceKind::Files)
        .unwrap();
    assert!(matches!(
        controller.open_files_service(owner.id, owner.generation),
        Err(FfiError::TransferDenied)
    ));
    host.set_service_sharing(FfiServiceKind::Files, true)
        .unwrap();
    let pipe = controller
        .open_files_service(owner.id, owner.generation)
        .unwrap();
    pipe.close();
    assert_eq!(
        controller.controller_session(session.id).unwrap().phase,
        FfiSessionPhase::Active
    );
}

#[test]
fn adopted_desktop_lifetime_crosses_ffi_without_closing_connection() {
    let host = core();
    let controller = core();
    pair(&host, &controller, false, true);
    let session = controller.connect(host.endpoint_id()).unwrap();
    let owner = controller
        .attach_controller_service(session.id, session.generation, FfiServiceKind::Desktop)
        .unwrap();
    let handle = controller
        .open_desktop_service(owner.id, owner.generation)
        .unwrap();
    assert_eq!(handle.info(), owner);
    let files = controller
        .attach_controller_service(session.id, session.generation, FfiServiceKind::Files)
        .unwrap();
    let pipe = controller
        .open_files_service(files.id, files.generation)
        .unwrap();
    handle.close();
    assert!(handle.is_cancelled());
    assert!(controller
        .controller_session(session.id)
        .unwrap()
        .quality
        .is_none());
    assert_eq!(
        controller.controller_service(owner.id).unwrap().phase,
        FfiServicePhase::Closed {
            cause: FfiCloseCause::OwnerClose,
        }
    );
    assert!(!pipe.is_cancelled());
    assert_eq!(
        controller.controller_session(session.id).unwrap().phase,
        FfiSessionPhase::Active
    );
    assert!(matches!(
        controller.open_desktop_service(files.id, files.generation),
        Err(FfiError::InvalidSetting { .. })
    ));
}

#[test]
fn recording_ffi_checks_installation_and_consent_before_creating_files() {
    let host = core();
    let controller = core();
    pair(&host, &controller, false, false);
    let session = controller
        .begin_connect_services(host.endpoint_id())
        .unwrap();
    let until = Instant::now() + Duration::from_secs(5);
    while controller.controller_session(session.id).unwrap().phase != FfiSessionPhase::Active {
        assert!(Instant::now() < until);
        std::thread::sleep(Duration::from_millis(2));
    }
    let session = controller.controller_session(session.id).unwrap();
    let owner = controller
        .attach_controller_service(session.id, session.generation, FfiServiceKind::Desktop)
        .unwrap();
    let desktop = controller
        .open_desktop_service(owner.id, owner.generation)
        .unwrap();
    let folder = std::env::temp_dir().join(format!(
        "vnilink-app-recording-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir(&folder).unwrap();
    let path = folder.join("test.mkv").to_string_lossy().into_owned();
    let limits = FfiRecordingLimits {
        max_bytes: 8 * 1024 * 1024,
        max_duration_ms: 10_000,
    };
    let denied = controller.start_desktop_recording(desktop.clone(), path.clone(), limits, None);
    assert_eq!(denied.err(), Some(FfiError::Unauthorized));
    assert_eq!(std::fs::read_dir(&folder).unwrap().count(), 0);
    host.set_recording_sharing(true).unwrap();
    let until = Instant::now() + Duration::from_secs(5);
    while !desktop.inner.recording_allowed().unwrap() {
        assert!(Instant::now() < until);
        std::thread::sleep(Duration::from_millis(2));
    }
    assert!(matches!(
        host.start_desktop_recording(desktop.clone(), path.clone(), limits, None),
        Err(FfiError::Unauthorized)
    ));
    assert_eq!(std::fs::read_dir(&folder).unwrap().count(), 0);
    assert!(matches!(
        controller.start_desktop_recording(
            desktop.clone(),
            path.clone(),
            limits,
            Some(FfiRecordingRetention {
                max_segments: 0,
                max_total_bytes: 1024 * 1024,
            }),
        ),
        Err(FfiError::InvalidSetting { .. })
    ));
    let recording = controller
        .start_desktop_recording(desktop.clone(), path, limits, None)
        .unwrap();
    assert_eq!(recording.status().phase, FfiRecordingPhase::Starting);
    host.set_recording_sharing(false).unwrap();
    let until = Instant::now() + Duration::from_secs(5);
    while recording.status().phase != FfiRecordingPhase::Failed {
        assert!(Instant::now() < until);
        std::thread::sleep(Duration::from_millis(2));
    }
    assert_eq!(
        recording.status().failure,
        Some(FfiRecordingFailure::Startup)
    );
    assert!(!desktop.is_cancelled());
    assert_eq!(
        controller.controller_session(session.id).unwrap().phase,
        FfiSessionPhase::Active
    );
    desktop.close();
    controller.shutdown();
    host.shutdown();
    std::fs::remove_dir_all(folder).unwrap();
}

#[test]
fn neutral_desktop_input_video_and_reopen_cross_ffi() {
    let host = core();
    let controller = core();
    let applied = Arc::new(Mutex::new(Vec::new()));
    host.inner
        .set_desktop(Box::new(Desktop(applied.clone())))
        .unwrap();
    pair(&host, &controller, false, false);
    let pending = controller
        .begin_connect_services(host.endpoint_id())
        .unwrap();
    let until = Instant::now() + Duration::from_secs(5);
    while controller.controller_session(pending.id).unwrap().phase != FfiSessionPhase::Active {
        assert!(Instant::now() < until);
        std::thread::sleep(Duration::from_millis(10));
    }
    let parent = controller.controller_session(pending.id).unwrap();
    let owner = controller
        .attach_controller_service(parent.id, parent.generation, FfiServiceKind::Desktop)
        .unwrap();
    let first = controller
        .open_desktop_service(owner.id, owner.generation)
        .unwrap();
    assert_eq!(first.desktop_info().unwrap().monitor_width, 1920);
    let remote = host
        .hosted_services()
        .unwrap()
        .into_iter()
        .find(|s| s.kind == FfiServiceKind::Desktop)
        .unwrap();
    let video = first.subscribe_video().unwrap();
    host.publish_desktop_video(remote.id, remote.generation, true, 10, vec![7, 8])
        .unwrap();
    assert_eq!(
        video.recv_timeout(5000).unwrap().unwrap().payload,
        vec![7, 8]
    );
    first.submit_key(4, true).unwrap();
    first.submit_text("hello".into()).unwrap();
    let until = Instant::now() + Duration::from_secs(5);
    while applied.lock().unwrap().len() != 2 {
        assert!(Instant::now() < until);
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(
        *applied.lock().unwrap(),
        vec![
            Applied::Input(vnilink::InputEvent::key_down(4)),
            Applied::Text("hello".into())
        ]
    );
    assert_eq!(
        first.control_state().unwrap(),
        FfiDesktopControlState::Controlling
    );
    first.yield_control().unwrap();
    let until = Instant::now() + Duration::from_secs(5);
    while first.control_state().unwrap() != FfiDesktopControlState::Viewing {
        assert!(Instant::now() < until);
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(
        applied.lock().unwrap().last(),
        Some(&Applied::Input(vnilink::InputEvent::key_up(4)))
    );
    assert!(!first.is_cancelled());
    first.acquire_control().unwrap();
    let until = Instant::now() + Duration::from_secs(5);
    while first.control_state().unwrap() != FfiDesktopControlState::Controlling {
        assert!(Instant::now() < until);
        std::thread::sleep(Duration::from_millis(10));
    }
    first.close();
    let next = controller
        .attach_controller_service(parent.id, parent.generation, FfiServiceKind::Desktop)
        .unwrap();
    let second = controller
        .open_desktop_service(next.id, next.generation)
        .unwrap();
    first.close();
    assert!(!second.is_cancelled());
    assert!(first.submit_pointer(20, 30, 1, 1).is_err());
    // Local close is immediate; the host observes its stream retirement
    // asynchronously. Reopening another stream does not order that delivery.
    let until = Instant::now() + Duration::from_secs(5);
    while host
        .hosted_services()
        .unwrap()
        .iter()
        .any(|service| service.id == remote.id && service.phase == FfiServicePhase::Active)
    {
        assert!(
            Instant::now() < until,
            "closed Desktop owner remained active on the host"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(host
        .publish_desktop_video(remote.id, remote.generation, true, 11, vec![9])
        .is_err());
    assert_eq!(
        controller.controller_session(parent.id).unwrap().phase,
        FfiSessionPhase::Active
    );
    assert!(controller
        .controller_session(parent.id)
        .unwrap()
        .quality
        .is_none());
}

#[test]
fn desktop_keyboard_settings_and_character_planning_cross_ffi() {
    let host = core();
    let controller = core();
    let applied = Arc::new(Mutex::new(Vec::new()));
    host.inner
        .set_desktop(Box::new(Desktop(applied.clone())))
        .unwrap();
    pair(&host, &controller, false, false);
    let pending = controller
        .begin_connect_services(host.endpoint_id())
        .unwrap();
    let until = Instant::now() + Duration::from_secs(5);
    while controller.controller_session(pending.id).unwrap().phase != FfiSessionPhase::Active {
        assert!(Instant::now() < until);
        std::thread::sleep(Duration::from_millis(10));
    }
    let parent = controller.controller_session(pending.id).unwrap();
    let owner = controller
        .attach_controller_service(parent.id, parent.generation, FfiServiceKind::Desktop)
        .unwrap();
    let desktop = controller
        .open_desktop_service(owner.id, owner.generation)
        .unwrap();
    assert_eq!(desktop.keyboard_mode().unwrap(), FfiKeyboardMode::Character);
    desktop
        .submit_local_key(vnilink::usage::GRAVE, true, "@".into(), false)
        .unwrap();
    assert!(desktop.set_keyboard_mode(FfiKeyboardMode::Map).is_err());
    let until = Instant::now() + Duration::from_secs(5);
    while applied.lock().unwrap().len() != 1 {
        assert!(Instant::now() < until);
        std::thread::sleep(Duration::from_millis(10));
    }
    desktop
        .submit_local_key(vnilink::usage::GRAVE, false, "".into(), false)
        .unwrap();
    desktop.set_keyboard_mode(FfiKeyboardMode::Map).unwrap();
    desktop.set_swap_control_and_command(true).unwrap();
    assert!(desktop.swap_control_and_command().unwrap());
    desktop.submit_key(vnilink::usage::LEFT_META, true).unwrap();
    assert!(desktop.set_swap_control_and_command(false).is_err());
    let until = Instant::now() + Duration::from_secs(5);
    while applied.lock().unwrap().len() != 2 {
        assert!(Instant::now() < until);
        std::thread::sleep(Duration::from_millis(10));
    }
    desktop.release_input().unwrap();
    desktop.set_swap_control_and_command(false).unwrap();
    let until = Instant::now() + Duration::from_secs(5);
    while applied.lock().unwrap().len() != 3 {
        assert!(Instant::now() < until);
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(
        *applied.lock().unwrap(),
        vec![
            Applied::Text("@".into()),
            Applied::Input(vnilink::InputEvent::key_down(vnilink::usage::LEFT_CONTROL)),
            Applied::Input(vnilink::InputEvent::key_up(vnilink::usage::LEFT_CONTROL))
        ]
    );
    desktop.close();
    assert!(desktop.keyboard_mode().is_err());
    assert!(desktop
        .submit_local_key(4, true, "a".into(), false)
        .is_err());
}

#[test]
fn desktop_clipboard_ack_and_owner_event_cross_ffi() {
    let host = core();
    let controller = core();
    let board = Arc::new(Mutex::new(None));
    host.install_clipboard(Box::new(Pasteboard(board.clone())))
        .unwrap();
    pair(&host, &controller, true, false);
    let pending = controller
        .begin_connect_services(host.endpoint_id())
        .unwrap();
    let until = Instant::now() + Duration::from_secs(5);
    while controller.controller_session(pending.id).unwrap().phase != FfiSessionPhase::Active {
        assert!(Instant::now() < until);
        std::thread::sleep(Duration::from_millis(10));
    }
    let parent = controller.controller_session(pending.id).unwrap();
    let owner = controller
        .attach_controller_service(parent.id, parent.generation, FfiServiceKind::Desktop)
        .unwrap();
    let desktop = controller
        .open_desktop_service(owner.id, owner.generation)
        .unwrap();
    desktop.submit_clipboard("from controller".into()).unwrap();
    assert_eq!(board.lock().unwrap().as_deref(), Some("from controller"));
    let events = controller.subscribe_events().unwrap();
    let remote = host
        .hosted_services()
        .unwrap()
        .into_iter()
        .find(|s| s.kind == FfiServiceKind::Desktop)
        .unwrap();
    host.publish_desktop_clipboard(remote.id, remote.generation, "from host".into())
        .unwrap();
    assert_eq!(
        desktop.clipboard_text().unwrap().as_deref(),
        Some("from host")
    );
    assert!(events.poll(32).into_iter().any(
        |event| matches!(event, FfiEvent::DesktopClipboardReceived { service } if service == owner)
    ));
    host.set_desktop_sharing(true, true, false, false).unwrap();
    assert!(matches!(
        desktop.submit_clipboard("denied".into()),
        Err(FfiError::ClipboardDenied)
    ));
    assert!(!desktop.is_cancelled());
    desktop.close();
    assert!(desktop.clipboard_text().is_err());
    assert!(host
        .publish_desktop_clipboard(remote.id, remote.generation, "late".into())
        .is_err());
    assert_eq!(
        controller.controller_session(parent.id).unwrap().phase,
        FfiSessionPhase::Active
    );
}

#[test]
fn files_handle_owns_submission_queue_and_cleanup() {
    let host = core();
    let controller = core();
    pair(&host, &controller, false, true);
    controller
        .set_desktop_sharing(false, false, false, true)
        .unwrap();
    let directory = std::env::temp_dir().join(format!(
        "vnilink-app-owned-jobs-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    struct Cleanup(std::path::PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    let _cleanup = Cleanup(directory.clone());
    for (core, name) in [(&host, "host"), (&controller, "controller")] {
        std::fs::create_dir_all(directory.join(name).join("root")).unwrap();
        core.inner.enable_filesystem_access().unwrap();
        core.inner
            .set_transfer_directory(directory.join(name).join("blobs"))
            .unwrap();
    }
    let host_files = FilesystemLocation::new(
        host.inner.file_roots().unwrap(),
        &directory.join("host").join("root"),
    );
    let controller_files = FilesystemLocation::new(
        controller.inner.file_roots().unwrap(),
        &directory.join("controller").join("root"),
    );
    std::fs::write(
        directory.join("controller/root/source.txt"),
        b"scoped contents",
    )
    .unwrap();
    let parent = controller
        .begin_connect_services(host.endpoint_id())
        .unwrap();
    let until = Instant::now() + Duration::from_secs(5);
    while controller.controller_session(parent.id).unwrap().phase != FfiSessionPhase::Active {
        assert!(Instant::now() < until);
        std::thread::sleep(Duration::from_millis(10));
    }
    let parent = controller.controller_session(parent.id).unwrap();
    let info = controller
        .attach_controller_service(parent.id, parent.generation, FfiServiceKind::Files)
        .unwrap();
    let files = controller
        .open_files_service(info.id, info.generation)
        .unwrap();
    let job = files
        .upload(
            controller_files.root,
            controller_files.path(""),
            vec!["source.txt".to_owned()],
            host_files.root,
            host_files.path("upload"),
        )
        .unwrap();
    assert_eq!(
        files
            .jobs()
            .unwrap()
            .iter()
            .map(|j| j.id)
            .collect::<Vec<_>>(),
        vec![job]
    );
    assert_eq!(files.job_snapshot(job).unwrap().state, FfiJobState::Serving);
    assert_eq!(files.job_poll(job).unwrap().id, job);
    files.close();
    let until = Instant::now() + Duration::from_secs(5);
    while files.job_snapshot(job).unwrap().state != FfiJobState::Interrupted {
        assert!(Instant::now() < until);
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(files
        .upload(
            controller_files.root,
            controller_files.path(""),
            vec!["source.txt".to_owned()],
            host_files.root,
            host_files.path("refused")
        )
        .is_err());
    assert!(files.job_resume(job).is_err());
    assert_eq!(files.job_cancel(job).unwrap().state, FfiJobState::Cancelled);
    let replacement = controller
        .attach_controller_service(parent.id, parent.generation, FfiServiceKind::Files)
        .unwrap();
    let replacement = controller
        .open_files_service(replacement.id, replacement.generation)
        .unwrap();
    assert!(replacement.jobs().unwrap().is_empty());
    assert!(replacement.job_snapshot(job).is_err());
    assert!(replacement.job_cancel(job).is_err());
    assert!(replacement.job_forget(job).is_err());
    files.job_forget(job).unwrap();
    assert!(files.jobs().unwrap().is_empty());
    std::fs::write(directory.join("host/root/return.txt"), b"return contents").unwrap();
    let download = replacement
        .download(
            host_files.root,
            host_files.path(""),
            vec!["return.txt".to_owned()],
            controller_files.root,
            controller_files.path("download"),
        )
        .unwrap();
    let until = Instant::now() + Duration::from_secs(8);
    while replacement.job_snapshot(download).unwrap().state != FfiJobState::Complete {
        assert!(
            Instant::now() < until,
            "download did not complete: {:?}",
            replacement.job_snapshot(download).unwrap()
        );
        host.inner.drive_jobs().unwrap();
        replacement.job_poll(download).unwrap();
    }
    assert_eq!(
        std::fs::read(directory.join("controller/root/download/return.txt")).unwrap(),
        b"return contents"
    );
    replacement.job_forget(download).unwrap();
    assert_eq!(
        controller.controller_session(parent.id).unwrap().phase,
        FfiSessionPhase::Active
    );
    controller.inner.shutdown();
    host.inner.shutdown();
}

#[cfg(unix)]
#[test]
fn terminal_ffi_round_trip_resize_exit_and_close() {
    let host = core();
    let controller = core();
    pair(&host, &controller, false, false);
    host.enable_native_terminal(
        "/bin/sh".into(),
        std::env::temp_dir().to_string_lossy().into_owned(),
    )
    .unwrap();
    host.set_service_sharing(FfiServiceKind::Terminal, true)
        .unwrap();
    let pending = controller
        .begin_connect_services(host.endpoint_id())
        .unwrap();
    let end = Instant::now() + Duration::from_secs(8);
    let session = loop {
        let session = controller.controller_session(pending.id).unwrap();
        if session.phase == FfiSessionPhase::Active {
            break session;
        }
        assert!(Instant::now() < end);
        std::thread::sleep(Duration::from_millis(5));
    };
    let owner = controller
        .attach_controller_service(session.id, session.generation, FfiServiceKind::Terminal)
        .unwrap();
    let terminal = controller
        .open_terminal_service(
            owner.id,
            owner.generation,
            FfiTerminalSize {
                rows: 24,
                columns: 80,
            },
        )
        .unwrap();
    terminal
        .resize(FfiTerminalSize {
            rows: 31,
            columns: 95,
        })
        .unwrap();
    terminal.input(b"stty size; exit 9\n".to_vec()).unwrap();
    let mut output = Vec::new();
    let mut code = None;
    while code.is_none() {
        assert!(Instant::now() < end);
        while let Some(event) = terminal.poll() {
            match event {
                FfiTerminalEvent::Output { bytes } => output.extend(bytes),
                FfiTerminalEvent::Exited { code: value } => code = Some(value),
                FfiTerminalEvent::Failed { message } => panic!("{message}"),
                FfiTerminalEvent::OutputGap { lost_bytes } => {
                    panic!("unexpected output gap {lost_bytes}")
                }
            }
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(code, Some(9));
    assert!(String::from_utf8(output).unwrap().contains("31 95"));
    terminal.close();
    host.disable_terminal().unwrap();
    assert_eq!(
        controller.controller_session(session.id).unwrap().phase,
        FfiSessionPhase::Active
    );
}

#[test]
fn native_camera_callback_and_frame_queue_cross_ffi() {
    struct Provider(Arc<Mutex<Option<Arc<CameraSource>>>>);
    impl CameraProvider for Provider {
        fn devices(&self) -> Result<Vec<FfiCameraDevice>, FfiError> {
            Ok(vec![FfiCameraDevice {
                id: "test-camera".into(),
                name: "Test camera".into(),
            }])
        }
        fn start_capture(
            &self,
            requested: FfiCameraConfig,
            startup: Arc<BackendStartup>,
        ) -> Result<Arc<CameraSource>, FfiError> {
            startup.check()?;
            let source = CameraSource::new(requested)?;
            *self.0.lock().unwrap() = Some(source.clone());
            Ok(source)
        }
    }
    let host = core();
    let controller = core();
    pair(&host, &controller, false, true);
    let produced = Arc::new(Mutex::new(None));
    host.install_camera_provider(Box::new(Provider(produced.clone())))
        .unwrap();
    host.set_service_sharing(FfiServiceKind::Camera, true)
        .unwrap();
    let connection = controller
        .begin_connect_services(host.endpoint_id())
        .unwrap();
    let until = Instant::now() + Duration::from_secs(8);
    while controller.controller_session(connection.id).unwrap().phase != FfiSessionPhase::Active {
        assert!(Instant::now() < until);
        std::thread::sleep(Duration::from_millis(5));
    }
    let connection = controller.controller_session(connection.id).unwrap();
    assert_eq!(
        controller
            .remote_camera_devices(connection.id, connection.generation)
            .unwrap()[0]
            .id,
        "test-camera"
    );
    let owner = controller
        .attach_controller_service(connection.id, connection.generation, FfiServiceKind::Camera)
        .unwrap();
    let camera = controller
        .open_camera_service(
            owner.id,
            owner.generation,
            FfiCameraConfig {
                device_id: "test-camera".into(),
                width: 640,
                height: 480,
                fps: 30,
            },
        )
        .unwrap();
    let source = produced.lock().unwrap().clone().unwrap();
    assert!(source.take_keyframe_request());
    let expected = FfiEncodedFrame {
        codec: crate::FfiVideoCodec::H264,
        keyframe: true,
        timestamp_us: 42,
        payload: vec![9; 300_000],
        display_id: 0,
        revision: 0,
        timing: Default::default(),
    };
    source.publish(expected.clone()).unwrap();
    let until = Instant::now() + Duration::from_secs(8);
    let frame = loop {
        if let Some(frame) = camera.poll() {
            break frame;
        }
        assert!(Instant::now() < until);
        std::thread::sleep(Duration::from_millis(5));
    };
    assert_eq!(frame, expected);
    host.disable_camera().unwrap();
    let until = Instant::now() + Duration::from_secs(8);
    while !source.is_cancelled() || !camera.is_cancelled() {
        assert!(Instant::now() < until);
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(
        controller.controller_session(connection.id).unwrap().phase,
        FfiSessionPhase::Active
    );
}

#[test]
fn ffi_pointer_policy_preserves_fractional_motion_and_swapped_button_cleanup() {
    let host = core();
    let controller = core();
    let seen = Arc::new(Mutex::new(Vec::new()));
    host.inner
        .set_desktop(Box::new(Desktop(seen.clone())))
        .unwrap();
    pair(&host, &controller, false, false);
    let session = controller.connect(host.endpoint_id()).unwrap();
    let settings = FfiPointerSettings {
        sensitivity_percent: 50,
        reverse_wheel: true,
        swap_buttons: true,
    };
    controller
        .set_session_pointer_settings(session.id, session.generation, settings)
        .unwrap();
    assert_eq!(
        controller
            .session_pointer_settings(session.id, session.generation)
            .unwrap(),
        settings
    );
    assert!(controller
        .submit_precise_wheel(session.id, session.generation, 9000, 0)
        .is_err());
    controller
        .submit_motion(session.id, session.generation, 1, 0)
        .unwrap();
    controller
        .submit_button(session.id, session.generation, FfiPointerButton::Left, true)
        .unwrap();
    controller
        .submit_motion(session.id, session.generation, 1, 0)
        .unwrap();
    controller
        .submit_precise_wheel(session.id, session.generation, 15, -30)
        .unwrap();
    controller
        .submit_button(
            session.id,
            session.generation,
            FfiPointerButton::Forward,
            true,
        )
        .unwrap();
    controller
        .submit_button(
            session.id,
            session.generation,
            FfiPointerButton::Forward,
            false,
        )
        .unwrap();
    controller
        .release_session_input(session.id, session.generation)
        .unwrap();
    let expected = vec![
        Applied::Input(vnilink::InputEvent::button_down(
            vnilink::PointerButton::Right,
        )),
        Applied::Input(vnilink::InputEvent::motion(1, 0)),
        Applied::Input(vnilink::InputEvent::precise_wheel(-15, 30)),
        Applied::Input(vnilink::InputEvent::button_down(
            vnilink::PointerButton::Forward,
        )),
        Applied::Input(vnilink::InputEvent::button_up(
            vnilink::PointerButton::Forward,
        )),
        Applied::Input(vnilink::InputEvent::button_up(
            vnilink::PointerButton::Right,
        )),
    ];
    let deadline = Instant::now() + Duration::from_secs(5);
    while *seen.lock().unwrap() != expected {
        assert!(
            Instant::now() < deadline,
            "FFI pointer sequence did not settle: {:?}",
            seen.lock().unwrap()
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    controller.shutdown();
    host.shutdown();
}

#[test]
fn camera_provider_sees_startup_cancellation_through_ffi() {
    struct Provider {
        entered: std::sync::mpsc::SyncSender<()>,
        finished: std::sync::mpsc::SyncSender<()>,
    }
    impl CameraProvider for Provider {
        fn devices(&self) -> Result<Vec<FfiCameraDevice>, FfiError> {
            Ok(vec![FfiCameraDevice {
                id: "camera".into(),
                name: "Test".into(),
            }])
        }
        fn start_capture(
            &self,
            _: FfiCameraConfig,
            startup: Arc<BackendStartup>,
        ) -> Result<Arc<CameraSource>, FfiError> {
            self.entered.send(()).unwrap();
            let deadline = Instant::now() + Duration::from_secs(5);
            while !startup.is_cancelled() {
                assert!(Instant::now() < deadline);
                std::thread::sleep(Duration::from_millis(5));
            }
            assert!(startup.check().is_err());
            self.finished.send(()).unwrap();
            startup.check()?;
            unreachable!()
        }
    }
    let host = core();
    let controller = core();
    pair(&host, &controller, false, false);
    host.set_service_sharing(FfiServiceKind::Camera, true)
        .unwrap();
    let (entered, ready) = std::sync::mpsc::sync_channel(1);
    let (finished, done) = std::sync::mpsc::sync_channel(1);
    host.install_camera_provider(Box::new(Provider { entered, finished }))
        .unwrap();
    let pending = controller
        .begin_connect_services(host.endpoint_id())
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while controller.controller_session(pending.id).unwrap().phase != FfiSessionPhase::Active {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    }
    let parent = controller.controller_session(pending.id).unwrap();
    let owner = controller
        .attach_controller_service(parent.id, parent.generation, FfiServiceKind::Camera)
        .unwrap();
    std::thread::scope(|scope| {
        let open = scope.spawn(|| {
            controller.open_camera_service(
                owner.id,
                owner.generation,
                FfiCameraConfig {
                    device_id: "camera".into(),
                    width: 640,
                    height: 480,
                    fps: 30,
                },
            )
        });
        ready.recv_timeout(Duration::from_secs(5)).unwrap();
        host.set_service_sharing(FfiServiceKind::Camera, false)
            .unwrap();
        done.recv_timeout(Duration::from_secs(1)).unwrap();
        assert!(open.join().unwrap().is_err());
    });
    assert_eq!(
        controller.controller_session(parent.id).unwrap().phase,
        FfiSessionPhase::Active
    );
}

#[test]
fn terminal_policy_crosses_ffi_and_rejects_invalid_limits() {
    let core = core();
    let policy = FfiTerminalPolicy {
        allow_persistence: false,
        max_sessions: 2,
        max_sessions_per_peer: 1,
        history_bytes: 16384,
        replay_bytes: 2048,
        detached_timeout_secs: 15,
    };
    core.set_terminal_policy(policy).unwrap();
    assert_eq!(core.terminal_policy().unwrap(), policy);
    let mut invalid = policy;
    invalid.max_sessions_per_peer = 3;
    assert!(core.set_terminal_policy(invalid).is_err());
    assert_eq!(core.terminal_policy().unwrap(), policy);
}

#[test]
fn file_retention_policy_crosses_ffi_with_exact_values() {
    let core = core();
    let policy = FfiFileRetentionPolicy {
        resume_cache_bytes: 0,
        resume_cache_age_secs: 0,
        recent_fetch_bytes: 12345,
        recent_fetch_age_secs: 18,
    };
    core.set_file_retention_policy(policy).unwrap();
    assert_eq!(core.file_retention_policy().unwrap(), policy);
    let mut invalid = policy;
    invalid.recent_fetch_age_secs = u64::MAX;
    assert!(core.set_file_retention_policy(invalid).is_err());
    assert_eq!(core.file_retention_policy().unwrap(), policy);
}

#[test]
fn file_resource_admission_policy_crosses_ffi_with_exact_values_and_bounds() {
    let core = core();
    let policy = FfiFileResourcePolicy {
        max_concurrent_receivers: 3,
        max_reserved_bytes: 123456789,
        minimum_free_bytes: 987654,
    };
    core.set_file_resource_policy(policy).unwrap();
    assert_eq!(core.file_resource_policy().unwrap(), policy);
    let mut invalid = policy;
    invalid.max_concurrent_receivers = 0;
    assert!(core.set_file_resource_policy(invalid).is_err());
    assert_eq!(core.file_resource_policy().unwrap(), policy);
}

#[test]
fn measured_media_status_crosses_ffi_without_losing_counters_or_reason() {
    let status = vnilink::DesktopMediaStatus {
        reason: vnilink::AdaptationReason::NetworkPressure,
        received_kbps: 12345,
        received_fps: 57,
        receiver_queue_ms: 123,
        host_queue_ms: 456,
        send_loss_per_mille: 17,
        rtt_ms: 789,
        decode_us: 1234,
        encode_us: 5678,
        capture_us: 9012,
        conditions: vnilink::ConditionSet::default(),
    };
    let ffi: FfiDesktopMediaStatus = status.into();
    assert_eq!(ffi.reason, FfiAdaptationReason::NetworkPressure);
    assert_eq!((ffi.received_kbps, ffi.received_fps), (12345, 57));
    assert_eq!(
        (ffi.receiver_queue_ms, ffi.host_queue_ms, ffi.rtt_ms),
        (123, 456, 789)
    );
    assert_eq!(ffi.send_loss_per_mille, 17);
    assert_eq!(ffi.decode_us, 1234);
    assert_eq!((ffi.encode_us, ffi.capture_us), (5678, 9012));
}

#[test]
fn system_actions_cross_ffi_with_policy_refusal_and_no_side_effects() {
    let host = core();
    let controller = core();
    pair(&host, &controller, false, false);
    host.set_desktop_sharing(false, false, false, false)
        .unwrap();
    host.set_service_sharing(FfiServiceKind::System, true)
        .unwrap();
    // Default policy refuses shutdown without touching any native backend.
    let pending = controller
        .begin_connect_services(host.endpoint_id())
        .unwrap();
    let until = Instant::now() + Duration::from_secs(5);
    let session = loop {
        let session = controller.controller_session(pending.id).unwrap();
        if session.phase == FfiSessionPhase::Active {
            break session;
        }
        assert!(
            Instant::now() < until,
            "system FFI connection did not activate"
        );
        std::thread::sleep(Duration::from_millis(10));
    };
    let owner = controller
        .attach_controller_service(session.id, session.generation, FfiServiceKind::System)
        .unwrap();
    let system = controller
        .open_system_service(owner.id, owner.generation)
        .unwrap();
    let report = system.support().unwrap();
    assert!(!report.support.lock);
    assert_eq!(report.session.locked, None);
    let refused = system.execute(FfiSystemAction::Shutdown).unwrap();
    assert_eq!(refused.state, FfiOperationState::Refused);
    assert_eq!(system.operation(refused.id).unwrap(), refused);
    // Policy round-trips through FFI records; the machine is untouched.
    let policy = host.system_policy().unwrap();
    assert!(policy.allow_lock && !policy.allow_shutdown && !policy.allow_privacy);
    host.set_system_policy(FfiSystemPolicy {
        allow_shutdown: true,
        ..policy
    })
    .unwrap();
    assert!(host.system_policy().unwrap().allow_shutdown);
    let privacy = host.system_privacy().unwrap();
    assert!(!privacy.display_private && !privacy.input_blocked);
    assert_eq!(privacy.holder_connection, None);
}

#[test]
fn clipboard_files_cross_the_ffi_boundary() {
    let host = core();
    let controller = core();
    pair(&host, &controller, true, true);
    controller
        .set_desktop_sharing(true, true, true, true)
        .unwrap();
    let session = controller.connect(host.endpoint_id()).unwrap();
    let incoming = host.snapshot().unwrap().incoming.unwrap();
    let offer = FfiFileSelection {
        entries: vec![FfiClipboardFileEntry {
            path: "note.txt".into(),
            size: 9,
            dir: false,
        }],
    };
    controller
        .submit_clipboard_files(session.id, session.generation, offer)
        .unwrap();
    let stored = host.incoming_clipboard_files(incoming.id).unwrap().unwrap();
    assert_eq!(stored.revision, 1);
    assert_eq!(stored.selection.entries.len(), 1);
    assert_eq!(stored.selection.entries[0].path, "note.txt");
    // Invalid selections map to a typed FFI error.
    let bad = FfiFileSelection {
        entries: vec![FfiClipboardFileEntry {
            path: "../escape".into(),
            size: 1,
            dir: false,
        }],
    };
    assert!(matches!(
        controller.submit_clipboard_files(session.id, session.generation, bad),
        Err(FfiError::InvalidSetting { .. })
    ));
}

#[test]
fn producer_duration_report_crosses_the_ffi_boundary_into_diagnostics() {
    let host = core();
    let controller = core();
    host.inner
        .set_desktop(Box::new(Desktop(Arc::new(Mutex::new(Vec::new())))))
        .unwrap();
    pair(&host, &controller, false, false);
    let pending = controller
        .begin_connect_services(host.endpoint_id())
        .unwrap();
    let until = Instant::now() + Duration::from_secs(5);
    while controller.controller_session(pending.id).unwrap().phase != FfiSessionPhase::Active {
        assert!(Instant::now() < until);
        std::thread::sleep(Duration::from_millis(10));
    }
    let parent = controller.controller_session(pending.id).unwrap();
    let owner = controller
        .attach_controller_service(parent.id, parent.generation, FfiServiceKind::Desktop)
        .unwrap();
    let _first = controller
        .open_desktop_service(owner.id, owner.generation)
        .unwrap();
    let remote = host
        .hosted_services()
        .unwrap()
        .into_iter()
        .find(|s| s.kind == FfiServiceKind::Desktop)
        .unwrap();
    host.report_desktop_producer_duration(remote.id, remote.generation, 100, 200)
        .unwrap();
    let snapshot = host.inner.diagnostic_snapshot().unwrap();
    let pipeline = snapshot
        .pipelines
        .iter()
        .find(|p| p.service_id == remote.id)
        .unwrap();
    assert_eq!(
        (pipeline.capture_us, pipeline.encode_us),
        (Some(100), Some(200))
    );
    assert!(host
        .report_desktop_producer_duration(remote.id, remote.generation, 6_000_001, 0)
        .is_err());
    assert!(host
        .report_desktop_producer_duration(9999, 0, 1, 1)
        .is_err());
}
