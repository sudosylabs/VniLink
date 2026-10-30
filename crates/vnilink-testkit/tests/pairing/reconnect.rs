// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! Reaching a paired computer again: the addresses a session teaches each
//! side, a dropped link dialled again without a new file, and a session
//! either side closed staying closed.

use std::time::Duration;

use vnilink::{
    usage, CoreEvent, CustomRelay, Grants, InputEvent, RelaySettings, ServiceKind, SessionPhase,
    SessionRole,
};
use vnilink_testkit::desktop::InputLog;
use vnilink_testkit::events::drain;
use vnilink_testkit::session::wait_closed;
use vnilink_testkit::store::Installed;
use vnilink_testkit::{holds, pair_cores_for_control, traced, wait_for, Engine, Pair, TestResult};

use super::support::device;

/// How long a reconnect or a refresh takes, at most.
const SETTLING: Duration = Duration::from_secs(5);

/// The home relay a host was set up with is what its controller stores
/// for it after a link, and the refresh changes no grant on either
/// side.
#[test]
fn a_custom_home_relay_is_what_the_peer_stores() -> TestResult {
    let _trace = traced();
    let relay = RelaySettings::custom(vec![CustomRelay::new("https://relay.example.com")?])?;
    let host = Installed::start_with("host", relay);
    let controller = Engine::start("controller");
    pair_cores_for_control(&host.core, &controller.core)?;
    let host_id = host.core.endpoint_id();
    controller.connect_services_to(&host_id)?;
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

/// A dropped link is dialled again without a new file: the connection
/// comes back under a new generation, the desktop of the lost one ends and
/// takes no input, and the host released the key held across the drop
/// before a desktop on the new generation applies input again. The
/// controller publishes that it was reconnecting.
#[test]
fn a_dropped_connection_dials_again_without_a_new_file() -> TestResult {
    let _trace = traced();
    let pair = Pair::for_desktop();
    let controller = &pair.controller.core;
    let applied = InputLog::install(&pair.host, (1920, 1080))?;
    let connection = pair.connect_services();
    let desktop = pair.controller.open_desktop(&connection)?;
    let events = controller.subscribe()?;
    desktop.submit_input(InputEvent::key_down(usage::A))?;
    assert_eq!(
        applied.wait_for(1),
        [InputEvent::key_down(usage::A)],
        "the host applies the key once"
    );
    controller.drop_controller_link(connection.id)?;
    let again = pair.controller.redialed(&connection);
    assert_eq!(
        again.id, connection.id,
        "the connection keeps its handle across the redial"
    );
    wait_for("the lost generation's desktop to end", SETTLING, || {
        desktop.is_cancelled().then_some(())
    });
    assert!(
        desktop
            .submit_input(InputEvent::key_down(usage::B))
            .is_err(),
        "the lost generation's desktop takes no input"
    );
    pair.host.wait_released(Some(ServiceKind::Desktop));
    let fresh = pair.controller.open_desktop(&again)?;
    fresh.submit_input(InputEvent::key_down(usage::C))?;
    assert_eq!(
        applied.wait_for(3),
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
    pair.connect_services();
    pair.controller.core.disconnect()?;
    stays_closed(&pair);
    Ok(())
}

/// A link the host closes is not dialled again.
#[test]
fn the_host_closing_the_session_does_not_dial_again() -> TestResult {
    let _trace = traced();
    let pair = Pair::for_desktop();
    pair.connect_services();
    pair.host.core.stop_hosted_session()?;
    stays_closed(&pair);
    Ok(())
}
