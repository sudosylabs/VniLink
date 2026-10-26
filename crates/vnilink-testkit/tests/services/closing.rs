//! How a service ends and what its end names: its owner, its peer, the
//! parent's loss, shutdown, a revoked grant, idle expiry, a removed
//! backend, and a close that races a backend still starting.

use expect_test::expect;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use vnilink::camera::{CameraBackend, CameraConfig, CameraDevice, CameraSource, HostCameraFactory};
use vnilink::power::PowerPolicy;
use vnilink::{
    usage, BackendStartup, CloseCause, CoreEvent, Error, InputEvent, PointerButton, ServiceKind,
    ServicePhase, SessionPhase,
};
use vnilink_testkit::camera::{config, CAMERA};
use vnilink_testkit::desktop::{keyframe, InputLog};
use vnilink_testkit::{traced, wait_for, Engine, Pair, TestResult, DEFAULT_WAIT};

use super::support::{
    finished, pair_for_control, phase, reaches, redialed, wait_hosted_close, wait_record,
    whole_disk,
};

/// The controller names each close: a service that followed its connection
/// across a redial and one on a connection that closed both name the
/// parent's loss, and shutting down names itself on the event stream.
#[test]
fn every_close_names_its_cause_on_the_controller() -> TestResult {
    let trace = traced();
    let controller = Engine::start("controller");
    let host_a = Engine::start("host-a");
    let host_b = Engine::start("host-b");
    let core = &controller.core;
    // The record follows its connection across the redial; it adopts the
    // new generation as the connection establishes, so a later close names
    // the parent's loss. Replaced covers only the race before adoption,
    // which no ordered sequence reaches.
    pair_for_control(&host_a, &controller)?;
    let session = controller.connect_control(&host_a)?;
    let follower =
        core.attach_controller_service(session.id, session.generation, ServiceKind::Files)?;
    core.drop_controller_link(session.id)?;
    redialed(&controller, &session);
    core.disconnect_session(session.id)?;
    reaches(&controller, session.id, SessionPhase::Closed);
    let followed = core.controller_service(follower.id)?;
    assert!(
        followed.generation > session.generation,
        "the record followed its connection across the redial"
    );
    assert_eq!(
        followed.phase,
        ServicePhase::Closed(CloseCause::ParentLoss),
        "a followed record closes with its parent"
    );
    // The record shares the closing connection's generation.
    pair_for_control(&host_b, &controller)?;
    let session = controller.connect_control(&host_b)?;
    let orphaned =
        core.attach_controller_service(session.id, session.generation, ServiceKind::Desktop)?;
    core.disconnect_session(session.id)?;
    reaches(&controller, session.id, SessionPhase::Closed);
    assert_eq!(
        phase(&controller, &orphaned),
        ServicePhase::Closed(CloseCause::ParentLoss),
        "a service closes with its connection"
    );
    // Shutdown names itself on the event stream.
    let live = Engine::start("live");
    let last_host = Engine::start("last-host");
    pair_for_control(&last_host, &live)?;
    let session = live.connect_control(&last_host)?;
    let owned =
        live.core
            .attach_controller_service(session.id, session.generation, ServiceKind::Files)?;
    let events = live.core.subscribe()?;
    live.core.shutdown();
    let mut shut_down = Vec::new();
    while let Some(event) = events.try_recv() {
        if let CoreEvent::ServiceChanged { service } = event {
            if service.id == owned.id {
                shut_down.push(service.phase);
            }
        }
    }
    assert!(
        !shut_down.is_empty(),
        "shutting down publishes the service's close"
    );
    assert!(
        shut_down
            .iter()
            .all(|phase| *phase == ServicePhase::Closed(CloseCause::Shutdown)),
        "shutting down names itself: {shut_down:?}"
    );
    expect![[r#"
        controller:
          service: service phase from=active generation=1 kind=Files service=1 to=suspended
          service: service phase from=suspended generation=3 kind=Files service=1 to=active
          service: service phase from=active generation=3 kind=Files service=1 to=closed: ParentLoss
          service: service phase from=active generation=1 kind=Desktop service=2 to=closed: ParentLoss
        live:
          service: service phase from=active generation=1 kind=Files service=1 to=closed: Shutdown
    "#]].assert_eq(&trace.story(&["service"]));
    Ok(())
}

/// The host withdrawing Files closes its hosted Files as revoked, and an
/// idle Desktop with no input expires as idle while its connection stays.
#[test]
fn a_revoked_grant_and_idle_expiry_name_their_causes() -> TestResult {
    let trace = traced();
    let pair = Pair::for_services(&[ServiceKind::Files]);
    let (host, controller) = (&pair.host.core, &pair.controller.core);
    whole_disk(&pair.host)?;
    InputLog::install(&pair.host, (1920, 1080))?;
    let parent = pair.connect_services();
    let (_, _pipe) = pair.controller.open_files(&parent)?;
    wait_for("the host to serve Files", DEFAULT_WAIT, || {
        pair.host
            .hosted(Some(ServiceKind::Files))
            .iter()
            .any(|service| service.phase == ServicePhase::Active)
            .then_some(())
    });
    // The event proves the cause: the serve task may remove the record
    // before any read sees its finished phase.
    let host_events = host.subscribe()?;
    host.set_service_sharing(ServiceKind::Files, false)?;
    wait_hosted_close(&host_events, ServiceKind::Files, CloseCause::Revoked);
    wait_for("the host to finish its Files", DEFAULT_WAIT, || {
        pair.host
            .hosted(Some(ServiceKind::Files))
            .iter()
            .all(|service| finished(&service.phase))
            .then_some(())
    });

    // An idle desktop with no input expires while the connection stays.
    let desktop_owner = pair.attach(&parent, ServiceKind::Desktop);
    let _desktop = controller.open_desktop_service(desktop_owner.id, desktop_owner.generation)?;
    controller.set_power_policy(PowerPolicy {
        outgoing_idle_secs: 1,
        ..Default::default()
    })?;
    wait_for("the idle Desktop to expire", DEFAULT_WAIT, || {
        finished(&phase(&pair.controller, &desktop_owner)).then_some(())
    });
    assert_eq!(
        phase(&pair.controller, &desktop_owner),
        ServicePhase::Closed(CloseCause::IdleExpired),
        "an idle Desktop expires as idle"
    );
    assert_eq!(
        controller.controller_session(parent.id)?.phase,
        SessionPhase::Active,
        "an idle Desktop's expiry leaves the connection"
    );
    wait_record(
        &trace,
        "host",
        "service phase",
        &[("kind", "Desktop"), ("to", "closed: PeerClose")],
    );
    expect![[r#"
        controller:
          service: service phase from=active generation=1 kind=Files service=1 to=closed: PeerClose
          service: service phase from=active generation=1 kind=Desktop service=2 to=closed: IdleExpired
        host:
          hosting: service opened connection=1 kind=Some(Files) service=1
          service: service phase from=active generation=1 kind=Files service=1 to=closed: Revoked
          hosting: service released service=1
          service: service phase from=active generation=1 kind=Desktop service=2 to=closed: PeerClose
    "#]].assert_eq(&trace.story(&["service", "hosting"]));
    Ok(())
}

/// Removing the host's shell backend closes its hosted Terminal as the
/// device lost.
#[cfg(unix)]
#[test]
fn removing_a_backend_closes_its_hosted_services_as_device_loss() -> TestResult {
    let trace = traced();
    let pair = Pair::for_services(&[ServiceKind::Terminal]);
    let host = &pair.host.core;
    host.set_terminal_factory(Some(vnilink_testkit::terminal::shell(&pair.host.root())))?;
    let parent = pair.connect_services();
    let owner = pair.attach(&parent, ServiceKind::Terminal);
    let _terminal = pair.controller.core.open_terminal_service(
        owner.id,
        owner.generation,
        vnilink_testkit::terminal::SIZE,
    )?;
    wait_for("the host to serve Terminal", DEFAULT_WAIT, || {
        pair.host
            .hosted(Some(ServiceKind::Terminal))
            .iter()
            .any(|service| service.phase == ServicePhase::Active)
            .then_some(())
    });
    // The event proves the cause: the serve task may remove the record
    // before any read sees its finished phase.
    let host_events = host.subscribe()?;
    host.set_terminal_factory(None)?;
    wait_hosted_close(&host_events, ServiceKind::Terminal, CloseCause::DeviceLoss);
    trace.wait_for_records("service released", 1);
    expect![[r#"
        host:
          hosting: service opened connection=1 kind=Some(Terminal) service=1
          hosting: service released service=1
    "#]]
    .assert_eq(&trace.story(&["hosting"]));
    Ok(())
}

/// A camera backend that never looks at its startup and blocks until
/// released.
struct Stall {
    entered: Sender<()>,
    release: Mutex<Receiver<()>>,
    late: AtomicUsize,
}

impl HostCameraFactory for Stall {
    fn devices(&self) -> Result<Vec<CameraDevice>, Error> {
        Ok(vec![CameraDevice {
            id: CAMERA.into(),
            name: "Stall".into(),
        }])
    }

    fn open(
        &self,
        requested: CameraConfig,
        _startup: BackendStartup,
    ) -> Result<CameraBackend, Error> {
        // Hostile backend: it never polls `startup`, so only the engine's
        // own cancellation can settle the open.
        self.entered.send(()).expect("the test waits for the open");
        self.release
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .recv()
            .expect("the test releases the open");
        self.late.fetch_add(1, Ordering::SeqCst);
        CameraBackend::new(requested, CameraSource::new())
    }
}

/// Closing a Camera whose backend is stuck opening settles the open at
/// once, and the backend that finishes later is dropped, never served.
#[test]
fn a_stalled_camera_open_cancels_cleanly_and_adopts_nothing_late() -> TestResult {
    let trace = traced();
    let pair = Pair::for_services(&[ServiceKind::Camera]);
    let controller = &pair.controller.core;
    let (entered, entering) = std::sync::mpsc::channel();
    let (release, released) = std::sync::mpsc::channel();
    let stall = Arc::new(Stall {
        entered,
        release: Mutex::new(released),
        late: AtomicUsize::new(0),
    });
    pair.host
        .core
        .set_camera_factory(Some(Arc::clone(&stall) as _))?;
    let parent = pair.connect_services();
    let owner = pair.attach(&parent, ServiceKind::Camera);
    let (elapsed, outcome) = std::thread::scope(|scope| -> Result<_, Error> {
        let opener =
            scope.spawn(|| controller.open_camera_service(owner.id, owner.generation, config()));
        entering
            .recv_timeout(Duration::from_secs(5))
            .expect("the backend starts opening");
        let started = Instant::now();
        controller.close_controller_service(owner.id, owner.generation)?;
        Ok((started.elapsed(), opener.join().expect("the open returns")))
    })?;
    assert!(
        elapsed < Duration::from_secs(4),
        "cancelling settles the open before the exchange times out: {elapsed:?}"
    );
    assert!(outcome.is_err(), "the cancelled open fails");
    assert_eq!(
        phase(&pair.controller, &owner),
        ServicePhase::Closed(CloseCause::OwnerClose),
        "the Camera closed by its owner"
    );
    // The late backend completes into a dropped task: counted, never
    // admitted, and the host's record stays finished. It is let go once the
    // host has seen its viewer leave, which a backend finishing first would
    // race.
    trace.wait_for_records("camera not opened", 1);
    release.send(())?;
    wait_for("the late backend to finish", DEFAULT_WAIT, || {
        (stall.late.load(Ordering::SeqCst) == 1).then_some(())
    });
    wait_for("the host's Camera to finish", DEFAULT_WAIT, || {
        pair.host
            .hosted(Some(ServiceKind::Camera))
            .iter()
            .all(|service| finished(&service.phase))
            .then_some(())
    });
    assert!(
        pair.host
            .hosted(Some(ServiceKind::Camera))
            .iter()
            .all(|service| service.phase != ServicePhase::Active),
        "the late backend is never served"
    );
    trace.wait_for_records("service released", 1);
    // The host learns its viewer closed the camera while it was opening.
    expect![[r#"
        controller:
          service: service phase from=active generation=1 kind=Camera service=1 to=closed: OwnerClose
        host:
          hosting: service opened connection=1 kind=Some(Camera) service=1
          camera: camera not opened error=no session service=1
          camera: camera stream ended ended=viewer closed service=1
          hosting: service closed kind=Camera phase=closed: PeerClose service=1
          hosting: service released service=1
    "#]].assert_eq(&trace.story(&["camera", "service", "hosting"]));
    Ok(())
}

/// How a test ends an adopted Desktop.
#[derive(Clone, Copy, Debug)]
enum DesktopClose {
    HandleClose,
    OwnerClose,
    HostClose,
    HostStopsSharing,
    HandleDropped,
}

/// Every way an adopted Desktop ends releases the keys and buttons it held,
/// stops its pictures and input, names who closed it, and leaves the
/// connection and its Files service; a redial after it stays neutral and
/// keeps the first close's cause.
#[test]
fn an_adopted_desktop_close_releases_input_and_keeps_files_for_every_close() -> TestResult {
    for close in [
        DesktopClose::HandleClose,
        DesktopClose::OwnerClose,
        DesktopClose::HostClose,
        DesktopClose::HostStopsSharing,
        DesktopClose::HandleDropped,
    ] {
        close_adopted_desktop(close)?;
    }
    Ok(())
}

fn close_adopted_desktop(close: DesktopClose) -> TestResult {
    let _trace = traced();
    let pair = Pair::for_desktop();
    let (host, controller) = (&pair.host.core, &pair.controller.core);
    let (root, folder) = whole_disk(&pair.host)?;
    let applied = InputLog::install(&pair.host, (1920, 1080))?;
    host.set_service_sharing(ServiceKind::Files, true)?;
    let session = controller.connect(&pair.host.endpoint())?;
    let incoming = host.snapshot()?.incoming.ok_or("the host's session")?;
    let desktop = pair.attach(&session, ServiceKind::Desktop);
    let handle = controller.open_desktop_service(desktop.id, desktop.generation)?;
    assert!(
        matches!(
            controller.open_desktop_service(desktop.id, desktop.generation),
            Err(Error::Busy)
        ),
        "{close:?}: a Desktop owner opens once"
    );
    let (files, pipe) = pair.controller.open_files(&session)?;
    let subscription = controller.subscribe_session_video(session.id, session.generation)?;
    assert!(
        host.host_desktop_active(incoming.id, incoming.generation)?,
        "{close:?}: the host's desktop is active"
    );
    host.publish_video(incoming.id, keyframe(7, vec![7]))?;
    assert_eq!(
        subscription
            .recv_timeout(Duration::from_secs(5))
            .ok_or("a picture")?
            .payload,
        vec![7],
        "{close:?}: the Desktop carries the host's pictures"
    );
    let key = InputEvent::key_down(usage::A);
    let button = InputEvent::button_down(PointerButton::Left);
    controller.submit_input(session.id, session.generation, key)?;
    controller.submit_input(session.id, session.generation, button)?;
    applied.exactly(2);
    let remote = pair
        .host
        .hosted_for(ServiceKind::Desktop, &pair.controller)
        .ok_or("the host serves the Desktop")?;
    match close {
        DesktopClose::HandleClose => handle.close(),
        DesktopClose::OwnerClose => {
            controller.close_controller_service(desktop.id, desktop.generation)?
        }
        DesktopClose::HostClose => host.close_hosted_service(remote.id, remote.generation)?,
        DesktopClose::HostStopsSharing => host.set_service_sharing(ServiceKind::Desktop, false)?,
        DesktopClose::HandleDropped => drop(handle),
    }
    let expected = match close {
        DesktopClose::HostClose | DesktopClose::HostStopsSharing => {
            ServicePhase::Closed(CloseCause::PeerClose)
        }
        _ => ServicePhase::Closed(CloseCause::OwnerClose),
    };
    wait_for(
        &format!("{close:?}: the Desktop to close as {expected:?}"),
        DEFAULT_WAIT,
        || (phase(&pair.controller, &desktop) == expected).then_some(()),
    );
    assert_eq!(
        applied.wait_for(4),
        vec![
            key,
            button,
            InputEvent::button_up(PointerButton::Left),
            InputEvent::key_up(usage::A)
        ],
        "{close:?}: the close releases what the Desktop held, latest first"
    );
    assert!(
        matches!(
            controller.submit_input(session.id, session.generation, key),
            Err(Error::Unsupported(_))
        ),
        "{close:?}: a closed Desktop takes no input"
    );
    assert!(
        matches!(
            host.publish_video(incoming.id, keyframe(8, vec![8])),
            Err(Error::NoSession)
        ),
        "{close:?}: a closed Desktop takes no pictures"
    );
    assert!(
        subscription.try_recv().is_none(),
        "{close:?}: no picture follows the close"
    );
    assert!(
        controller.controller_session(session.id)?.quality.is_none(),
        "{close:?}: the controller's connection has no picture quality"
    );
    let incoming_now = host.snapshot()?.incoming.ok_or("the host's session")?;
    assert!(
        incoming_now.quality.is_none(),
        "{close:?}: the host's session has no picture quality"
    );
    assert!(
        !host.host_desktop_active(incoming.id, incoming.generation)?,
        "{close:?}: the host's desktop is no longer active"
    );
    assert_eq!(
        controller.controller_session(session.id)?.phase,
        SessionPhase::Active,
        "{close:?}: the controller's connection stays"
    );
    assert_eq!(
        incoming_now.phase,
        SessionPhase::Active,
        "{close:?}: the host's session stays"
    );
    pipe.mkdir(root, &folder, "after-desktop-close")?;
    assert_eq!(
        pipe.list(root, &folder, 0, 20)?.entries[0].name,
        "after-desktop-close",
        "{close:?}: Files works after the Desktop closed"
    );
    assert!(!pipe.is_cancelled(), "{close:?}: Files is not cancelled");
    assert_eq!(
        phase(&pair.controller, &files),
        ServicePhase::Active,
        "{close:?}: Files stays active"
    );
    assert!(
        !controller
            .remote_service_support(session.id, session.generation)?
            .desktop,
        "{close:?}: the connection no longer offers its Desktop"
    );
    // A redial after the Desktop closed stays neutral rather than resuming
    // capture.
    controller.drop_controller_link(session.id)?;
    let fresh = redialed(&pair.controller, &session);
    assert!(
        fresh.quality.is_none(),
        "{close:?}: the redial has no picture quality"
    );
    assert_eq!(
        (fresh.monitor_width, fresh.monitor_height),
        (0, 0),
        "{close:?}: the redial has no monitor"
    );
    assert_eq!(
        phase(&pair.controller, &desktop),
        expected,
        "{close:?}: the redial keeps the first close's cause"
    );
    let files = controller.controller_service(files.id)?;
    let fresh_pipe = controller.open_files_service(files.id, files.generation)?;
    pipe.close();
    assert!(
        !fresh_pipe.is_cancelled(),
        "{close:?}: closing the old pipe leaves the new one"
    );
    fresh_pipe.mkdir(root, &folder, "after-reconnect")?;
    Ok(())
}
