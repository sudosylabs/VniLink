// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! How a service ends and what its end names: its owner, its peer, the
//! parent's loss, shutdown, a revoked grant, idle expiry, a removed
//! backend, and a close that races a backend still starting.

use expect_test::expect;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Mutex};

use vnilink::camera::{CameraBackend, CameraConfig, CameraDevice, CameraSource, HostCameraFactory};
use vnilink::power::PowerPolicy;
use vnilink::{
    BackendStartup, CloseCause, CoreEvent, Error, ServiceKind, ServicePhase, SessionPhase,
};
use vnilink_testkit::camera::{config, CAMERA};
use vnilink_testkit::desktop::InputLog;
use vnilink_testkit::events::wait_hosted_close;
use vnilink_testkit::session::service_ended;
use vnilink_testkit::terminal::Hosting;
use vnilink_testkit::{traced, wait_for, Engine, Pair, TestResult, DEFAULT_WAIT};

use super::support::{check, phase, DesktopClose};

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
    host_a.pair_for_control(&controller)?;
    let session = controller.connect_services(&host_a)?;
    let follower =
        core.attach_controller_service(session.id, session.generation, ServiceKind::Files)?;
    core.drop_controller_link(session.id)?;
    controller.redialed(&session);
    core.disconnect_session(session.id)?;
    controller.wait_phase(session.id, SessionPhase::Closed);
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
    host_b.pair_for_control(&controller)?;
    let session = controller.connect_services(&host_b)?;
    let orphaned =
        core.attach_controller_service(session.id, session.generation, ServiceKind::Desktop)?;
    core.disconnect_session(session.id)?;
    controller.wait_phase(session.id, SessionPhase::Closed);
    assert_eq!(
        phase(&controller, &orphaned)?,
        ServicePhase::Closed(CloseCause::ParentLoss),
        "a service closes with its connection"
    );
    // Shutdown names itself on the event stream.
    let live = Engine::start("live");
    let last_host = Engine::start("last-host");
    last_host.pair_for_control(&live)?;
    let session = live.connect_services(&last_host)?;
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
    vnilink_testkit::assert_story(
        expect![[r#"
        controller:
          service: service phase from=active generation=1 kind=Files service=1 to=suspended
          service: service phase from=suspended generation=3 kind=Files service=1 to=active
          service: service phase from=active generation=3 kind=Files service=1 to=closed: ParentLoss
          service: service phase from=active generation=1 kind=Desktop service=2 to=closed: ParentLoss
        live:
          service: service phase from=active generation=1 kind=Files service=1 to=closed: Shutdown
    "#]],
        || trace.story(&["service"]),
    );
    Ok(())
}

/// The host withdrawing Files closes its hosted Files as revoked, and an
/// idle Desktop with no input expires as idle while its connection stays.
#[test]
fn a_revoked_grant_and_idle_expiry_name_their_causes() -> TestResult {
    let trace = traced();
    let pair = Pair::for_services(&[ServiceKind::Files]);
    let (host, controller) = (&pair.host.core, &pair.controller.core);
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
            .all(|service| service_ended(&service.phase))
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
        service_ended(&phase(&pair.controller, &desktop_owner).ok()?).then_some(())
    });
    assert_eq!(
        phase(&pair.controller, &desktop_owner)?,
        ServicePhase::Closed(CloseCause::IdleExpired),
        "an idle Desktop expires as idle"
    );
    assert_eq!(
        controller.controller_session(parent.id)?.phase,
        SessionPhase::Active,
        "an idle Desktop's expiry leaves the connection"
    );
    trace.wait_for_records_with(
        "service phase",
        &[
            ("engine", "host"),
            ("kind", "Desktop"),
            ("to", "closed: PeerClose"),
        ],
        1,
    );
    vnilink_testkit::assert_story(
        expect![[r#"
        controller:
          service: service phase from=active generation=1 kind=Files service=1 to=closed: PeerClose
          service: service phase from=active generation=1 kind=Desktop service=2 to=closed: IdleExpired
        host:
          hosting: service opened connection=1 kind=Some(Files) service=1
          service: service phase from=active generation=1 kind=Files service=1 to=closed: Revoked
          hosting: service released service=1
          service: service phase from=active generation=1 kind=Desktop service=2 to=closed: PeerClose
    "#]],
        || trace.story(&["service", "hosting"]),
    );
    Ok(())
}

/// Removing the host's shell backend closes its hosted Terminal as the
/// device lost.
#[test]
fn removing_a_backend_closes_its_hosted_services_as_device_loss() -> TestResult {
    let trace = traced();
    let hosting = Hosting::shell()?;
    let (pair, host) = (&hosting.pair, &hosting.pair.host.core);
    let _terminal = hosting.open()?;
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
    vnilink_testkit::assert_story(
        expect![[r#"
        host:
          hosting: service opened connection=1 kind=Some(Terminal) service=1
          hosting: service released service=1
    "#]],
        || trace.story(&["hosting"]),
    );
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
    let outcome = std::thread::scope(|scope| -> Result<_, Error> {
        let opener =
            scope.spawn(|| controller.open_camera_service(owner.id, owner.generation, config()));
        entering
            .recv_timeout(DEFAULT_WAIT)
            .expect("the backend starts opening");
        controller.close_controller_service(owner.id, owner.generation)?;
        Ok(opener.join().expect("the open returns"))
    })?;
    // The backend holds until the host records the open refused, below:
    // only the close can settle the open here, never the backend finishing,
    // and an open that waited out its exchange would fail as timed out.
    assert!(
        matches!(outcome, Err(Error::NoSession)),
        "closing settles a stalled open as ended, not timed out: {:?}",
        outcome.err()
    );
    assert_eq!(
        phase(&pair.controller, &owner)?,
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
            .all(|service| service_ended(&service.phase))
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
    vnilink_testkit::assert_story(
        expect![[r#"
        controller:
          service: service phase from=active generation=1 kind=Camera service=1 to=closed: OwnerClose
        host:
          hosting: service opened connection=1 kind=Some(Camera) service=1
          camera: camera not opened error=no session service=1
          camera: camera stream ended ended=viewer closed service=1
          hosting: service closed kind=Camera phase=closed: PeerClose service=1
          hosting: service released service=1
    "#]],
        || trace.story(&["camera", "service", "hosting"]),
    );
    Ok(())
}

/// A Desktop whose handle is closed releases what it held and leaves the
/// connection and Files; a redial keeps it closed by its owner.
#[test]
fn a_desktop_closed_by_its_handle_releases_what_it_held_and_keeps_files() -> TestResult {
    let trace = traced();
    check(
        &trace,
        DesktopClose::HandleClose,
        expect![[r#"
        controller:
          service: service phase from=active generation=1 kind=Desktop service=1 to=closed: OwnerClose
        host:
          hosting: service opened connection=1 kind=Some(Files) service=2
          service: service phase from=active generation=1 kind=Desktop service=1 to=closed: PeerClose
    "#]],
    )
}

/// A Desktop whose owner is closed releases what it held and leaves the
/// connection and Files; a redial keeps it closed by its owner.
#[test]
fn a_desktop_closed_by_its_owner_releases_what_it_held_and_keeps_files() -> TestResult {
    let trace = traced();
    check(
        &trace,
        DesktopClose::OwnerClose,
        expect![[r#"
        controller:
          service: service phase from=active generation=1 kind=Desktop service=1 to=closed: OwnerClose
        host:
          hosting: service opened connection=1 kind=Some(Files) service=2
          service: service phase from=active generation=1 kind=Desktop service=1 to=closed: PeerClose
    "#]],
    )
}

/// A Desktop the host closes releases what it held and leaves the
/// connection and Files; a redial keeps it closed by its peer.
#[test]
fn a_desktop_closed_by_the_host_releases_what_it_held_and_keeps_files() -> TestResult {
    let trace = traced();
    check(
        &trace,
        DesktopClose::HostClose,
        expect![[r#"
        controller:
          service: service phase from=active generation=1 kind=Desktop service=1 to=closed: PeerClose
        host:
          hosting: service opened connection=1 kind=Some(Files) service=2
          service: service phase from=active generation=1 kind=Desktop service=1 to=closed: OwnerClose
    "#]],
    )
}

/// A Desktop the host stops sharing releases what it held and leaves the
/// connection and Files; a redial keeps it closed by its peer.
#[test]
fn a_desktop_the_host_stops_sharing_releases_what_it_held_and_keeps_files() -> TestResult {
    let trace = traced();
    check(
        &trace,
        DesktopClose::HostStopsSharing,
        expect![[r#"
        controller:
          service: service phase from=active generation=1 kind=Desktop service=1 to=closed: PeerClose
        host:
          hosting: service opened connection=1 kind=Some(Files) service=2
          service: service phase from=active generation=1 kind=Desktop service=1 to=closed: Revoked
    "#]],
    )
}

/// Dropping a Desktop's handle closes the Desktop as its owner would.
#[test]
fn a_desktop_whose_handle_is_dropped_is_closed_by_its_owner() -> TestResult {
    let trace = traced();
    let pair = Pair::for_desktop();
    InputLog::install(&pair.host, (1920, 1080))?;
    let session = pair.connect_services();
    let desktop = pair.attach(&session, ServiceKind::Desktop);
    let handle = pair
        .controller
        .core
        .open_desktop_service(desktop.id, desktop.generation)?;
    drop(handle);
    wait_for("the dropped Desktop to close", DEFAULT_WAIT, || {
        (phase(&pair.controller, &desktop).ok()? == ServicePhase::Closed(CloseCause::OwnerClose))
            .then_some(())
    });
    pair.host.wait_released(Some(ServiceKind::Desktop));
    trace.wait_for_records("service phase", 2);
    vnilink_testkit::assert_story(
        expect![[r#"
        controller:
          service: service phase from=active generation=1 kind=Desktop service=1 to=closed: OwnerClose
        host:
          service: service phase from=active generation=1 kind=Desktop service=1 to=closed: PeerClose
    "#]],
        || trace.story(&["service"]),
    );
    Ok(())
}
