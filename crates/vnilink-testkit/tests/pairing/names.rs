//! What paired computers call each other: the computer name each one
//! reports, learnt at pairing and refreshed on connecting, and the
//! nickname an owner gives a peer, which stays on that computer. Kit
//! engines are named after themselves, so `host` and `controller` are the
//! names they report.

use expect_test::expect;
use std::time::Duration;

use vnilink::{CoreEvent, Error, SessionPhase, SessionRole};
use vnilink_testkit::events::drain;
use vnilink_testkit::session::wait_closed;
use vnilink_testkit::store::Installed;
use vnilink_testkit::{pair_cores_for_control, traced, Engine, Pair, TestResult};

use super::support::device;

const CLOSING: Duration = Duration::from_secs(5);

/// Each computer knows the other by the name it reports, from pairing on,
/// so a connect that brings the same name publishes no rename.
#[test]
fn peers_exchange_display_names_on_connect() -> TestResult {
    let trace = traced();
    let pair = Pair::for_desktop();
    let (host, controller) = (&pair.host.core, &pair.controller.core);
    let host_events = host.subscribe()?;
    let controller_events = controller.subscribe()?;
    let session = pair.connect_services();
    assert_eq!(session.phase, SessionPhase::Active);
    assert_eq!(
        controller.peer_hostname(&pair.host.endpoint())?.as_deref(),
        Some("host"),
        "the controller knows the host by its name"
    );
    assert_eq!(
        host.peer_hostname(&pair.controller.endpoint())?.as_deref(),
        Some("controller"),
        "the host knows the controller by its name"
    );
    // Pairing already stored the name, so the unchanged handshake is quiet.
    for events in [&host_events, &controller_events] {
        assert!(
            !drain(events)
                .iter()
                .any(|event| matches!(event, CoreEvent::PeerRenamed { .. })),
            "an unchanged name publishes no rename"
        );
    }
    controller.disconnect()?;
    wait_closed(controller, SessionRole::Controller, CLOSING);
    vnilink_testkit::assert_story(
        expect![[r#"
        controller:
          pairing: invitation accepted
          pairing: invitation issued direct=1 relays=0 ttl_s=60
        host:
          pairing: invitation issued direct=1 relays=0 ttl_s=60
          pairing: invitation accepted
    "#]],
        || trace.story(&["pairing"]),
    );
    Ok(())
}

/// A computer that is not paired has no name, and an id that is not one
/// is refused.
#[test]
fn unknown_devices_have_no_name() {
    let _trace = traced();
    let core = Engine::start("host");
    let other = Engine::start("controller");
    assert!(
        matches!(
            core.core.peer_hostname(&other.endpoint()),
            Err(Error::NotPaired)
        ),
        "an unpaired computer has no name"
    );
    assert!(
        matches!(
            core.core.peer_hostname("not-a-key"),
            Err(Error::InvalidSetting(_))
        ),
        "an id that is not one is refused"
    );
}

/// The name a computer learnt survives its restart.
#[test]
fn names_survive_a_restart_in_the_current_record() -> TestResult {
    let _trace = traced();
    let mut pair = Pair::for_desktop();
    pair.connect_services();
    assert_eq!(
        pair.controller
            .core
            .peer_hostname(&pair.host.endpoint())?
            .as_deref(),
        Some("host"),
        "the controller knows the host by its name"
    );
    // Reopening the store keeps the names.
    pair.host.restart();
    assert_eq!(
        pair.host
            .core
            .peer_hostname(&pair.controller.endpoint())?
            .as_deref(),
        Some("controller"),
        "the restarted host still knows the controller's name"
    );
    Ok(())
}

/// Forgetting a computer forgets its name, on the side that forgot it only.
#[test]
fn unpairing_drops_the_name_with_the_device() -> TestResult {
    let _trace = traced();
    let pair = Pair::for_desktop();
    let (host, controller) = (&pair.host.core, &pair.controller.core);
    pair.connect_services();
    assert_eq!(
        controller.peer_hostname(&pair.host.endpoint())?.as_deref(),
        Some("host")
    );
    host.unpair(&pair.controller.endpoint())?;
    assert_eq!(
        controller.peer_hostname(&pair.host.endpoint())?.as_deref(),
        Some("host"),
        "the controller still knows the host until it forgets it too"
    );
    controller.unpair(&pair.host.endpoint())?;
    assert!(
        matches!(
            controller.peer_hostname(&pair.host.endpoint()),
            Err(Error::NotPaired)
        ),
        "a forgotten computer has no name"
    );
    Ok(())
}

/// Pairing learns the name an invitation carries before any connect, and
/// a connect refreshes it to the one the peer reports now, publishing the
/// rename. A nickname is the owner's own: cleaned, never sent to the peer,
/// kept when its save fails or it is not a name, through a restart, and
/// gone with the device.
#[test]
fn pairing_learns_names_before_connect_and_local_names_survive_refresh() -> TestResult {
    let trace = traced();
    let mut host = Installed::start("host");
    let controller = Engine::start("controller");
    pair_cores_for_control(&host.core, &controller.core)?;
    assert_eq!(
        host.core.peer_hostname(&controller.endpoint())?.as_deref(),
        Some("controller"),
        "pairing teaches the host the controller's name"
    );
    assert_eq!(
        controller
            .core
            .peer_hostname(&host.core.endpoint_id())?
            .as_deref(),
        Some("host"),
        "pairing teaches the controller the host's name"
    );
    let id = controller.endpoint();
    host.core.set_peer_pet_name(&id, Some("  Work laptop  "))?;
    assert_eq!(
        device(&host.core, &id).pet_name.as_deref(),
        Some("Work laptop"),
        "a nickname is kept trimmed"
    );
    // The controller's invitation carries another name than the one it
    // reports when it next connects.
    controller.core.set_hostname("renamed-laptop")?;
    let token = controller.core.create_invitation(Duration::from_secs(60))?;
    controller.core.set_hostname("controller")?;
    host.core.accept_invitation(&token)?;
    assert_eq!(
        host.core.peer_hostname(&id)?.as_deref(),
        Some("renamed-laptop"),
        "an invitation brings the name it carries"
    );
    let events = host.core.subscribe()?;
    controller.connect_services_to(&host.core.endpoint_id())?;
    assert_eq!(
        host.core.peer_hostname(&id)?.as_deref(),
        Some("controller"),
        "a connect refreshes the name"
    );
    assert!(
        drain(&events).iter().any(
            |event| matches!(event, CoreEvent::PeerRenamed { endpoint_id, .. } if *endpoint_id == id)
        ),
        "hostname refresh must notify subscribers"
    );
    assert!(
        controller.core.snapshot()?.devices[0].pet_name.is_none(),
        "local names never reach the peer"
    );
    controller.core.disconnect()?;
    wait_closed(&controller.core, SessionRole::Controller, CLOSING);
    host.store.fail_next_save(Error::StoreUnavailable);
    assert!(
        matches!(
            host.core.set_peer_pet_name(&id, Some("Bad save")),
            Err(Error::StoreUnavailable)
        ),
        "a nickname whose save fails is refused"
    );
    assert_eq!(
        host.core.snapshot()?.devices[0].pet_name.as_deref(),
        Some("Work laptop"),
        "a failed save keeps the nickname"
    );
    assert!(
        host.core.set_peer_pet_name(&id, Some("bad\nname")).is_err(),
        "a nickname across lines is refused"
    );
    host.restart();
    assert_eq!(
        host.core.snapshot()?.devices[0].pet_name.as_deref(),
        Some("Work laptop"),
        "a nickname survives a restart"
    );
    host.core.set_peer_pet_name(&id, None)?;
    assert!(
        host.core.snapshot()?.devices[0].pet_name.is_none(),
        "a nickname can be cleared"
    );
    host.core.unpair(&id)?;
    assert!(
        matches!(
            host.core.set_peer_pet_name(&id, Some("gone")),
            Err(Error::NotPaired)
        ),
        "a forgotten computer takes no nickname"
    );
    vnilink_testkit::assert_story(
        expect![[r#"
        controller:
          pairing: invitation accepted
          pairing: invitation issued direct=1 relays=0 ttl_s=60
          pairing: invitation issued direct=1 relays=0 ttl_s=60
        host:
          pairing: invitation issued direct=1 relays=0 ttl_s=60
          pairing: invitation accepted
          pairing: invitation accepted
          pairing: unpair requested
    "#]],
        || trace.story(&["pairing"]),
    );
    Ok(())
}
