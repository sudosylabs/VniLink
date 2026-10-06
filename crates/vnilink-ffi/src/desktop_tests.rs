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
