use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::error::Error;
use crate::events::CoreEvent;
use crate::input::{usage, InputEvent, MAX_KEY_TEXT};
use crate::session::SessionPhase;

use super::keyboard_support::*;

#[test]
fn the_command_swap_is_process_local_and_off_by_default() {
    let core = start_direct();
    assert!(!core.swap_control_and_command().unwrap());
    core.set_swap_control_and_command(true).unwrap();
    assert!(core.swap_control_and_command().unwrap());
    core.set_swap_control_and_command(false).unwrap();
    assert!(!core.swap_control_and_command().unwrap());
    core.shutdown();
    assert!(matches!(
        core.set_swap_control_and_command(true),
        Err(Error::ShutDown)
    ));
}

#[test]
fn a_swapped_command_is_the_key_the_host_releases() {
    let host = start_direct();
    let controller = start_direct();
    let applied = Arc::new(Mutex::new(Vec::new()));
    host.set_desktop(Box::new(RecordingDesktop {
        seen: Arc::clone(&applied),
        fail_text: false,
        fail_chord: false,
    }))
    .unwrap();
    pair(&host, &controller);
    controller.set_swap_control_and_command(true).unwrap();
    let session = controller.connect(&host.endpoint_id()).unwrap();
    controller
        .submit_input(
            session.id,
            session.generation,
            InputEvent::key_down(usage::LEFT_META),
        )
        .unwrap();
    controller
        .submit_input(
            session.id,
            session.generation,
            InputEvent::key_down(usage::LEFT_SHIFT),
        )
        .unwrap();
    wait_seen(
        &applied,
        &[
            Seen::Event(InputEvent::key_down(usage::LEFT_CONTROL)),
            Seen::Event(InputEvent::key_down(usage::LEFT_SHIFT)),
        ],
    );
    host.stop_hosted_session().unwrap();
    wait_seen(
        &applied,
        &[
            Seen::Event(InputEvent::key_down(usage::LEFT_CONTROL)),
            Seen::Event(InputEvent::key_down(usage::LEFT_SHIFT)),
            Seen::Event(InputEvent::key_up(usage::LEFT_SHIFT)),
            Seen::Event(InputEvent::key_up(usage::LEFT_CONTROL)),
        ],
    );
}

#[test]
fn finished_text_is_typed_in_order_and_is_not_logged() {
    let host = start_direct();
    let controller = start_direct();
    let applied = Arc::new(Mutex::new(Vec::new()));
    host.set_desktop(Box::new(RecordingDesktop {
        seen: Arc::clone(&applied),
        fail_text: false,
        fail_chord: false,
    }))
    .unwrap();
    pair(&host, &controller);
    let session = controller.connect(&host.endpoint_id()).unwrap();
    let events = controller.subscribe().unwrap();
    let host_events = host.subscribe().unwrap();
    assert!(matches!(
        controller.submit_text(session.id, session.generation, ""),
        Err(Error::InvalidSetting("text entry is empty"))
    ));
    assert!(matches!(
        controller.submit_text(
            session.id,
            session.generation,
            &"a".repeat(MAX_KEY_TEXT + 1)
        ),
        Err(Error::InvalidSetting("text entry is longer than 128 bytes"))
    ));
    assert!(matches!(
        controller.submit_text(session.id, session.generation, "a\0b"),
        Err(Error::InvalidSetting("text entry contains a null byte"))
    ));
    controller
        .submit_input(
            session.id,
            session.generation,
            InputEvent::key_down(usage::LEFT_META),
        )
        .unwrap();
    controller
        .submit_text(session.id, session.generation, "café")
        .unwrap();
    controller
        .submit_input(
            session.id,
            session.generation,
            InputEvent::key_down(usage::ENTER),
        )
        .unwrap();
    wait_seen(
        &applied,
        &[
            Seen::Event(InputEvent::key_down(usage::LEFT_META)),
            Seen::Text("café".to_owned()),
            Seen::Event(InputEvent::key_down(usage::ENTER)),
        ],
    );
    hides(&events, "café");
    hides(&host_events, "café");
    assert_eq!(
        host.snapshot().unwrap().incoming.unwrap().phase,
        SessionPhase::Active
    );
}

#[test]
fn a_refused_text_leaves_the_keys_working() {
    let host = start_direct();
    let controller = start_direct();
    let applied = Arc::new(Mutex::new(Vec::new()));
    host.set_desktop(Box::new(RecordingDesktop {
        seen: Arc::clone(&applied),
        fail_text: true,
        fail_chord: false,
    }))
    .unwrap();
    pair(&host, &controller);
    let session = controller.connect(&host.endpoint_id()).unwrap();
    let events = host.subscribe().unwrap();
    controller
        .submit_text(session.id, session.generation, "nope")
        .unwrap();
    wait_until(|| {
        events
            .recv_timeout(Duration::from_millis(20))
            .is_some_and(|event| {
                matches!(
                    event,
                    CoreEvent::InputFailed {
                        message: "the operating system rejected the event",
                        ..
                    }
                )
            })
    });
    controller
        .submit_input(
            session.id,
            session.generation,
            InputEvent::key_down(usage::A),
        )
        .unwrap();
    wait_seen(&applied, &[Seen::Event(InputEvent::key_down(usage::A))]);
    assert_eq!(
        host.snapshot().unwrap().incoming.unwrap().phase,
        SessionPhase::Active
    );
}

#[test]
fn a_dropped_session_does_not_replay_finished_text() {
    let host = start_direct();
    let controller = start_direct();
    let applied = Arc::new(Mutex::new(Vec::new()));
    host.set_desktop(Box::new(RecordingDesktop {
        seen: Arc::clone(&applied),
        fail_text: false,
        fail_chord: false,
    }))
    .unwrap();
    pair(&host, &controller);
    let session = controller.connect(&host.endpoint_id()).unwrap();
    controller
        .submit_text(session.id, session.generation, "one")
        .unwrap();
    wait_seen(&applied, &[Seen::Text("one".to_owned())]);
    controller.drop_link().unwrap();
    wait_until(|| {
        controller
            .snapshot()
            .unwrap()
            .outgoing
            .as_ref()
            .is_some_and(|outgoing| outgoing.phase != SessionPhase::Active)
    });
    assert!(matches!(
        controller.submit_text(session.id, session.generation, "replayed"),
        Err(Error::NoSession | Error::StaleSession)
    ));
    wait_until(|| {
        let outgoing = controller.snapshot().unwrap().outgoing.unwrap();
        outgoing.phase == SessionPhase::Active && outgoing.generation != session.generation
    });
    let again = controller.snapshot().unwrap().outgoing.unwrap();
    controller
        .submit_text(again.id, again.generation, "two")
        .unwrap();
    wait_seen(
        &applied,
        &[Seen::Text("one".to_owned()), Seen::Text("two".to_owned())],
    );
}
