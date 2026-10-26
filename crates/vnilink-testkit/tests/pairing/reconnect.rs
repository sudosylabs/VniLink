//! Reaching a paired computer again: the addresses a session teaches each
//! side, a dropped link dialled again without a new file, and a session
//! either side closed staying closed.

use std::time::Duration;

use vnilink::{
    usage, CoreEvent, CustomRelay, Error, Grants, InputEvent, RelaySettings, SessionPhase,
    SessionRole,
};
use vnilink_testkit::desktop::InputLog;
use vnilink_testkit::{holds, traced, wait_for, Engine, Pair, TestResult};

use super::support::{device, drain, pair_for_control, wait_closed, Installed};

/// How long a reconnect or a refresh takes, at most.
const SETTLING: Duration = Duration::from_secs(5);

/// The home relay a host was set up with is what its controller stores
/// for it after a session, and the refresh changes no grant on either
/// side.
#[test]
fn a_custom_home_relay_is_what_the_peer_stores() -> TestResult {
    let _trace = traced();
    let relay = RelaySettings::custom(vec![CustomRelay::new("https://relay.example.com")?])?;
    let host = Installed::start_with("host", relay);
    let controller = Engine::start("controller");
    pair_for_control(&host.core, &controller.core);
    let host_id = host.core.endpoint_id();
    controller.core.connect(&host_id)?;
    let expected = host.core.snapshot()?.relay_urls;
    assert_eq!(
        expected,
        vec!["https://relay.example.com/".to_string()],
        "the host is set up with its relay"
    );
    wait_for("the controller to store the host's relay", SETTLING, || {
        (device(&controller.core, &host_id).relays == expected).then_some(())
    });
    assert_eq!(
        device(&controller.core, &host_id).grants,
        Grants::default(),
        "the controller grants the host nothing"
    );
    let granted = device(&host.core, &controller.endpoint()).grants;
    assert!(granted.control, "the host still grants control");
    assert!(!granted.clipboard, "the refresh grants no clipboard");
    assert!(!granted.transfer, "the refresh grants no transfer");
    Ok(())
}

/// A dropped link is dialled again without a new file: input on the old
/// session is refused, the session comes back under a new generation, and
/// the host released the key held across the drop before applying input
/// again. The controller publishes that it was reconnecting.
#[test]
fn a_dropped_session_dials_again_without_a_new_file() -> TestResult {
    let _trace = traced();
    let pair = Pair::for_desktop();
    let controller = &pair.controller.core;
    let applied = InputLog::install(&pair.host, (1920, 1080))?;
    let session = controller.connect(&pair.host.endpoint())?;
    let events = controller.subscribe()?;
    controller.submit_input(
        session.id,
        session.generation,
        InputEvent::key_down(usage::A),
    )?;
    let first = wait_for("the host to apply the key", SETTLING, || {
        let events = applied.events();
        (!events.is_empty()).then_some(events)
    });
    assert_eq!(
        first,
        [InputEvent::key_down(usage::A)],
        "the host applies the key once"
    );
    controller.drop_link()?;
    wait_for("the session to leave active", SETTLING, || {
        (controller
            .snapshot()
            .expect("a snapshot")
            .outgoing
            .expect("the outgoing session stays while it dials again")
            .phase
            != SessionPhase::Active)
            .then_some(())
    });
    assert!(
        matches!(
            controller.submit_input(
                session.id,
                session.generation,
                InputEvent::key_down(usage::B)
            ),
            Err(Error::NoSession | Error::StaleSession)
        ),
        "input on the dropped session is refused"
    );
    let again = wait_for("the session to be dialled again", SETTLING, || {
        let outgoing = controller
            .snapshot()
            .expect("a snapshot")
            .outgoing
            .expect("the outgoing session stays while it dials again");
        (outgoing.phase == SessionPhase::Active
            && (outgoing.id != session.id || outgoing.generation != session.generation))
            .then_some(outgoing)
    });
    assert_ne!(
        again.generation, session.generation,
        "the session comes back under a new generation"
    );
    controller.submit_input(again.id, again.generation, InputEvent::key_down(usage::C))?;
    let all = wait_for("the host to apply the new input", SETTLING, || {
        let events = applied.events();
        (events.len() >= 3).then_some(events)
    });
    assert_eq!(
        all,
        [
            InputEvent::key_down(usage::A),
            InputEvent::key_up(usage::A),
            InputEvent::key_down(usage::C),
        ],
        "the host released the held key, then applied the new input"
    );
    assert!(
        drain(&events).iter().any(|event| matches!(
            event,
            CoreEvent::SessionChanged {
                phase: SessionPhase::Reconnecting,
                ..
            }
        )),
        "the controller published that it was reconnecting"
    );
    Ok(())
}

/// Waits until both sides' sessions are closed, then checks that they stay
/// closed: nobody dials again.
fn stays_closed(pair: &Pair) {
    let (host, controller) = (&pair.host.core, &pair.controller.core);
    wait_closed(host, SessionRole::Host, SETTLING);
    wait_closed(controller, SessionRole::Controller, SETTLING);
    holds(
        "both sessions staying closed",
        Duration::from_millis(700),
        || {
            let closed = |phase: Option<SessionPhase>| phase == Some(SessionPhase::Closed);
            closed(
                controller
                    .snapshot()
                    .ok()
                    .and_then(|snapshot| snapshot.outgoing)
                    .map(|session| session.phase),
            ) && closed(
                host.snapshot()
                    .ok()
                    .and_then(|snapshot| snapshot.incoming)
                    .map(|session| session.phase),
            )
        },
    );
}

/// A controller that disconnects does not dial again.
#[test]
fn a_local_disconnect_does_not_dial_again() -> TestResult {
    let _trace = traced();
    let pair = Pair::for_desktop();
    pair.controller.core.connect(&pair.host.endpoint())?;
    pair.controller.core.disconnect()?;
    stays_closed(&pair);
    Ok(())
}

/// A session the host closes is not dialled again.
#[test]
fn the_host_closing_the_session_does_not_dial_again() -> TestResult {
    let _trace = traced();
    let pair = Pair::for_desktop();
    pair.controller.core.connect(&pair.host.endpoint())?;
    pair.host.core.stop_hosted_session()?;
    stays_closed(&pair);
    Ok(())
}
