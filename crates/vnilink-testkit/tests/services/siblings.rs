//! Services side by side on one connection: each has its own owner, work
//! and close, while they share the connection that carries them.

use expect_test::expect;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Barrier};
use std::time::Duration;

use vnilink::terminal::TerminalEvent;
use vnilink::{
    CloseCause, Error, Grants, ServiceInfo, ServiceKind, ServicePhase, SessionPhase,
    MAX_SERVICE_RECORDS,
};
use vnilink_testkit::camera::{config, StandInCameras};
use vnilink_testkit::desktop::{InputLog, MonitorCapture};
use vnilink_testkit::events::service_changes;
use vnilink_testkit::files::whole_disk;
use vnilink_testkit::terminal::{shell, SIZE};
use vnilink_testkit::{traced, wait_for, Pair, TestResult, DEFAULT_WAIT};

/// Work a backend owns, which says when it was dropped.
struct Resource(Arc<AtomicBool>);

impl Drop for Resource {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

/// A Desktop and a Files service share their connection but not their
/// cancellation: closing the Desktop ends the work it ran and drops what
/// that work held, while Files, the connection and the host's session carry
/// on; a closed service is replaced under a new id, forgotten once closed,
/// and only disconnecting cancels the rest at once.
#[test]
fn closing_a_service_ends_its_own_work_and_leaves_its_siblings() -> TestResult {
    let trace = traced();
    let pair = Pair::for_desktop();
    let (host, controller) = (&pair.host.core, &pair.controller.core);
    let session = pair.connect_control();
    let events = controller.subscribe()?;
    let screen = pair.attach(&session, ServiceKind::Desktop);
    let files = pair.attach(&session, ServiceKind::Files);
    let mut screen_work = controller.service_lease(screen.id, screen.generation)?;
    let files_work = controller.service_lease(files.id, files.generation)?;
    assert_eq!(
        controller.service_connection(files.id, files.generation)?,
        session,
        "a service is carried by the connection it was attached to"
    );
    assert!(
        matches!(
            controller.attach_controller_service(
                session.id,
                session.generation,
                ServiceKind::Desktop
            ),
            Err(Error::Busy)
        ),
        "a connection has one open Desktop"
    );
    assert!(
        matches!(
            controller.forget_controller_service(screen.id),
            Err(Error::Busy)
        ),
        "an open service cannot be forgotten"
    );

    // A backend awaiting I/O drops its owned resources when its service closes.
    let dropped = Arc::new(AtomicBool::new(false));
    let resource = Resource(Arc::clone(&dropped));
    let (ready_tx, ready_rx) = std::sync::mpsc::channel();
    let worker = std::thread::spawn(move || {
        tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("a runtime")
            .block_on(screen_work.run(async move {
                let _resource = resource;
                ready_tx.send(()).expect("the test waits for the work");
                std::future::pending::<Result<(), Error>>().await
            }))
    });
    ready_rx.recv_timeout(Duration::from_secs(2))?;
    controller.close_controller_service(screen.id, screen.generation)?;
    assert!(
        matches!(worker.join().expect("the work ends"), Err(Error::NoSession)),
        "closing a service ends the work it ran"
    );
    assert!(
        dropped.load(Ordering::SeqCst),
        "the ended work dropped what it held"
    );
    assert!(
        !files_work.is_cancelled(),
        "closing the Desktop leaves Files"
    );
    assert_eq!(
        controller.controller_session(session.id)?,
        session,
        "closing a service leaves its connection as it was"
    );
    assert_eq!(
        host.snapshot()?.incoming.ok_or("the host's session")?.phase,
        SessionPhase::Active,
        "closing a service leaves the host's session active"
    );
    assert!(
        matches!(
            controller.service_connection(screen.id, screen.generation),
            Err(Error::NoSession)
        ),
        "a closed service has no connection"
    );
    controller.close_controller_service(screen.id, screen.generation)?;
    let replacement = pair.attach(&session, ServiceKind::Desktop);
    assert_ne!(
        replacement.id, screen.id,
        "a replacement service has its own id"
    );
    controller.forget_controller_service(screen.id)?;
    assert!(
        matches!(
            controller.controller_service(screen.id),
            Err(Error::NoSession)
        ),
        "a forgotten service has no record"
    );
    assert_eq!(
        service_changes(&events),
        vec![
            screen.clone(),
            files.clone(),
            ServiceInfo {
                phase: ServicePhase::Closed(CloseCause::OwnerClose),
                ..screen
            },
            replacement
        ],
        "each service's opening and its one close are published, in order"
    );
    // Disconnecting closes the connection's services together, in no order.
    controller.disconnect_session(session.id)?;
    assert!(
        files_work.is_cancelled(),
        "disconnect must cancel immediately, without polling"
    );
    expect![[r#"
        controller:
          service: service phase from=active generation=1 kind=Desktop service=1 to=closed: OwnerClose
          service: service phase from=active generation=1 kind=Desktop service=3 to=closed: ParentLoss
          service: service phase from=active generation=1 kind=Files service=2 to=closed: ParentLoss
    "#]].assert_eq(&trace.concurrent_story(&["service"]));
    Ok(())
}

/// A connection that offers only Files refuses every other kind without
/// changing; its records are bounded, and forgetting one frees room for a
/// new one under a new id.
#[test]
fn unsupported_kinds_and_the_record_limit_leave_the_connection_as_it_was() -> TestResult {
    let _trace = traced();
    let pair = Pair::for_desktop();
    let (host, controller) = (&pair.host.core, &pair.controller.core);
    host.set_desktop_sharing(Grants {
        transfer: true,
        ..Grants::default()
    })?;
    let pending = controller.begin_connect_files(&pair.host.endpoint())?;
    let session = pair.controller.wait_phase(pending.id, SessionPhase::Active);
    let support = controller.controller_service_support(session.id)?;
    assert!(
        !support.desktop && support.files && !support.terminal && !support.camera,
        "a files connection supports Files alone: {support:?}"
    );
    for kind in [
        ServiceKind::Desktop,
        ServiceKind::Terminal,
        ServiceKind::Camera,
    ] {
        assert!(
            matches!(
                controller.attach_controller_service(session.id, session.generation, kind),
                Err(Error::Unsupported(_))
            ),
            "a files connection refuses {kind:?}"
        );
    }
    assert!(
        controller.controller_services()?.is_empty(),
        "a refused kind leaves no record"
    );
    for _ in 0..MAX_SERVICE_RECORDS {
        let service = pair.attach(&session, ServiceKind::Files);
        controller.close_controller_service(service.id, service.generation)?;
    }
    assert!(
        matches!(
            controller.attach_controller_service(
                session.id,
                session.generation,
                ServiceKind::Files
            ),
            Err(Error::Busy)
        ),
        "closed records count against the limit until forgotten"
    );
    controller.forget_controller_service(1)?;
    let service = pair.attach(&session, ServiceKind::Files);
    assert!(
        service.id > MAX_SERVICE_RECORDS as u64,
        "a new record never reuses a forgotten id"
    );
    assert_eq!(
        controller.controller_session(session.id)?,
        session,
        "refusals and the limit leave the connection as it was"
    );
    Ok(())
}

/// Desktop, Files, Terminal and Camera open on one services connection,
/// each once; closing them one by one ends each on the host alone while
/// the connection stays active and the others carry on.
#[cfg(unix)]
#[test]
fn all_services_share_one_connection_and_close_independently() -> TestResult {
    let trace = traced();
    let pair = Pair::for_services(&[
        ServiceKind::Desktop,
        ServiceKind::Files,
        ServiceKind::Terminal,
        ServiceKind::Camera,
    ]);
    let (host, controller) = (&pair.host.core, &pair.controller.core);
    whole_disk(&pair.host, "")?;
    InputLog::install(&pair.host, (1920, 1080))?;
    host.set_capture(Box::new(MonitorCapture::new((1920, 1080))))?;
    host.set_terminal_factory(Some(shell(&pair.host.root())))?;
    let cameras = Arc::new(StandInCameras::default());
    host.set_camera_factory(Some(cameras))?;
    let parent = pair.connect_services();

    let desktop_owner = pair.attach(&parent, ServiceKind::Desktop);
    let desktop = controller.open_desktop_service(desktop_owner.id, desktop_owner.generation)?;
    let files_owner = pair.attach(&parent, ServiceKind::Files);
    let files = controller.open_files_service(files_owner.id, files_owner.generation)?;
    let terminal_owner = pair.attach(&parent, ServiceKind::Terminal);
    let terminal =
        controller.open_terminal_service(terminal_owner.id, terminal_owner.generation, SIZE)?;
    let camera_owner = pair.attach(&parent, ServiceKind::Camera);
    let camera =
        controller.open_camera_service(camera_owner.id, camera_owner.generation, config())?;

    assert_eq!(
        controller.controller_services()?.len(),
        4,
        "four owners on one connection"
    );
    assert_eq!(pair.host.hosted(None).len(), 4, "the host serves all four");
    assert!(
        matches!(
            controller.open_desktop_service(desktop_owner.id, desktop_owner.generation),
            Err(Error::Busy)
        ),
        "a Desktop owner opens once"
    );
    assert!(
        matches!(
            controller.open_files_service(files_owner.id, files_owner.generation),
            Err(Error::Busy)
        ),
        "a Files owner opens once"
    );
    assert!(
        matches!(
            controller.open_terminal_service(terminal_owner.id, terminal_owner.generation, SIZE),
            Err(Error::Busy)
        ),
        "a Terminal owner opens once"
    );
    assert!(
        matches!(
            controller.open_camera_service(camera_owner.id, camera_owner.generation, config()),
            Err(Error::Busy)
        ),
        "a Camera owner opens once"
    );

    desktop.close();
    pair.host.wait_released(Some(ServiceKind::Desktop));
    assert_eq!(
        pair.host.hosted(None).len(),
        3,
        "closing the Desktop ends it alone on the host"
    );
    assert_eq!(
        controller.controller_service(desktop_owner.id)?.phase,
        ServicePhase::Closed(CloseCause::OwnerClose),
        "the Desktop closed by its owner"
    );
    assert_eq!(
        controller.controller_session(parent.id)?.phase,
        SessionPhase::Active,
        "closing the Desktop leaves the connection"
    );
    assert!(
        !files.is_cancelled() && !terminal.is_cancelled() && !camera.is_cancelled(),
        "closing the Desktop leaves its siblings"
    );
    assert!(
        desktop.subscribe_video().is_err(),
        "a closed Desktop handle cannot act"
    );

    files.close();
    pair.host.wait_released(Some(ServiceKind::Files));
    assert_eq!(
        pair.host.hosted(None).len(),
        2,
        "closing Files ends it alone on the host"
    );
    assert!(
        !terminal.is_cancelled() && !camera.is_cancelled(),
        "closing Files leaves Terminal and Camera"
    );
    assert_eq!(
        controller.controller_session(parent.id)?.phase,
        SessionPhase::Active,
        "closing Files leaves the connection"
    );

    terminal.close();
    pair.host.wait_released(Some(ServiceKind::Terminal));
    assert_eq!(
        pair.host.hosted(None).len(),
        1,
        "closing Terminal ends it alone on the host"
    );
    assert!(!camera.is_cancelled(), "closing Terminal leaves Camera");

    camera.close();
    pair.host.wait_released(None);
    assert_eq!(
        controller.controller_session(parent.id)?.phase,
        SessionPhase::Active,
        "a connection outlives all its services"
    );
    let services = controller.controller_services()?;
    assert_eq!(services.len(), 4, "the controller keeps the four records");
    for service in services {
        assert_eq!(
            service.phase,
            ServicePhase::Closed(CloseCause::OwnerClose),
            "each service closed by its owner: {service:?}"
        );
    }
    // The host releases each service it served; the Desktop's own record
    // is its phase.
    trace.wait_for_records("service released", 3);
    expect![[r#"
        controller:
          service: service phase from=active generation=1 kind=Desktop service=1 to=closed: OwnerClose
          service: service phase from=active generation=1 kind=Files service=2 to=closed: OwnerClose
          service: service phase from=active generation=1 kind=Terminal service=3 to=closed: OwnerClose
          service: service phase from=active generation=1 kind=Camera service=4 to=closed: OwnerClose
        host:
          hosting: service opened connection=1 kind=Some(Files) service=2
          hosting: service opened connection=1 kind=Some(Terminal) service=3
          hosting: service opened connection=1 kind=Some(Camera) service=4
          hosting: service refused connection=1 kind=Files status=busy
          service: service phase from=active generation=1 kind=Desktop service=1 to=closed: PeerClose
          hosting: service closed kind=Files phase=closed: PeerClose service=2
          hosting: service released service=2
          hosting: service closed kind=Terminal phase=closed: PeerClose service=3
          hosting: service released service=3
          hosting: service closed kind=Camera phase=closed: PeerClose service=4
          hosting: service released service=4
    "#]].assert_eq(&trace.story(&["service", "hosting"]));
    Ok(())
}

/// A host with no capture, desktop or encoder still opens Files, Terminal
/// and Camera on a services connection; only Desktop is refused, and that
/// refusal fails nothing else.
#[cfg(unix)]
#[test]
fn files_terminal_and_camera_start_without_capture_or_a_desktop_decoder() -> TestResult {
    let trace = traced();
    let pair = Pair::for_services(&[
        ServiceKind::Files,
        ServiceKind::Terminal,
        ServiceKind::Camera,
    ]);
    let (host, controller) = (&pair.host.core, &pair.controller.core);
    let (root, folder) = whole_disk(&pair.host, "")?;
    host.set_terminal_factory(Some(shell(&pair.host.root())))?;
    // No capture, no desktop adapter, no H.264 encoder on the host.
    host.set_video_codecs(vnilink::VideoCodecCaps::default())?;
    host.set_camera_factory(Some(Arc::new(StandInCameras::default())))?;

    let parent = pair.connect_services();
    assert_eq!(
        (parent.monitor_width, parent.monitor_height),
        (0, 0),
        "a services connection has no monitor"
    );
    assert!(
        parent.quality.is_none(),
        "a services connection has no picture quality"
    );

    let files_owner = pair.attach(&parent, ServiceKind::Files);
    let files = controller.open_files_service(files_owner.id, files_owner.generation)?;
    files.mkdir(root, &folder, "no-capture")?;
    assert!(
        pair.host.root().join("no-capture").is_dir(),
        "Files changes the host's disk without capture"
    );
    let terminal_owner = pair.attach(&parent, ServiceKind::Terminal);
    let terminal =
        controller.open_terminal_service(terminal_owner.id, terminal_owner.generation, SIZE)?;
    assert!(!terminal.is_cancelled(), "Terminal opens without capture");
    let camera_owner = pair.attach(&parent, ServiceKind::Camera);
    let camera =
        controller.open_camera_service(camera_owner.id, camera_owner.generation, config())?;
    assert!(!camera.is_cancelled(), "Camera opens without a display");

    let desktop_owner = pair.attach(&parent, ServiceKind::Desktop);
    assert!(
        matches!(
            controller.open_desktop_service(desktop_owner.id, desktop_owner.generation),
            Err(Error::Unsupported(_))
        ),
        "Desktop needs a real backend"
    );
    assert!(
        pair.host.hosted(Some(ServiceKind::Desktop)).is_empty(),
        "the refused Desktop is not served"
    );
    assert_eq!(
        controller.controller_session(parent.id)?.phase,
        SessionPhase::Active,
        "the refused Desktop leaves the connection"
    );
    assert!(!files.is_cancelled(), "the refused Desktop leaves Files");
    expect![[r#"
        controller:
          session: session phase from=connecting generation=1 role=Controller session=1 to=active
        host:
          hosting: service opened connection=1 kind=Some(Files) service=1
          hosting: service opened connection=1 kind=Some(Terminal) service=2
          hosting: service opened connection=1 kind=Some(Camera) service=3
    "#]]
    .assert_eq(&trace.story(&["service", "hosting", "session"]));
    Ok(())
}

/// A host whose capture and camera are ready never starts either for a
/// files connection or for a terminal on a services connection.
#[cfg(unix)]
#[test]
fn files_only_and_terminal_only_connections_never_request_capture() -> TestResult {
    let trace = traced();
    let pair = Pair::for_desktop();
    let (host, controller) = (&pair.host.core, &pair.controller.core);
    // Capture and camera backends exist, so an empty target list proves the
    // core never asked, not that no backend was registered.
    host.set_capture(Box::new(MonitorCapture::new((1920, 1080))))?;
    let cameras = Arc::new(StandInCameras::default());
    host.set_camera_factory(Some(Arc::clone(&cameras) as _))?;
    host.set_terminal_factory(Some(shell(&pair.host.root())))?;
    host.set_desktop_sharing(Grants {
        transfer: true,
        ..Grants::default()
    })?;

    // Files-only: the session carries jobs, never video.
    let pending = controller.begin_connect_files(&pair.host.endpoint())?;
    let files = pair.controller.wait_phase(pending.id, SessionPhase::Active);
    assert!(
        host.video_targets()?.is_empty(),
        "a files connection asks for no capture"
    );
    assert!(
        matches!(
            controller.subscribe_session_video(files.id, files.generation),
            Err(Error::Unsupported(_))
        ),
        "a files connection carries no video"
    );
    assert!(
        pair.host.hosted(Some(ServiceKind::Desktop)).is_empty(),
        "a files connection serves no Desktop"
    );
    controller.disconnect_session(files.id)?;
    pair.controller.wait_phase(files.id, SessionPhase::Closed);

    // Terminal-only: a live shell with output, still no capture target.
    host.set_service_sharing(ServiceKind::Terminal, true)?;
    let parent = pair.connect_services();
    let owner = pair.attach(&parent, ServiceKind::Terminal);
    let terminal = controller.open_terminal_service(owner.id, owner.generation, SIZE)?;
    terminal.input(b"echo capture-never-requested\n")?;
    let mut transcript = Vec::new();
    wait_for("the shell's echo", DEFAULT_WAIT, || {
        while let Some(event) = terminal.poll() {
            if let TerminalEvent::Output(bytes) = event {
                transcript.extend_from_slice(&bytes);
            }
        }
        transcript
            .windows(b"capture-never-requested".len())
            .any(|window| window == b"capture-never-requested")
            .then_some(())
    });
    assert!(
        host.video_targets()?.is_empty(),
        "a terminal asks for no capture"
    );
    assert!(
        pair.host
            .hosted(Some(ServiceKind::Desktop))
            .iter()
            .all(|service| service.phase != ServicePhase::Active),
        "a terminal serves no Desktop"
    );
    assert!(
        cameras
            .sources
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .is_empty(),
        "no camera was opened"
    );
    terminal.close();
    expect![[r#"
        controller:
          session: session phase from=connecting generation=1 role=Controller session=1 to=active
          session: session phase from=active generation=1 role=Controller session=1 to=stopping
          session: session phase from=stopping generation=1 role=Controller session=1 to=closed
          session: session phase from=connecting generation=3 role=Controller session=2 to=active
        host:
          session: session phase from=active generation=1 role=Host session=1 to=closed
          hosting: service opened connection=2 kind=Some(Terminal) service=1
    "#]]
    .assert_eq(&trace.story(&["service", "hosting", "session"]));
    Ok(())
}

/// Eight services connects to one host at once share one connection, and
/// connecting again reuses it without replacing the services it carries.
#[test]
fn concurrent_services_connects_share_one_connection_and_keep_its_services() -> TestResult {
    let trace = traced();
    let pair = Pair::for_desktop();
    let controller = &pair.controller.core;
    let barrier = Arc::new(Barrier::new(8));
    let workers: Vec<_> = (0..8)
        .map(|_| {
            let controller = Arc::clone(controller);
            let barrier = Arc::clone(&barrier);
            let peer = pair.host.endpoint();
            std::thread::spawn(move || {
                barrier.wait();
                controller
                    .begin_connect_services(&peer)
                    .map(|session| session.id)
            })
        })
        .collect();
    let ids = workers
        .into_iter()
        .map(|worker| worker.join().expect("a connect"))
        .collect::<Result<Vec<_>, _>>()?;
    assert!(
        ids.iter().all(|id| *id == ids[0]),
        "concurrent connects share one connection: {ids:?}"
    );
    assert_eq!(
        controller.controller_sessions()?.len(),
        1,
        "the controller keeps one connection"
    );
    let session = pair.controller.wait_phase(ids[0], SessionPhase::Active);
    let files = pair.attach(&session, ServiceKind::Files);
    let lease = controller.service_lease(files.id, files.generation)?;
    let reused = controller.begin_connect_services(&pair.host.endpoint())?;
    assert_eq!(reused, session, "connecting again reuses the connection");
    assert!(
        !lease.is_cancelled(),
        "reusing the connection keeps its services' work"
    );
    assert_eq!(
        controller.controller_service(files.id)?,
        files,
        "reusing the connection keeps its services' owners"
    );
    assert_eq!(
        pair.host.core.hosted_sessions()?.len(),
        1,
        "the host serves one connection"
    );
    expect![[r#"
        controller:
          session: session phase from=connecting generation=1 role=Controller session=1 to=active
    "#]]
    .assert_eq(&trace.story(&["service", "hosting", "session"]));
    Ok(())
}
