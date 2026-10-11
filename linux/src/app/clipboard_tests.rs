//! Real GTK clipboard and paired-core contracts, run in the isolated GTK child.
use super::*;

pub(crate) fn hidden_window_accepts_clipboard() {
    use vnilink::HostClipboard;
    let window = adw::ApplicationWindow::builder().build();
    assert!(!window.is_visible());
    let (mut adapter, native) = native_adapter(&window);
    let (send, receive) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = send.send(adapter.set_text("hidden host copy"));
    });
    let context = gtk::glib::MainContext::default();
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    let result = loop {
        while context.pending() {
            context.iteration(false);
        }
        if let Ok(result) = receive.try_recv() {
            break result;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "clipboard write timed out"
        );
        std::thread::sleep(Duration::from_millis(5));
    };
    assert!(
        native.is_remote(),
        "native remote writes must be marked to prevent echoes"
    );
    assert!(
        result.is_ok(),
        "hidden host must still accept remote clipboard text: {result:?}"
    );
}

pub(crate) fn automatic_text_contract() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    struct Capture;
    impl vnilink::HostCapture for Capture {
        fn monitor(&self) -> (u16, u16) {
            (1280, 720)
        }
    }
    struct Clipboard(Arc<Mutex<Option<String>>>, Arc<AtomicUsize>);
    impl vnilink::HostClipboard for Clipboard {
        fn set_text(&mut self, text: &str) -> Result<(), vnilink::Error> {
            *self.0.lock().unwrap() = Some(text.into());
            self.1.fetch_add(1, Ordering::Relaxed);
            Ok(())
        }
    }
    type Inputs = Arc<Mutex<Vec<(vnilink::InputEvent, Option<String>)>>>;
    struct Desktop(Inputs, Arc<Mutex<Option<String>>>);
    impl vnilink::HostDesktop for Desktop {
        fn apply(&mut self, event: vnilink::InputEvent) -> Result<(), vnilink::Error> {
            self.0
                .lock()
                .unwrap()
                .push((event, self.1.lock().unwrap().clone()));
            Ok(())
        }
        fn monitor(&self) -> (u16, u16) {
            (1280, 720)
        }
    }
    fn core() -> Arc<Core> {
        Arc::new(
            Core::open(
                Arc::new(vnilink::MemoryStore::empty()),
                vnilink::OpenConfig {
                    listen: vnilink::Listen::Loopback,
                    initial_relay: vnilink::RelaySettings::custom(vec![vnilink::CustomRelay::new(
                        "http://127.0.0.1:9",
                    )
                    .unwrap()])
                    .unwrap(),
                    ..vnilink::OpenConfig::default()
                },
            )
            .unwrap(),
        )
    }
    fn pump(ready: impl Fn() -> bool) {
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while !ready() {
            assert!(
                std::time::Instant::now() < deadline,
                "native sync did not complete"
            );
            let context = gtk::glib::MainContext::default();
            while context.pending() {
                context.iteration(false);
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }
    let native_core = core();
    let remote = core();
    let window = adw::ApplicationWindow::builder().build();
    // The host must remain usable when its primary window is hidden in the tray.
    assert!(!window.is_visible());
    install(&window, native_core.clone());
    let board = window.clipboard();
    let remote_text = Arc::new(Mutex::new(None::<String>));
    let remote_writes = Arc::new(AtomicUsize::new(0));
    let inputs = Arc::new(Mutex::new(Vec::new()));
    remote
        .set_desktop(Box::new(Desktop(inputs.clone(), remote_text.clone())))
        .unwrap();
    remote
        .set_clipboard(Box::new(Clipboard(
            remote_text.clone(),
            remote_writes.clone(),
        )))
        .unwrap();
    for core in [&native_core, &remote] {
        core.set_video_codecs(vnilink::VideoCodecCaps::h264_full(4096, 4096, "test"))
            .unwrap();
        core.set_capture(Box::new(Capture)).unwrap();
    }
    native_core
        .accept_invitation(&remote.create_invitation(Duration::from_secs(60)).unwrap())
        .unwrap();
    remote
        .accept_invitation(
            &native_core
                .create_invitation(Duration::from_secs(60))
                .unwrap(),
        )
        .unwrap();
    let grants = vnilink::Grants {
        view: true,
        control: true,
        clipboard: true,
        transfer: true,
    };
    remote
        .authorize(&native_core.endpoint_id(), grants)
        .unwrap();
    native_core
        .authorize(&remote.endpoint_id(), grants)
        .unwrap();
    let root = std::env::temp_dir().join(format!("vnilink-gtk-contract-{}", std::process::id()));
    let local_files = root.join("native");
    let remote_files = root.join("remote");
    native_core.set_blob_directory(&local_files).unwrap();
    remote.set_blob_directory(&remote_files).unwrap();
    remote.start_hosting().unwrap();
    let pending = native_core
        .begin_connect_services(&remote.endpoint_id())
        .unwrap();
    pump(|| native_core.controller_session(pending.id).unwrap().phase == SessionPhase::Active);
    let session = native_core.controller_session(pending.id).unwrap();
    let viewer = super::super::viewer::Viewer::build(&window, native_core.clone(), session.clone());
    viewer.start(native_core.clone());
    pump(|| {
        viewer.desktop_owner().is_some_and(|owner| {
            owner.control_state().ok() == Some(vnilink::DesktopControlState::Controlling)
        })
    });
    let controllers = viewer.picture.observe_controllers();
    let keys = controllers
        .iter::<gtk::glib::Object>()
        .filter_map(Result::ok)
        .find_map(|c| c.downcast::<gtk::EventControllerKey>().ok())
        .unwrap();
    assert!(controllers
        .iter::<gtk::glib::Object>()
        .any(|c| c.unwrap().is::<gtk::DropTarget>()));
    // Paste immediately, before the asynchronous changed callback can finish.
    // Control was already held when the image gained focus. Selection
    // ownership itself is asynchronous under X11: wait until a read-back
    // sees the text, so the paste below models an established clipboard
    // rather than the ownership handshake.
    board.set_text("paste before shortcut");
    let owned = std::rc::Rc::new(std::cell::Cell::new(false));
    let seen = owned.clone();
    board.read_text_async(None::<&gtk::gio::Cancellable>, move |result| {
        seen.set(result.ok().flatten().as_deref() == Some("paste before shortcut"));
    });
    pump(|| owned.get());
    let ctrl = gtk::gdk::ModifierType::CONTROL_MASK;
    assert!(keys.emit_by_name::<bool>("key-pressed", &[&gtk::gdk::Key::v, &55u32, &ctrl]));
    keys.emit_by_name::<()>("key-released", &[&gtk::gdk::Key::v, &55u32, &ctrl]);
    keys.emit_by_name::<()>(
        "key-released",
        &[
            &gtk::gdk::Key::Control_R,
            &105u32,
            &gtk::gdk::ModifierType::empty(),
        ],
    );
    pump(|| inputs.lock().unwrap().len() == 4);
    let events = inputs.lock().unwrap().clone();
    assert_eq!(
        events.iter().map(|(e, _)| *e).collect::<Vec<_>>(),
        [
            vnilink::InputEvent::key_down(224),
            vnilink::InputEvent::key_down(25),
            vnilink::InputEvent::key_up(25),
            vnilink::InputEvent::key_up(224),
        ]
    );
    assert_eq!(
        events[1].1.as_deref(),
        Some("paste before shortcut"),
        "paste key arrived before clipboard application"
    );
    // Desktop file drops are exercised by the Files staging contract. They
    // now select an approved item for explicit destination/Send confirmation.
    board.set_text("local copy α");
    pump(|| remote_text.lock().unwrap().as_deref() == Some("local copy α"));
    let count = remote_writes.load(Ordering::Relaxed);
    let incoming = remote
        .hosted_services()
        .unwrap()
        .into_iter()
        .find(|owner| {
            owner.kind == vnilink::ServiceKind::Desktop
                && owner.phase == vnilink::ServicePhase::Active
        })
        .unwrap();
    remote
        .publish_desktop_clipboard(incoming.id, incoming.generation, "remote copy β")
        .unwrap();
    let returned = Rc::new(RefCell::new(None::<String>));
    let poll = returned.clone();
    pump(|| {
        let poll = poll.clone();
        board.read_text_async(None::<&gtk::gio::Cancellable>, move |result| {
            if let Ok(Some(text)) = result {
                *poll.borrow_mut() = Some(text.into());
            }
        });
        returned.borrow().as_deref() == Some("remote copy β")
    });
    // Let the observer and worker run: remote content must not bounce back.
    let until = std::time::Instant::now() + Duration::from_millis(200);
    pump(|| std::time::Instant::now() >= until);
    assert_eq!(
        remote_writes.load(Ordering::Relaxed),
        count,
        "remote clipboard echoed to its sender"
    );
    // Bidirectional Files jobs have their own real-disk runtime contracts.
    // Closing this Desktop leaves its neutral parent independently available.
    viewer.retire();
    native_core.disconnect_session(session.id).unwrap();
    viewer.window.close();
    pump(|| {
        remote
            .snapshot()
            .unwrap()
            .incoming
            .is_none_or(|s| s.phase != SessionPhase::Active)
    });
    native_core.start_hosting().unwrap();
    let pending = remote
        .begin_connect_services(&native_core.endpoint_id())
        .unwrap();
    pump(|| remote.controller_session(pending.id).unwrap().phase == SessionPhase::Active);
    let control = remote.controller_session(pending.id).unwrap();
    let owner = remote
        .attach_controller_service(
            control.id,
            control.generation,
            vnilink::ServiceKind::Desktop,
        )
        .unwrap();
    let desktop = Arc::new(
        remote
            .open_desktop_service(owner.id, owner.generation)
            .unwrap(),
    );
    let worker = desktop.clone();
    let (done, result) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = done.send(worker.submit_clipboard("hidden host γ"));
    });
    pump(|| match result.try_recv() {
        Ok(r) => {
            r.unwrap();
            true
        }
        Err(_) => false,
    });
    let incoming = native_core
        .hosted_services()
        .unwrap()
        .into_iter()
        .find(|owner| {
            owner.kind == vnilink::ServiceKind::Desktop
                && owner.phase == vnilink::ServicePhase::Active
        })
        .unwrap();
    board.set_text("host return δ");
    pump(|| desktop.clipboard_text().unwrap().as_deref() == Some("host return δ"));
    // Revocation must stop automatic publication even while the session continues.
    native_core
        .authorize(
            &remote.endpoint_id(),
            vnilink::Grants {
                clipboard: false,
                ..grants
            },
        )
        .unwrap();
    board.set_text("private clipboard");
    let until = std::time::Instant::now() + Duration::from_millis(200);
    pump(|| std::time::Instant::now() >= until);
    assert_eq!(
        desktop.clipboard_text().unwrap().as_deref(),
        Some("host return δ")
    );
    assert!(matches!(
        native_core.publish_desktop_clipboard(incoming.id, incoming.generation, "denied"),
        Err(vnilink::Error::ClipboardDenied)
    ));
    remote.disconnect_session(control.id).unwrap();
    remote.shutdown();
    native_core.shutdown();
    let context = gtk::glib::MainContext::default();
    while context.pending() {
        context.iteration(false);
    }
    std::fs::remove_dir_all(root).unwrap();
}
