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
    }
}
fn pair(host: &VnilinkCore, controller: &VnilinkCore, clipboard: bool, transfer: bool) {
    host.set_h264_support(true, true).unwrap();
    controller.set_h264_support(true, true).unwrap();
    host.inner.set_capture(Box::new(Capture)).unwrap();
    controller
        .accept_invitation(host.create_invitation(60).unwrap())
        .unwrap();
    host.accept_invitation(controller.create_invitation(60).unwrap())
        .unwrap();
    host.authorize(controller.endpoint_id(), true, clipboard, transfer)
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
            "vnilink-ffi-service-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        )));
    std::fs::create_dir(&scratch.0).unwrap();
    host.inner.set_file_roots(vec![scratch.0.clone()]).unwrap();
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
    let root = host.inner.file_roots().unwrap()[0].0;
    pipe.mkdir(root, "".into(), "folder".into()).unwrap();
    let page = pipe.list(root, "".into(), 0, 20).unwrap();
    assert_eq!(page.entries.len(), 1);
    pipe.rename(
        root,
        "".into(),
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
        FfiServicePhase::Closed
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
    assert_eq!(changed[2].phase, FfiServicePhase::Closed);
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
    host.set_transfer_directory(host_dir.to_string_lossy().into())
        .unwrap();
    controller
        .set_transfer_directory(controller_dir.to_string_lossy().into())
        .unwrap();
    pair(&host, &controller, true, true);
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
            .submit_local_key(session.id, session.generation, usage, down, text.into())
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
    controller
        .submit_file(
            session.id,
            session.generation,
            "sent.bin".into(),
            vec![0, 255, 10],
            false,
        )
        .unwrap();
    assert_eq!(
        std::fs::read(host_dir.join("sent.bin")).unwrap(),
        [0, 255, 10]
    );
    assert_eq!(
        controller.submit_file(
            session.id,
            session.generation,
            "sent.bin".into(),
            vec![1],
            false
        ),
        Err(FfiError::FileExists)
    );
    host.publish_file(incoming.id, "returned.bin".into(), vec![4, 2], false)
        .unwrap();
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
        controller.submit_file(
            session.id,
            session.generation,
            "denied.txt".into(),
            vec![1],
            false
        ),
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
            "vnilink-ffi-owned-jobs-{}-{}",
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
        .set_blob_directory(scratch.0.join("host-blobs"))
        .unwrap();
    controller
        .inner
        .set_blob_directory(scratch.0.join("controller-blobs"))
        .unwrap();
    host.inner.set_file_roots(vec![host_root.clone()]).unwrap();
    controller
        .inner
        .set_file_roots(vec![controller_root.clone()])
        .unwrap();
    pair(&host, &controller, false, true);
    controller
        .authorize(host.endpoint_id(), false, false, true)
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
            0,
            "up.txt".into(),
            0,
            "uploaded".into(),
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
            0,
            "down.txt".into(),
            0,
            "cancelled".into(),
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
    while controller.inner.job_snapshot(down).unwrap().state != vnilink::JobState::Cancelled {
        assert!(Instant::now() < until, "owned FFI job did not cancel");
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(!controller_root.join("cancelled").exists());
    assert!(controller
        .files_service_download(
            owner.id,
            owner.generation,
            0,
            "down.txt".into(),
            0,
            "later".into()
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
            camera: false
        }
    );
    host.authorize_service(id.clone(), FfiServiceKind::Terminal, true)
        .unwrap();
    host.authorize_service(id.clone(), FfiServiceKind::Camera, true)
        .unwrap();
    host.authorize_service(id.clone(), FfiServiceKind::Files, false)
        .unwrap();
    assert_eq!(
        host.service_permissions(id.clone()).unwrap(),
        FfiServicePermissions {
            desktop: true,
            files: false,
            terminal: true,
            camera: true
        }
    );
    // Legacy native authorization only changes its three legacy fields.
    host.authorize(id.clone(), true, false, true).unwrap();
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
    host.revoke(id.clone()).unwrap();
    assert_eq!(
        host.service_permissions(id).unwrap(),
        FfiServicePermissions {
            desktop: false,
            files: false,
            terminal: false,
            camera: false
        }
    );
}

#[test]
fn neutral_connections_cross_ffi_without_desktop_or_transfer_grants() {
    let host = core();
    let controller = core();
    pair(&host, &controller, false, false);
    host.authorize(controller.endpoint_id(), false, false, false)
        .unwrap();
    host.set_h264_support(false, false).unwrap();
    controller.set_h264_support(false, false).unwrap();
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
    host.authorize_service(controller.endpoint_id(), FfiServiceKind::Files, true)
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
        FfiServicePhase::Closed
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
    first.close();
    let next = controller
        .attach_controller_service(parent.id, parent.generation, FfiServiceKind::Desktop)
        .unwrap();
    let second = controller
        .open_desktop_service(next.id, next.generation)
        .unwrap();
    first.close();
    assert!(!second.is_cancelled());
    assert!(first.submit_pointer(20, 30).is_err());
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
        .submit_local_key(vnilink::usage::GRAVE, true, "@".into())
        .unwrap();
    assert!(desktop.set_keyboard_mode(FfiKeyboardMode::Map).is_err());
    let until = Instant::now() + Duration::from_secs(5);
    while applied.lock().unwrap().len() != 1 {
        assert!(Instant::now() < until);
        std::thread::sleep(Duration::from_millis(10));
    }
    desktop
        .submit_local_key(vnilink::usage::GRAVE, false, "".into())
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
    assert!(desktop.submit_local_key(4, true, "a".into()).is_err());
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
    host.authorize(controller.endpoint_id(), true, false, false)
        .unwrap();
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
        .authorize(host.endpoint_id(), false, false, true)
        .unwrap();
    let directory = std::env::temp_dir().join(format!(
        "vnilink-ffi-owned-jobs-{}-{}",
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
        core.inner
            .set_file_roots(vec![directory.join(name).join("root")])
            .unwrap();
        core.inner
            .set_blob_directory(directory.join(name).join("blobs"))
            .unwrap();
    }
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
        .upload(0, "source.txt".into(), 0, "upload".into())
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
    while files.job_snapshot(job).unwrap().state != FfiJobState::Cancelled {
        assert!(Instant::now() < until);
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(files
        .upload(0, "source.txt".into(), 0, "refused".into())
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
        .download(0, "return.txt".into(), 0, "download".into())
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
