//! Presence: a controller keeps a link to each paired host it may reach, so
//! it can show which are there and what each allows, without opening
//! anything on them.

use expect_test::expect;
use std::time::Duration;

use vnilink::{
    Capability, Core, CoreEvent, DevicePresence, Error, Grants, RemoteStatus, SessionPhase,
};
use vnilink_testkit::desktop::MonitorCapture;
use vnilink_testkit::events::drain;
use vnilink_testkit::{holds, traced, wait_for, Engine, Pair, TestResult};

use super::support::device;

/// How long presence takes to follow the host, at most.
const FOLLOWING: Duration = Duration::from_secs(20);

fn presence(core: &Core, peer: &str) -> DevicePresence {
    device(core, peer).presence
}

fn status(core: &Core, peer: &str, capability: Capability) -> Option<RemoteStatus> {
    core.peer_availability(peer)
        .expect("a paired computer's availability")
        .and_then(|report| report.into_iter().find(|(c, _)| *c == capability))
        .map(|(_, status)| status)
}

/// Waits until `controller` sees `host` as `wanted`.
fn wait_presence(controller: &Core, host: &Engine, wanted: DevicePresence) {
    wait_for(
        &format!("the host to show as {wanted:?}"),
        FOLLOWING,
        || (presence(controller, &host.endpoint()) == wanted).then_some(()),
    );
}

/// With presence links on, a paired host shows as connected and the change
/// is published, while nothing runs on it; asking for a link again reuses
/// the one there is.
#[test]
fn a_paired_computer_is_connected_without_opening_anything() -> TestResult {
    let trace = traced();
    let pair = Pair::for_desktop();
    let (host, controller) = (&pair.host.core, &pair.controller.core);
    let events = controller.subscribe()?;
    assert_eq!(
        presence(controller, &pair.host.endpoint()),
        DevicePresence::Disconnected,
        "a host is disconnected before any link"
    );
    controller.set_presence_links(true)?;
    wait_presence(controller, &pair.host, DevicePresence::Connected);
    // A presence is published before anyone can read it.
    let changes: Vec<_> = drain(&events)
        .into_iter()
        .filter_map(|event| match event {
            CoreEvent::PresenceChanged { presence, .. } => Some(presence),
            _ => None,
        })
        .collect();
    assert_eq!(
        changes.last(),
        Some(&DevicePresence::Connected),
        "the change to connected is published by the time it reads so"
    );
    // The link is reachability, not use: nothing runs on the host.
    assert_eq!(host.hosted_sessions()?.len(), 1, "the host holds one link");
    assert!(
        host.hosted_services()?.is_empty(),
        "a link runs no service on the host"
    );
    // Asking for the link again reuses it.
    let first = controller.begin_connect_services(&pair.host.endpoint())?;
    let again = controller.begin_connect_services(&pair.host.endpoint())?;
    assert_eq!(first.id, again.id, "asking again reuses the link");
    vnilink_testkit::assert_story(
        expect![[r#"
        controller:
          relay: endpoint bound listen=Loopback relay=false
          relay: presence presence=Connecting
          relay: a link to a paired computer is up paths=1 relayed=false
          relay: presence presence=Connected
        host:
          relay: endpoint bound listen=Loopback relay=false
    "#]],
        || trace.story(&["relay"]),
    );
    Ok(())
}

/// A host that stops accepting shows as such, with no report of what it
/// allows, and shows as connected again once it accepts.
#[test]
fn a_host_that_stops_accepting_shows_as_such_and_comes_back() -> TestResult {
    let trace = traced();
    let pair = Pair::for_desktop();
    let (host, controller) = (&pair.host.core, &pair.controller.core);
    controller.set_presence_links(true)?;
    wait_presence(controller, &pair.host, DevicePresence::Connected);
    host.stop_hosting()?;
    wait_presence(controller, &pair.host, DevicePresence::NotAccepting);
    assert_eq!(
        controller.peer_availability(&pair.host.endpoint())?,
        None,
        "a host that does not accept reports nothing it allows"
    );
    host.start_hosting()?;
    wait_presence(controller, &pair.host, DevicePresence::Connected);
    vnilink_testkit::assert_story(
        expect![[r#"
        controller:
          relay: endpoint bound listen=Loopback relay=false
          relay: presence presence=Connecting
          relay: a link to a paired computer is up paths=1 relayed=false
          relay: presence presence=Connected
          relay: presence presence=Disconnected
          relay: a link to a paired computer is down; trying again failures=1 refused=false
          relay: presence presence=NotAccepting
          relay: a link to a paired computer is down; trying again failures=2 refused=true
          relay: a link to a paired computer is up paths=1 relayed=false
          relay: presence presence=Connected
        host:
          relay: endpoint bound listen=Loopback relay=false
    "#]],
        || trace.story(&["relay"]),
    );
    Ok(())
}

/// Every redial of an unreachable host is a session of its own. They do not
/// pile up in the controller's list: services read that list to find their
/// link, and a long one cannot be read.
#[test]
fn links_that_failed_do_not_pile_up_in_the_controllers_sessions() -> TestResult {
    let _trace = traced();
    let pair = Pair::for_desktop();
    let (host, controller) = (&pair.host.core, &pair.controller.core);
    host.stop_hosting()?;
    controller.set_presence_links(true)?;
    wait_presence(controller, &pair.host, DevicePresence::NotAccepting);
    let refused = controller.controller_sessions()?;
    assert!(
        refused
            .iter()
            .all(|s| !matches!(s.phase, SessionPhase::Failed(_))),
        "a refused link is not kept: {refused:?}"
    );
    host.start_hosting()?;
    wait_presence(controller, &pair.host, DevicePresence::Connected);
    let sessions = controller.controller_sessions()?;
    assert_eq!(
        sessions.len(),
        1,
        "only the live link is listed: {sessions:?}"
    );
    assert_eq!(sessions[0].phase, SessionPhase::Active);
    Ok(())
}

/// Over the link the host reports, for every capability, whether it
/// allows it, and reports again when what it shares changes.
#[test]
fn the_host_reports_what_it_allows_and_follows_policy_changes() -> TestResult {
    let _trace = traced();
    let pair = Pair::for_desktop();
    let (host, controller) = (&pair.host.core, &pair.controller.core);
    controller.set_presence_links(true)?;
    let peer = pair.host.endpoint();
    let report = wait_for("the host's first report", FOLLOWING, || {
        controller
            .peer_availability(&peer)
            .expect("a paired computer's availability")
    });
    assert_eq!(
        report.len(),
        Capability::ALL.len(),
        "the report names every capability"
    );
    // Pairing granted view and control but not files.
    assert_eq!(
        status(controller, &peer, Capability::Files),
        Some(RemoteStatus::TurnedOff),
        "Files the host does not share is off"
    );
    // No camera backend is installed on the host.
    assert_eq!(
        status(controller, &peer, Capability::Camera),
        Some(RemoteStatus::Unsupported),
        "a camera the host has not is unsupported"
    );
    let events = controller.subscribe()?;
    host.set_desktop_sharing(Grants {
        view: true,
        control: true,
        clipboard: false,
        transfer: true,
    })?;
    wait_for("Files to turn on", FOLLOWING, || {
        (status(controller, &peer, Capability::Files) == Some(RemoteStatus::Available))
            .then_some(())
    });
    assert!(
        drain(&events).iter().any(
            |event| matches!(event, CoreEvent::PeerAvailabilityChanged { endpoint_id } if *endpoint_id == peer)
        ),
        "the new report is published"
    );
    Ok(())
}

/// Forgetting a host ends its link, which is not dialled again, and its
/// report is gone.
#[test]
fn unpairing_ends_the_link_for_good() -> TestResult {
    let trace = traced();
    let pair = Pair::for_desktop();
    let (host, controller) = (&pair.host.core, &pair.controller.core);
    controller.set_presence_links(true)?;
    wait_presence(controller, &pair.host, DevicePresence::Connected);
    controller.unpair(&pair.host.endpoint())?;
    wait_for("the link to close", FOLLOWING, || {
        host.hosted_sessions().ok()?.is_empty().then_some(())
    });
    holds(
        "an unpaired computer not being dialled again",
        Duration::from_secs(3),
        || {
            host.hosted_sessions()
                .is_ok_and(|sessions| sessions.is_empty())
        },
    );
    assert!(
        matches!(
            controller.peer_availability(&pair.host.endpoint()),
            Err(Error::NotPaired)
        ),
        "a forgotten host has no report"
    );
    vnilink_testkit::assert_story(
        expect![[r#"
        controller:
          relay: endpoint bound listen=Loopback relay=false
          relay: presence presence=Connecting
          relay: a link to a paired computer is up paths=1 relayed=false
          relay: presence presence=Connected
          relay: a link to a computer no longer paired ended
        host:
          relay: endpoint bound listen=Loopback relay=false
    "#]],
        || trace.story(&["relay"]),
    );
    Ok(())
}

/// Idle links do not count toward the host's cap on live sessions: a
/// desktop still opens on one of five of them.
#[test]
fn links_stay_outside_the_live_session_cap() -> TestResult {
    let _trace = traced();
    let pair = Pair::for_desktop();
    pair.host
        .core
        .set_capture(Box::new(MonitorCapture::new((1920, 1080))))?;
    let mut controllers = vec![&pair.controller];
    let viewers: Vec<Engine> = (1..5)
        .map(|index| pair.add_viewer(&format!("viewer-{index}")))
        .collect();
    controllers.extend(&viewers);
    for controller in &controllers {
        controller.core.set_presence_links(true)?;
    }
    for controller in &controllers {
        wait_presence(&controller.core, &pair.host, DevicePresence::Connected);
    }
    assert_eq!(
        pair.host.core.hosted_sessions()?.len(),
        5,
        "the host holds five links"
    );
    // A desktop still opens beside five idle links, on the controller's own.
    let link = pair
        .controller
        .core
        .controller_sessions()?
        .into_iter()
        .find(|session| session.phase == SessionPhase::Active)
        .ok_or("the controller's link")?;
    let desktop = pair.controller.open_desktop(&link)?;
    assert!(!desktop.is_cancelled(), "the desktop is admitted");
    Ok(())
}
