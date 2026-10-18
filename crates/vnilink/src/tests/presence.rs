use super::support::{pair_for_control, start_direct};
use crate::protocol::ControlMessage;
use crate::{Capability, Core, CoreEvent, DevicePresence, Error, Grants, RemoteStatus};
use std::time::{Duration, Instant};

fn wait(name: &str, mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !condition() {
        assert!(Instant::now() < deadline, "{name} timed out");
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn presence(core: &Core, peer: &str) -> DevicePresence {
    core.snapshot()
        .unwrap()
        .devices
        .into_iter()
        .find(|device| device.endpoint_id == peer)
        .map(|device| device.presence)
        .unwrap()
}

fn status(core: &Core, peer: &str, capability: Capability) -> Option<RemoteStatus> {
    core.peer_availability(peer)
        .unwrap()
        .and_then(|report| report.into_iter().find(|(c, _)| *c == capability))
        .map(|(_, status)| status)
}

#[test]
fn a_paired_computer_is_connected_without_opening_anything() {
    let host = start_direct();
    let controller = start_direct();
    pair_for_control(&host, &controller);
    let events = controller.subscribe().unwrap();
    assert_eq!(
        presence(&controller, &host.endpoint_id()),
        DevicePresence::Disconnected
    );
    controller.set_presence_links(true).unwrap();
    wait("connected", || {
        presence(&controller, &host.endpoint_id()) == DevicePresence::Connected
    });
    let changes: Vec<_> = std::iter::from_fn(|| events.try_recv())
        .filter_map(|event| match event {
            CoreEvent::PresenceChanged { presence, .. } => Some(presence),
            _ => None,
        })
        .collect();
    assert_eq!(changes.last(), Some(&DevicePresence::Connected));
    // The link is reachability, not use: nothing runs on the host.
    assert_eq!(host.hosted_sessions().unwrap().len(), 1);
    assert!(host.hosted_services().unwrap().is_empty());
    // Asking for the link again reuses it.
    let first = controller
        .begin_connect_services(&host.endpoint_id())
        .unwrap();
    let again = controller
        .begin_connect_services(&host.endpoint_id())
        .unwrap();
    assert_eq!(first.id, again.id);
}

#[test]
fn a_host_that_stops_accepting_shows_as_such_and_comes_back() {
    let host = start_direct();
    let controller = start_direct();
    pair_for_control(&host, &controller);
    controller.set_presence_links(true).unwrap();
    wait("connected", || {
        presence(&controller, &host.endpoint_id()) == DevicePresence::Connected
    });
    host.stop_hosting().unwrap();
    wait("not accepting", || {
        presence(&controller, &host.endpoint_id()) == DevicePresence::NotAccepting
    });
    assert_eq!(
        controller.peer_availability(&host.endpoint_id()).unwrap(),
        None
    );
    host.start_hosting().unwrap();
    wait("connected again", || {
        presence(&controller, &host.endpoint_id()) == DevicePresence::Connected
    });
}

#[test]
fn the_host_reports_what_it_allows_and_follows_policy_changes() {
    let host = start_direct();
    let controller = start_direct();
    pair_for_control(&host, &controller);
    controller.set_presence_links(true).unwrap();
    let peer = host.endpoint_id();
    wait("first report", || {
        controller.peer_availability(&peer).unwrap().is_some()
    });
    let report = controller.peer_availability(&peer).unwrap().unwrap();
    assert_eq!(report.len(), Capability::ALL.len());
    // Pairing granted view and control but not files.
    assert_eq!(
        status(&controller, &peer, Capability::Files),
        Some(RemoteStatus::TurnedOff)
    );
    // No camera backend is installed on the host.
    assert_eq!(
        status(&controller, &peer, Capability::Camera),
        Some(RemoteStatus::Unsupported)
    );
    let events = controller.subscribe().unwrap();
    host.set_desktop_sharing(Grants {
        view: true,
        control: true,
        clipboard: false,
        transfer: true,
    })
    .unwrap();
    wait("files turned on", || {
        status(&controller, &peer, Capability::Files) == Some(RemoteStatus::Available)
    });
    assert!(std::iter::from_fn(|| events.try_recv()).any(
        |event| matches!(event, CoreEvent::PeerAvailabilityChanged { endpoint_id } if endpoint_id == peer)
    ));
}

#[test]
fn unpairing_ends_the_link_for_good() {
    let host = start_direct();
    let controller = start_direct();
    pair_for_control(&host, &controller);
    controller.set_presence_links(true).unwrap();
    wait("connected", || {
        presence(&controller, &host.endpoint_id()) == DevicePresence::Connected
    });
    controller.unpair(&host.endpoint_id()).unwrap();
    wait("link closed", || host.hosted_sessions().unwrap().is_empty());
    std::thread::sleep(Duration::from_secs(3));
    assert!(
        host.hosted_sessions().unwrap().is_empty(),
        "an unpaired computer was dialled again"
    );
    assert!(matches!(
        controller.peer_availability(&host.endpoint_id()),
        Err(Error::NotPaired)
    ));
}

#[test]
fn links_stay_outside_the_live_session_cap() {
    let host = start_direct();
    let controllers: Vec<Core> = (0..5).map(|_| start_direct()).collect();
    for controller in &controllers {
        pair_for_control(&host, controller);
        controller.set_presence_links(true).unwrap();
    }
    for controller in &controllers {
        wait("every link connected", || {
            presence(controller, &host.endpoint_id()) == DevicePresence::Connected
        });
    }
    assert_eq!(host.hosted_sessions().unwrap().len(), 5);
    // A desktop session still fits beside five idle links.
    let session = controllers[0].begin_connect(&host.endpoint_id()).unwrap();
    wait("desktop admitted", || {
        controllers[0].controller_session(session.id).unwrap().phase == crate::SessionPhase::Active
    });
}

#[test]
fn availability_message_is_strict() {
    let message = ControlMessage::Availability {
        entries: vec![
            (Capability::Camera, RemoteStatus::TurnedOff),
            (Capability::Files, RemoteStatus::Available),
        ],
    };
    let bytes = message.encode();
    assert_eq!(ControlMessage::decode(&bytes).unwrap(), message);
    let empty = ControlMessage::Availability { entries: vec![] };
    assert_eq!(ControlMessage::decode(&empty.encode()).unwrap(), empty);
    for bad in [
        vec![13, 1, 0, 0],       // unknown capability 0
        vec![13, 1, 6, 9],       // unknown status
        vec![13, 2, 6, 1, 6, 0], // the same capability twice
        vec![13, 2, 6, 1],       // shorter than its count
        vec![13, 1, 6, 1, 4],    // trailing byte
        vec![13, 14],            // more entries than capabilities
    ] {
        assert!(
            matches!(ControlMessage::decode(&bad), Err(Error::Malformed)),
            "{bad:?}"
        );
    }
}
