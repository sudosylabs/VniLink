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
fn unpair_forgets_the_device_stops_the_session_and_blocks_reconnect() {
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
    wait_applied(
        &applied,
        &[InputEvent::key_down(crate::input::usage::LEFT_SHIFT)],
    );
    host.unpair(&controller.endpoint_id()).unwrap();
    // Held input releases like any local stop.
    wait_applied(
        &applied,
        &[
            InputEvent::key_down(crate::input::usage::LEFT_SHIFT),
            InputEvent::key_up(crate::input::usage::LEFT_SHIFT),
        ],
    );
    wait_closed(&host, SessionRole::Host);
    wait_closed(&controller, SessionRole::Controller);
    // The device is gone, not kept with empty grants. This side forgot, so
    // the controller still dials — and the host rejects a peer it no longer
    // knows with Unauthorized instead of admitting it.
    assert!(host.snapshot().unwrap().devices.is_empty());
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
fn unpairing_an_unknown_device_reports_not_paired() {
    let host = start_direct();
    let controller = start_direct();
    assert!(matches!(
        host.unpair(&controller.endpoint_id()),
        Err(Error::NotPaired)
    ));
    // Pairing still works afterwards; nothing was disturbed.
    pair_for_control(&host, &controller);
    let session = controller.connect(&host.endpoint_id()).unwrap();
    assert_eq!(session.phase, SessionPhase::Active);
    // The caller forgetting its peer fails before dialing.
    controller.unpair(&host.endpoint_id()).unwrap();
    wait_closed(&controller, SessionRole::Controller);
    assert!(matches!(
        controller.connect(&host.endpoint_id()),
        Err(Error::NotPaired)
    ));
}

#[test]
fn a_controller_unpair_ends_its_outgoing_session() {
    let host = start_direct();
    let controller = start_direct();
    pair_for_control(&host, &controller);
    controller.connect(&host.endpoint_id()).unwrap();
    controller.unpair(&host.endpoint_id()).unwrap();
    assert!(controller.snapshot().unwrap().devices.is_empty());
    wait_closed(&controller, SessionRole::Controller);
    // Unpairing is local only: the host still lists the controller, and its
    // side closes because the control stream ended.
    wait_closed(&host, SessionRole::Host);
    assert_eq!(host.snapshot().unwrap().devices.len(), 1);
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

#[test]
fn quality_updates_select_the_role_with_both_directions_active() {
    use crate::protocol::Quality;

    let a = start_direct();
    let b = start_direct();
    // Each core hosts the other. Local handles are unique across roles;
    // role-specific quality APIs must still update only the selected direction.
    pair_for_control(&a, &b);
    pair_for_control(&b, &a);
    let b_out = b.connect(&a.endpoint_id()).unwrap();
    let a_out = a.connect(&b.endpoint_id()).unwrap();
    let a_host_id = a.snapshot().unwrap().incoming.unwrap().id;
    assert_ne!(a_host_id, a_out.id);
    assert_ne!(b.snapshot().unwrap().incoming.unwrap().id, b_out.id);

    let controller_quality = Quality::new(1_500, 30, 1280, 720).unwrap();
    a.update_session_quality(a_out.id, SessionRole::Controller, controller_quality)
        .unwrap();
    assert_eq!(
        a.snapshot().unwrap().outgoing.unwrap().quality,
        Some(controller_quality)
    );
    // The hosted session keeps the negotiated limits, not the controller's.
    assert_ne!(
        a.snapshot().unwrap().incoming.unwrap().quality,
        Some(controller_quality)
    );

    let host_quality = Quality::new(800, 15, 640, 480).unwrap();
    a.update_session_quality(a_host_id, SessionRole::Host, host_quality)
        .unwrap();
    assert_eq!(
        a.snapshot().unwrap().incoming.unwrap().quality,
        Some(host_quality)
    );
    assert_eq!(
        a.snapshot().unwrap().outgoing.unwrap().quality,
        Some(controller_quality)
    );
}
