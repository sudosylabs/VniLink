use super::*;
use vnilink::usage;

#[test]
fn physical_keys_buttons_scroll_and_scaled_pointer_map_to_portal_events() {
    let map = |event| notifications(event, (3840, 2160), (1920, 1080), (0, 0)).unwrap();
    assert_eq!(
        map(InputEvent::key_down(usage::A)),
        vec![Notify::Key(30, true)]
    );
    assert_eq!(
        map(InputEvent::key_up(usage::RIGHT_CONTROL)),
        vec![Notify::Key(97, false)]
    );
    assert!(notifications(InputEvent::key_down(0xffff), (2, 2), (2, 2), (0, 0)).is_err());
    assert_eq!(
        map(InputEvent::pointer(1920, 1080, 1, 1)),
        vec![Notify::Pointer(960.0, 540.0)]
    );
    assert!(notifications(
        InputEvent::pointer(65535, 65535, 1, 1),
        (3840, 2160),
        (1920, 1080),
        (0, 0)
    )
    .is_err());
    assert!(notifications(
        InputEvent::pointer(-1, 0, 1, 1),
        (3840, 2160),
        (1920, 1080),
        (0, 0)
    )
    .is_err());
    assert_eq!(
        notifications(
            InputEvent::pointer(110, 24, 1, 1),
            (100, 50),
            (100, 50),
            (100, 20)
        )
        .unwrap(),
        vec![Notify::Pointer(10.0, 4.0)]
    );
    assert!(notifications(
        InputEvent::pointer(10, 4, 1, 1),
        (100, 50),
        (100, 50),
        (100, 20)
    )
    .is_err());
    for (button, code) in [
        (PointerButton::Left, 272),
        (PointerButton::Right, 273),
        (PointerButton::Middle, 274),
    ] {
        assert_eq!(
            map(InputEvent::button_down(button)),
            vec![Notify::Button(code, true)]
        );
        assert_eq!(
            map(InputEvent::button_up(button)),
            vec![Notify::Button(code, false)]
        );
    }
    assert_eq!(
        map(InputEvent::wheel(-2, 3)),
        vec![
            Notify::Wheel(Axis::Horizontal, -2),
            Notify::Wheel(Axis::Vertical, 3)
        ]
    );
    assert!(map(InputEvent::wheel(0, 0)).is_empty());
}

#[test]
fn adapter_orders_text_events_and_refuses_input_after_revocation() {
    let (tx, rx) = mpsc::sync_channel(64);
    let connection = Arc::new(Connection {
        tx,
        closed: AtomicBool::new(false),
    });
    let mut input = WaylandInput {
        connection: connection.clone(),
        pixels: (1920, 1080),
        logical: (1920, 1080),
        origin: (0, 0),
    };
    let thread = std::thread::spawn(move || match rx.recv().unwrap() {
        Command::Input(events, reply) => {
            assert_eq!(
                events,
                vec![
                    Notify::Symbol(0xe9, true),
                    Notify::Symbol(0xe9, false),
                    Notify::Symbol(0x0101_f600, true),
                    Notify::Symbol(0x0101_f600, false),
                    Notify::Symbol(0xff0d, true),
                    Notify::Symbol(0xff0d, false)
                ]
            );
            reply.send(Ok(())).unwrap();
        }
        _ => panic!("expected text input"),
    });
    input.type_text("é😀\n").unwrap();
    thread.join().unwrap();
    input.close();
    assert!(matches!(
        input.apply(InputEvent::key_down(usage::A)),
        Err(vnilink::Error::InputFailed(_))
    ));
    assert!(input.type_text("test").is_err());
    assert_eq!(input.monitor(), (1920, 1080));
}

// A private D-Bus service checks the real ashpd wire calls without injecting
// into the user's desktop. Run under dbus-run-session; never replace a portal.
use ashpd::zbus::{
    self,
    zvariant::{OwnedObjectPath, OwnedValue, Value},
};
use std::collections::HashMap;
use std::sync::Mutex;
struct FakePortal(
    Arc<Mutex<Vec<Notify>>>,
    Arc<AtomicBool>,
    Arc<std::sync::atomic::AtomicU32>,
    Arc<AtomicBool>,
    Arc<AtomicBool>,
);
#[zbus::interface(name = "org.freedesktop.portal.RemoteDesktop")]
impl FakePortal {
    #[zbus(property, name = "version")]
    fn version(&self) -> u32 {
        2
    }
    async fn create_session(
        &self,
        options: HashMap<String, OwnedValue>,
        #[zbus(connection)] connection: &zbus::Connection,
        #[zbus(header)] header: zbus::message::Header<'_>,
    ) -> zbus::fdo::Result<OwnedObjectPath> {
        let sender = header
            .sender()
            .unwrap()
            .as_str()
            .trim_start_matches(':')
            .replace('.', "_");
        let token = |key: &str| String::try_from(options[key].try_clone().unwrap()).unwrap();
        let session = format!(
            "/org/freedesktop/portal/desktop/session/{sender}/{}",
            token("session_handle_token")
        );
        let request = format!(
            "/org/freedesktop/portal/desktop/request/{sender}/{}",
            token("handle_token")
        );
        let mut result = HashMap::new();
        connection
            .object_server()
            .at(session.as_str(), FakeSession(self.2.clone()))
            .await
            .unwrap();
        result.insert("session_handle", Value::from(session));
        connection
            .emit_signal(
                None::<&str>,
                request.as_str(),
                "org.freedesktop.portal.Request",
                "Response",
                &(0u32, result),
            )
            .await
            .unwrap();
        Ok(OwnedObjectPath::try_from(request).unwrap())
    }
    async fn select_devices(
        &self,
        _session: OwnedObjectPath,
        options: HashMap<String, OwnedValue>,
        #[zbus(connection)] connection: &zbus::Connection,
        #[zbus(header)] header: zbus::message::Header<'_>,
    ) -> zbus::fdo::Result<OwnedObjectPath> {
        assert_eq!(
            u32::try_from(options["types"].try_clone().unwrap()).unwrap(),
            3
        );
        assert_eq!(
            u32::try_from(options["persist_mode"].try_clone().unwrap()).unwrap(),
            0
        );
        if self.3.load(Ordering::Acquire) {
            self.4.store(true, Ordering::Release);
            let sender = header
                .sender()
                .unwrap()
                .as_str()
                .trim_start_matches(':')
                .replace('.', "_");
            let token = String::try_from(options["handle_token"].try_clone().unwrap()).unwrap();
            return Ok(OwnedObjectPath::try_from(format!(
                "/org/freedesktop/portal/desktop/request/{sender}/{token}"
            ))
            .unwrap());
        }
        response(connection, header, &options, HashMap::new()).await
    }
    async fn start(
        &self,
        _session: OwnedObjectPath,
        _parent: String,
        options: HashMap<String, OwnedValue>,
        #[zbus(connection)] connection: &zbus::Connection,
        #[zbus(header)] header: zbus::message::Header<'_>,
    ) -> zbus::fdo::Result<OwnedObjectPath> {
        let mut props = HashMap::new();
        props.insert("size", Value::from((1920i32, 1080i32)));
        props.insert("source_type", Value::from(1u32));
        let mut result = HashMap::new();
        result.insert(
            "devices",
            Value::from(if self.1.load(Ordering::Relaxed) {
                3u32
            } else {
                1u32
            }),
        );
        result.insert("streams", Value::from(vec![(42u32, props)]));
        response(connection, header, &options, result).await
    }
    async fn notify_keyboard_keycode(
        &self,
        session: OwnedObjectPath,
        _options: HashMap<String, OwnedValue>,
        keycode: i32,
        state: u32,
        #[zbus(connection)] connection: &zbus::Connection,
    ) {
        self.0
            .lock()
            .unwrap()
            .push(Notify::Key(keycode, state == 1));
        // Synthetic key 999 is only used to revoke this fake permission grant.
        if keycode == 999 {
            connection
                .emit_signal(
                    None::<&str>,
                    session.as_str(),
                    "org.freedesktop.portal.Session",
                    "Closed",
                    &(HashMap::<String, OwnedValue>::new(),),
                )
                .await
                .unwrap();
        }
    }
    fn notify_keyboard_keysym(
        &self,
        _session: OwnedObjectPath,
        _options: HashMap<String, OwnedValue>,
        keysym: i32,
        state: u32,
    ) {
        self.0
            .lock()
            .unwrap()
            .push(Notify::Symbol(keysym, state == 1));
    }
    fn notify_pointer_motion_absolute(
        &self,
        _session: OwnedObjectPath,
        _options: HashMap<String, OwnedValue>,
        stream: u32,
        x: f64,
        y: f64,
    ) {
        assert_eq!(stream, 42);
        self.0.lock().unwrap().push(Notify::Pointer(x, y));
    }
    fn notify_pointer_motion(
        &self,
        _session: OwnedObjectPath,
        _options: HashMap<String, OwnedValue>,
        dx: f64,
        dy: f64,
    ) {
        self.0.lock().unwrap().push(Notify::Motion(dx, dy));
    }
    fn notify_pointer_axis(
        &self,
        _session: OwnedObjectPath,
        _options: HashMap<String, OwnedValue>,
        dx: f64,
        dy: f64,
    ) {
        self.0.lock().unwrap().push(Notify::PreciseWheel(dx, dy));
    }
    fn notify_pointer_button(
        &self,
        _session: OwnedObjectPath,
        _options: HashMap<String, OwnedValue>,
        button: i32,
        state: u32,
    ) {
        self.0
            .lock()
            .unwrap()
            .push(Notify::Button(button, state == 1));
    }
    fn notify_pointer_axis_discrete(
        &self,
        _session: OwnedObjectPath,
        _options: HashMap<String, OwnedValue>,
        axis: u32,
        steps: i32,
    ) {
        self.0.lock().unwrap().push(Notify::Wheel(
            if axis == 0 {
                Axis::Vertical
            } else {
                Axis::Horizontal
            },
            steps,
        ));
    }
}

async fn response(
    connection: &zbus::Connection,
    header: zbus::message::Header<'_>,
    options: &HashMap<String, OwnedValue>,
    result: HashMap<&str, Value<'_>>,
) -> zbus::fdo::Result<OwnedObjectPath> {
    let sender = header
        .sender()
        .unwrap()
        .as_str()
        .trim_start_matches(':')
        .replace('.', "_");
    let token = String::try_from(options["handle_token"].try_clone().unwrap()).unwrap();
    let request = format!("/org/freedesktop/portal/desktop/request/{sender}/{token}");
    connection
        .emit_signal(
            None::<&str>,
            request.as_str(),
            "org.freedesktop.portal.Request",
            "Response",
            &(0u32, result),
        )
        .await
        .unwrap();
    Ok(OwnedObjectPath::try_from(request).unwrap())
}
struct FakeCast(Arc<std::sync::atomic::AtomicU32>);
#[zbus::interface(name = "org.freedesktop.portal.ScreenCast")]
impl FakeCast {
    #[zbus(property)]
    fn available_cursor_modes(&self) -> u32 {
        self.0.load(Ordering::Relaxed)
    }
    #[zbus(property, name = "version")]
    fn version(&self) -> u32 {
        4
    }
    async fn select_sources(
        &self,
        _session: OwnedObjectPath,
        options: HashMap<String, OwnedValue>,
        #[zbus(connection)] connection: &zbus::Connection,
        #[zbus(header)] header: zbus::message::Header<'_>,
    ) -> zbus::fdo::Result<OwnedObjectPath> {
        assert!(!options.contains_key("persist_mode") && !options.contains_key("restore_token"));
        assert_eq!(
            u32::try_from(options["cursor_mode"].try_clone().unwrap()).unwrap(),
            if self.0.load(Ordering::Relaxed) & 4 != 0 {
                4
            } else {
                2
            }
        );
        assert_eq!(
            u32::try_from(options["types"].try_clone().unwrap()).unwrap(),
            1
        );
        response(connection, header, &options, HashMap::new()).await
    }
    fn open_pipe_wire_remote(
        &self,
        _session: OwnedObjectPath,
        _options: HashMap<String, OwnedValue>,
    ) -> zbus::zvariant::OwnedFd {
        let fd: OwnedFd = std::fs::File::open("/dev/null").unwrap().into();
        fd.into()
    }
}
struct FakeSession(Arc<std::sync::atomic::AtomicU32>);
#[zbus::interface(name = "org.freedesktop.portal.Session")]
impl FakeSession {
    fn close(&self) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }
}

#[test]
#[ignore = "requires an isolated dbus-run-session bus"]
fn real_portal_notify_transport_preserves_all_events() {
    assert_eq!(
        std::env::var("VNILINK_TEST_PORTAL_BUS").as_deref(),
        Ok("private")
    );
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        tokio::time::timeout(Duration::from_secs(5), async {
            let recorded = Arc::new(Mutex::new(Vec::new()));
            let granted = Arc::new(AtomicBool::new(true));
            let closed = Arc::new(std::sync::atomic::AtomicU32::new(0));
            let stall = Arc::new(AtomicBool::new(false));
            let entered = Arc::new(AtomicBool::new(false));
            let cursor_modes = Arc::new(std::sync::atomic::AtomicU32::new(6));
            let _service = zbus::connection::Builder::session()
                .unwrap()
                .name("org.freedesktop.portal.Desktop")
                .unwrap()
                .serve_at(
                    "/org/freedesktop/portal/desktop",
                    FakePortal(
                        recorded.clone(),
                        granted.clone(),
                        closed.clone(),
                        stall.clone(),
                        entered.clone(),
                    ),
                )
                .unwrap()
                .serve_at(
                    "/org/freedesktop/portal/desktop",
                    FakeCast(cursor_modes.clone()),
                )
                .unwrap()
                .build()
                .await
                .unwrap();
            let client = zbus::Connection::session().await.unwrap();
            let proxy = RemoteDesktop::with_connection(client).await.unwrap();
            let session = proxy.create_session(Default::default()).await.unwrap();
            let expected = vec![
                Notify::Key(30, true),
                Notify::Key(30, false),
                Notify::Symbol(0xe9, true),
                Notify::Symbol(0xe9, false),
                Notify::Pointer(960.0, 540.0),
                Notify::Button(272, true),
                Notify::Button(272, false),
                Notify::Wheel(Axis::Horizontal, -2),
                Notify::Wheel(Axis::Vertical, 3),
                Notify::Motion(-2.0, 3.0),
                Notify::PreciseWheel(1.25, -2.5),
                Notify::Button(278, true),
                Notify::Button(278, false),
            ];
            deliver(&proxy, &session, 42, expected.clone())
                .await
                .unwrap();
            assert_eq!(*recorded.lock().unwrap(), expected);
            let grant = open_grant(&vnilink::BackendStartup::new(Duration::from_secs(5)))
                .await
                .unwrap();
            assert!(grant.cursor_metadata);
            assert!(grant.control);
            assert_eq!(grant.logical, (1920, 1080));
            assert_eq!(grant.node, 42);
            grant.session.close().await.unwrap();
            assert_eq!(closed.load(Ordering::Relaxed), 1);
            // Exercise the actual worker/runtime boundary and explicit close.
            for revoked in [false, true] {
                tokio::task::spawn_blocking(move || {
                    let (tx, rx) = mpsc::sync_channel(64);
                    let connection = Arc::new(Connection {
                        tx,
                        closed: AtomicBool::new(false),
                    });
                    let worker = connection.clone();
                    let (ready_tx, ready_rx) = mpsc::channel();
                    let thread = std::thread::spawn(move || {
                        worker_main(
                            worker,
                            rx,
                            ready_tx,
                            vnilink::BackendStartup::new(Duration::from_secs(5)),
                        )
                    });
                    let (_fd, node, logical, _, control) = ready_rx
                        .recv_timeout(Duration::from_secs(2))
                        .unwrap()
                        .unwrap();
                    assert_eq!(node, 42);
                    assert!(control);
                    let mut input = WaylandInput {
                        connection,
                        pixels: (3840, 2160),
                        logical,
                        origin: (0, 0),
                    };
                    input.apply(InputEvent::key_down(usage::A)).unwrap();
                    input.apply(InputEvent::key_up(usage::A)).unwrap();
                    if revoked {
                        input.send(vec![Notify::Key(999, true)]).unwrap();
                    } else {
                        input.close();
                    }
                    let deadline = std::time::Instant::now() + Duration::from_secs(1);
                    while !thread.is_finished() && std::time::Instant::now() < deadline {
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    assert!(
                        thread.is_finished(),
                        "worker must close on stop or compositor revocation"
                    );
                    thread.join().unwrap();
                    assert!(input.apply(InputEvent::key_down(usage::A)).is_err());
                })
                .await
                .unwrap();
            }
            assert_eq!(closed.load(Ordering::Relaxed), 3);
            // The screen without keyboard and pointer is a view-only grant,
            // kept open so its revocation still ends the capture.
            granted.store(false, Ordering::Relaxed);
            let view_only = open_grant(&vnilink::BackendStartup::new(Duration::from_secs(5)))
                .await
                .unwrap();
            assert!(!view_only.control);
            assert_eq!(closed.load(Ordering::Relaxed), 3);
            view_only.session.close().await.unwrap();
            assert_eq!(closed.load(Ordering::Relaxed), 4);
            granted.store(true, Ordering::Relaxed);
            cursor_modes.store(2, Ordering::Relaxed);
            let embedded = open_grant(&vnilink::BackendStartup::new(Duration::from_secs(5)))
                .await
                .unwrap();
            assert!(!embedded.cursor_metadata);
            embedded.session.close().await.unwrap();
            assert_eq!(closed.load(Ordering::Relaxed), 5);
            // Permission response is intentionally never emitted. Cancellation
            // must close the actual portal Session rather than await consent.
            stall.store(true, Ordering::Release);
            let stop = Arc::new(AtomicBool::new(false));
            let startup = vnilink::BackendStartup::with_stop(stop.clone(), Duration::from_secs(5));
            let cancel = async {
                while !entered.load(Ordering::Acquire) {
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
                stop.store(true, Ordering::Release);
            };
            let (result, ()) = tokio::join!(open_grant(&startup), cancel);
            assert!(result.is_err());
            assert_eq!(closed.load(Ordering::Relaxed), 6);
        })
        .await
        .unwrap();
    });
}

#[test]
fn fractional_scroll_relative_motion_and_side_buttons_map_without_rounding() {
    let map = |event| notifications(event, (3840, 2160), (1920, 1080), (0, 0)).unwrap();
    assert_eq!(
        map(InputEvent::precise_wheel(15, -30)),
        vec![Notify::PreciseWheel(1.25, -2.5)]
    );
    assert_eq!(
        map(InputEvent::motion(-2, 3)),
        vec![Notify::Motion(-2.0, 3.0)]
    );
    assert_eq!(
        map(InputEvent::button_down(PointerButton::Back)),
        vec![Notify::Button(278, true)]
    );
    assert_eq!(
        map(InputEvent::button_up(PointerButton::Forward)),
        vec![Notify::Button(277, false)]
    );
    assert!(notifications(InputEvent::precise_wheel(9000, 0), (1, 1), (1, 1), (0, 0)).is_err());
}

#[test]
fn a_declined_dialog_is_a_denial_and_any_other_failure_is_not() {
    use ashpd::desktop::ResponseError;
    assert!(matches!(
        grant_error(&ashpd::Error::Response(ResponseError::Cancelled)),
        Error::CaptureDenied(_)
    ));
    // Cancellation from here and an incomplete grant both answer `Other`.
    assert!(matches!(
        grant_error(&ashpd::Error::Response(ResponseError::Other)),
        Error::CaptureRejected(_)
    ));
}
