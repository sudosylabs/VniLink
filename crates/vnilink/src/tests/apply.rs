use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::error::Error;
use crate::events::CoreEvent;
use crate::input::InputEvent;
use crate::protocol::Quality;
use crate::session::{Grants, SessionPhase, SessionRole};

use super::support::*;

#[test]
fn keyboard_and_pointer_events_are_applied_and_released() {
    let host = start_direct();
    let controller = start_direct();
    let applied = Arc::new(Mutex::new(Vec::new()));
    host.set_desktop(Box::new(RecordingDesktop {
        events: Arc::clone(&applied),
    }))
    .unwrap();
    assert!(host.capabilities().keyboard);
    assert!(host.capabilities().pointer);
    assert_eq!(host.capabilities().monitors, 1);
    host.start_hosting().unwrap();
    let token = host.create_invitation(Duration::from_secs(60)).unwrap();
    controller.accept_invitation(&token).unwrap();
    let back = controller
        .create_invitation(Duration::from_secs(60))
        .unwrap();
    host.accept_invitation(&back).unwrap();
    host.set_desktop_sharing(Grants {
        view: true,
        control: true,
        clipboard: false,
        transfer: false,
    })
    .unwrap();
    let session = controller.connect(&host.endpoint_id()).unwrap();
    assert_eq!(session.monitor_width, 1920);
    assert_eq!(session.monitor_height, 1080);
    arm_display(&host, &controller, session.id, session.generation);
    for event in [
        InputEvent::key_down(crate::input::usage::A),
        InputEvent::pointer(1, 1, 1, 1),
        InputEvent::pointer(4, 5, 1, 1),
        InputEvent::pointer(9, 8, 1, 1),
        InputEvent::button_down(crate::input::PointerButton::Left),
        InputEvent::wheel(0, -1),
    ] {
        controller
            .submit_input(session.id, session.generation, event)
            .unwrap();
    }
    // Queue coalescing removes samples still queued together. A running
    // consumer may already have sent either intermediate position; it is not
    // required to wait until this entire producer burst has finished.
    let wait_for_tail = |tail: InputEvent| {
        let until = std::time::Instant::now() + Duration::from_secs(2);
        loop {
            let got = applied.lock().unwrap().clone();
            if got.last() == Some(&tail) {
                return got;
            }
            assert!(
                std::time::Instant::now() < until,
                "input tail did not arrive: {got:?}"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    };
    let valid: Vec<Vec<InputEvent>> = (0..4)
        .map(|mask| {
            let mut events = vec![InputEvent::key_down(crate::input::usage::A)];
            if mask & 1 != 0 {
                events.push(InputEvent::pointer(1, 1, 1, 1));
            }
            if mask & 2 != 0 {
                events.push(InputEvent::pointer(4, 5, 1, 1));
            }
            events.extend([
                InputEvent::pointer(9, 8, 1, 1),
                InputEvent::button_down(crate::input::PointerButton::Left),
                InputEvent::wheel(0, -1),
            ]);
            events
        })
        .collect();
    let got = wait_for_tail(InputEvent::wheel(0, -1));
    assert!(
        valid.contains(&got),
        "unexpected complete input sequence: {got:?}"
    );
    host.stop_hosted_session().unwrap();
    let released = wait_for_tail(InputEvent::key_up(crate::input::usage::A));
    let mut expected = got;
    expected.extend([
        InputEvent::button_up(crate::input::PointerButton::Left),
        InputEvent::key_up(crate::input::usage::A),
    ]);
    assert_eq!(released, expected);
}

#[test]
fn key_up_and_each_button_are_applied_and_not_released_again() {
    let host = start_direct();
    let controller = start_direct();
    let applied = Arc::new(Mutex::new(Vec::new()));
    host.set_desktop(Box::new(RecordingDesktop {
        events: Arc::clone(&applied),
    }))
    .unwrap();
    pair_for_control(&host, &controller);
    let session = controller.connect(&host.endpoint_id()).unwrap();
    let expected = [
        InputEvent::key_down(crate::input::usage::A),
        InputEvent::key_up(crate::input::usage::A),
        InputEvent::button_down(crate::input::PointerButton::Right),
        InputEvent::button_up(crate::input::PointerButton::Right),
        InputEvent::button_down(crate::input::PointerButton::Middle),
        InputEvent::button_up(crate::input::PointerButton::Middle),
        InputEvent::wheel(2, -3),
    ];
    for event in expected {
        controller
            .submit_input(session.id, session.generation, event)
            .unwrap();
    }
    wait_applied(&applied, &expected);
    host.stop_hosted_session().unwrap();
    wait_closed(&host, SessionRole::Host);
    wait_closed(&controller, SessionRole::Controller);
    let got = applied.lock().unwrap_or_else(|poison| poison.into_inner());
    assert_eq!(got.as_slice(), expected.as_slice());
}

#[test]
fn an_apply_failure_is_reported_and_the_session_stays_open() {
    let host = start_direct();
    let controller = start_direct();
    let applied = Arc::new(Mutex::new(Vec::new()));
    host.set_desktop(Box::new(FixedDesktop {
        width: 640,
        height: 480,
        events: Arc::clone(&applied),
        fail_key_down: Some(crate::input::usage::B),
        fail_release: false,
    }))
    .unwrap();
    let events = host.subscribe().unwrap();
    pair_for_control(&host, &controller);
    let session = controller.connect(&host.endpoint_id()).unwrap();
    assert_eq!(session.monitor_width, 640);
    assert_eq!(session.monitor_height, 480);
    controller
        .submit_input(
            session.id,
            session.generation,
            InputEvent::key_down(crate::input::usage::B),
        )
        .unwrap();
    assert_eq!(
        take_matching(&events, |event| {
            matches!(event, CoreEvent::InputFailed { .. })
        }),
        CoreEvent::InputFailed {
            id: session.id,
            message: "the operating system rejected the event",
        }
    );
    controller
        .submit_input(
            session.id,
            session.generation,
            InputEvent::key_down(crate::input::usage::A),
        )
        .unwrap();
    arm_display(&host, &controller, session.id, session.generation);
    controller
        .submit_input(
            session.id,
            session.generation,
            InputEvent::pointer(3, 4, 1, 1),
        )
        .unwrap();
    wait_applied(
        &applied,
        &[
            InputEvent::key_down(crate::input::usage::A),
            InputEvent::pointer(3, 4, 1, 1),
        ],
    );
    assert_eq!(
        host.snapshot().unwrap().incoming.unwrap().phase,
        SessionPhase::Active
    );
    controller
        .submit_input(
            session.id,
            session.generation,
            InputEvent::key_up(crate::input::usage::A),
        )
        .unwrap();
}

#[test]
fn a_failed_release_is_reported_on_the_hosted_session() {
    let host = start_direct();
    let controller = start_direct();
    let applied = Arc::new(Mutex::new(Vec::new()));
    host.set_desktop(Box::new(FixedDesktop {
        width: 800,
        height: 600,
        events: Arc::clone(&applied),
        fail_key_down: None,
        fail_release: true,
    }))
    .unwrap();
    let events = host.subscribe().unwrap();
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
    host.stop_hosted_session().unwrap();
    assert_eq!(
        take_matching(&events, |event| {
            matches!(event, CoreEvent::InputFailed { .. })
        }),
        CoreEvent::InputFailed {
            id: session.id,
            message: "a held key or button could not be released",
        }
    );
    wait_closed(&host, SessionRole::Host);
    let got = applied.lock().unwrap_or_else(|poison| poison.into_inner());
    assert_eq!(
        got.as_slice(),
        &[
            InputEvent::key_down(crate::input::usage::A),
            InputEvent::button_down(crate::input::PointerButton::Left),
        ]
    );
}

#[test]
fn quality_updates_propagate_from_controller_to_host() {
    let host = start_direct();
    let controller = start_direct();
    let host_events = host.subscribe().unwrap();
    let controller_events = controller.subscribe().unwrap();
    pair_for_control(&host, &controller);
    let session = controller.connect(&host.endpoint_id()).unwrap();
    // Negotiation stores the agreed limits on both sides at connect.
    assert!(session.quality.is_some());
    assert_eq!(
        host.snapshot().unwrap().incoming.unwrap().quality,
        session.quality
    );
    let quality = Quality::new(1_500, 30, 1280, 720).unwrap();
    controller
        .update_session_quality(session.id, SessionRole::Controller, quality)
        .unwrap();
    assert_eq!(
        controller.snapshot().unwrap().outgoing.unwrap().quality,
        Some(quality)
    );
    // The controller queues the change; the host applies it on receipt.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        if host.snapshot().unwrap().incoming.unwrap().quality == Some(quality) {
            break;
        }
        if std::time::Instant::now() > deadline {
            panic!("host did not apply the propagated quality");
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    let host_quality = Quality::new(800, 15, 640, 480).unwrap();
    host.update_session_quality(session.id, SessionRole::Host, host_quality)
        .unwrap();
    assert_eq!(
        host.snapshot().unwrap().incoming.unwrap().quality,
        Some(host_quality)
    );
    assert!(matches!(
        host.update_session_quality(session.id + 1, SessionRole::Host, quality),
        Err(Error::NoSession)
    ));
    assert!(matches!(
        controller.update_session_quality(0, SessionRole::Controller, quality),
        Err(Error::NoSession)
    ));
    assert_eq!(
        take_matching(&host_events, |event| {
            matches!(event, CoreEvent::QualityUpdated { .. })
        }),
        CoreEvent::QualityUpdated {
            id: session.id,
            quality,
        }
    );
    assert_eq!(
        take_matching(&controller_events, |event| {
            matches!(event, CoreEvent::QualityUpdated { .. })
        }),
        CoreEvent::QualityUpdated {
            id: session.id,
            quality,
        }
    );
}
