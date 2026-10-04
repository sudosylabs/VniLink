use std::sync::{Arc, Mutex};

use crate::error::Error;
use crate::input::InputEvent;
use crate::session::{Grants, SessionPhase, SessionRole};

use super::support::*;

#[test]
fn input_for_the_wrong_session_or_generation_is_rejected() {
    let host = start_direct();
    let controller = start_direct();
    pair_for_control(&host, &controller);
    let session = controller.connect(&host.endpoint_id()).unwrap();
    assert!(matches!(
        controller.submit_input(
            session.id + 1,
            session.generation,
            InputEvent::key_down(crate::input::usage::A),
        ),
        Err(Error::NoSession)
    ));
    assert!(matches!(
        controller.submit_input(
            session.id,
            session.generation + 1,
            InputEvent::key_down(crate::input::usage::A),
        ),
        Err(Error::StaleSession)
    ));
    controller
        .submit_input(
            session.id,
            session.generation,
            InputEvent::key_down(crate::input::usage::A),
        )
        .unwrap();
    controller.disconnect().unwrap();
    assert!(matches!(
        controller.submit_input(
            session.id,
            session.generation,
            InputEvent::key_up(crate::input::usage::A),
        ),
        Err(Error::NoSession)
    ));
}

#[test]
fn stopping_hosting_disables_new_sessions_even_when_none_is_active() {
    let idle = start_direct();
    idle.stop_hosting().unwrap();
    assert!(!idle.snapshot().unwrap().hosting);

    let host = start_direct();
    let controller = start_direct();
    pair_for_control(&host, &controller);
    assert!(matches!(host.stop_hosted_session(), Err(Error::NoSession)));
    assert!(host.snapshot().unwrap().hosting);
    host.stop_hosting().unwrap();
    assert!(!host.snapshot().unwrap().hosting);
    assert!(matches!(
        controller.connect(&host.endpoint_id()),
        Err(Error::NotHosting)
    ));
    assert!(host.snapshot().unwrap().devices[0].grants.control);
}

#[test]
fn stopping_hosting_ends_the_active_session_and_releases_keys() {
    let host = start_direct();
    let controller = start_direct();
    let applied = Arc::new(Mutex::new(Vec::new()));
    host.set_desktop(Box::new(RecordingDesktop {
        events: Arc::clone(&applied),
    }))
    .unwrap();
    pair_for_control(&host, &controller);
    let session = controller.connect(&host.endpoint_id()).unwrap();
    controller
        .submit_input(
            session.id,
            session.generation,
            InputEvent::key_down(crate::input::usage::A),
        )
        .unwrap();
    controller
        .submit_input(
            session.id,
            session.generation,
            InputEvent::button_down(crate::input::PointerButton::Left),
        )
        .unwrap();
    wait_applied(
        &applied,
        &[
            InputEvent::key_down(crate::input::usage::A),
            InputEvent::button_down(crate::input::PointerButton::Left),
        ],
    );
    host.stop_hosting().unwrap();
    wait_applied(
        &applied,
        &[
            InputEvent::key_down(crate::input::usage::A),
            InputEvent::button_down(crate::input::PointerButton::Left),
            InputEvent::button_up(crate::input::PointerButton::Left),
            InputEvent::key_up(crate::input::usage::A),
        ],
    );
    wait_closed(&host, SessionRole::Host);
    wait_closed(&controller, SessionRole::Controller);
    assert!(!host.snapshot().unwrap().hosting);
    assert!(host.snapshot().unwrap().devices[0].grants.control);
    assert!(matches!(
        controller.connect(&host.endpoint_id()),
        Err(Error::NotHosting)
    ));
    host.start_hosting().unwrap();
    let again = controller.connect(&host.endpoint_id()).unwrap();
    assert_eq!(again.phase, SessionPhase::Active);
}
#[test]
fn disconnect_closes_both_sides_and_releases_held_input() {
    let host = start_direct();
    let controller = start_direct();
    let applied = Arc::new(Mutex::new(Vec::new()));
    host.set_desktop(Box::new(RecordingDesktop {
        events: Arc::clone(&applied),
    }))
    .unwrap();
    pair_for_control(&host, &controller);
    let session = controller.connect(&host.endpoint_id()).unwrap();
    controller
        .submit_input(
            session.id,
            session.generation,
            InputEvent::key_down(crate::input::usage::A),
        )
        .unwrap();
    controller
        .submit_input(
            session.id,
            session.generation,
            InputEvent::button_down(crate::input::PointerButton::Left),
        )
        .unwrap();
    wait_applied(
        &applied,
        &[
            InputEvent::key_down(crate::input::usage::A),
            InputEvent::button_down(crate::input::PointerButton::Left),
        ],
    );
    controller.disconnect().unwrap();
    wait_closed(&host, SessionRole::Host);
    wait_closed(&controller, SessionRole::Controller);
    wait_applied(
        &applied,
        &[
            InputEvent::key_down(crate::input::usage::A),
            InputEvent::button_down(crate::input::PointerButton::Left),
            InputEvent::button_up(crate::input::PointerButton::Left),
            InputEvent::key_up(crate::input::usage::A),
        ],
    );
    assert!(host.snapshot().unwrap().hosting);
    assert!(matches!(
        controller.submit_input(
            session.id,
            session.generation,
            InputEvent::key_down(crate::input::usage::A),
        ),
        Err(Error::NoSession)
    ));
    assert!(matches!(controller.disconnect(), Err(Error::NoSession)));
    let again = controller.connect(&host.endpoint_id()).unwrap();
    assert_eq!(again.phase, SessionPhase::Active);
}

#[test]
fn revoke_stops_the_session_releases_input_and_blocks_reconnect() {
    let host = start_direct();
    let controller = start_direct();
    let applied = Arc::new(Mutex::new(Vec::new()));
    host.set_desktop(Box::new(RecordingDesktop {
        events: Arc::clone(&applied),
    }))
    .unwrap();
    pair_for_control(&host, &controller);
    let session = controller.connect(&host.endpoint_id()).unwrap();
    controller
        .submit_input(
            session.id,
            session.generation,
            InputEvent::key_down(crate::input::usage::LEFT_SHIFT),
        )
        .unwrap();
    controller
        .submit_input(
            session.id,
            session.generation,
            InputEvent::button_down(crate::input::PointerButton::Right),
        )
        .unwrap();
    wait_applied(
        &applied,
        &[
            InputEvent::key_down(crate::input::usage::LEFT_SHIFT),
            InputEvent::button_down(crate::input::PointerButton::Right),
        ],
    );
    host.revoke(&controller.endpoint_id()).unwrap();
    wait_applied(
        &applied,
        &[
            InputEvent::key_down(crate::input::usage::LEFT_SHIFT),
            InputEvent::button_down(crate::input::PointerButton::Right),
            InputEvent::button_up(crate::input::PointerButton::Right),
            InputEvent::key_up(crate::input::usage::LEFT_SHIFT),
        ],
    );
    wait_closed(&host, SessionRole::Host);
    wait_closed(&controller, SessionRole::Controller);
    let devices = host.snapshot().unwrap().devices;
    assert_eq!(devices.len(), 1);
    assert_eq!(devices[0].endpoint_id, controller.endpoint_id());
    assert!(!devices[0].grants.control);
    assert!(matches!(
        controller.connect(&host.endpoint_id()),
        Err(Error::Unauthorized)
    ));
    assert!(matches!(
        controller.submit_input(
            session.id,
            session.generation,
            InputEvent::key_down(crate::input::usage::A),
        ),
        Err(Error::NoSession)
    ));
}

#[test]
fn removing_control_mid_session_stops_applying_input() {
    let host = start_direct();
    let controller = start_direct();
    let applied = Arc::new(Mutex::new(Vec::new()));
    host.set_desktop(Box::new(RecordingDesktop {
        events: Arc::clone(&applied),
    }))
    .unwrap();
    pair_for_control(&host, &controller);
    let session = controller.connect(&host.endpoint_id()).unwrap();
    controller
        .submit_input(
            session.id,
            session.generation,
            InputEvent::key_down(crate::input::usage::A),
        )
        .unwrap();
    wait_applied(&applied, &[InputEvent::key_down(crate::input::usage::A)]);
    host.authorize(
        &controller.endpoint_id(),
        Grants {
            control: false,
            clipboard: true,
            transfer: false,
        },
    )
    .unwrap();
    controller
        .submit_input(
            session.id,
            session.generation,
            InputEvent::key_down(crate::input::usage::B),
        )
        .unwrap();
    // The downgraded key is refused, the loop ends, and the held key releases.
    wait_applied(
        &applied,
        &[
            InputEvent::key_down(crate::input::usage::A),
            InputEvent::key_up(crate::input::usage::A),
        ],
    );
    wait_closed(&host, SessionRole::Host);
    // The controller learns through the reject frame, so its side fails
    // instead of closing cleanly.
    wait_finished(&controller);
    assert!(matches!(
        controller
            .snapshot()
            .unwrap()
            .outgoing
            .as_ref()
            .map(|session| &session.phase),
        Some(SessionPhase::Failed(_))
    ));
    assert!(matches!(
        controller.connect(&host.endpoint_id()),
        Err(Error::Unauthorized)
    ));
}

fn wait_finished(controller: &crate::core::Core) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let done = controller
            .snapshot()
            .unwrap()
            .outgoing
            .as_ref()
            .is_some_and(|session| {
                matches!(
                    session.phase,
                    SessionPhase::Closed | SessionPhase::Failed(_)
                )
            });
        if done {
            return;
        }
        if std::time::Instant::now() > deadline {
            panic!("controller session did not finish");
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}
