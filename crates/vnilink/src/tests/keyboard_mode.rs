use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::error::Error;
use crate::events::CoreEvent;
use crate::input::{usage, InputEvent};
use crate::keyboard::KeyboardMode;
use crate::session::SessionPhase;

use super::keyboard_support::*;

#[test]
fn the_default_mode_inserts_at_from_either_keyboard() {
    let host = start_direct();
    let controller = start_direct();
    let applied = Arc::new(Mutex::new(Vec::new()));
    host.set_desktop(Box::new(recording(Arc::clone(&applied), false)))
        .unwrap();
    pair(&host, &controller);
    assert_eq!(controller.keyboard_mode().unwrap(), KeyboardMode::Character);
    let session = controller.connect(&host.endpoint_id()).unwrap();
    let events = controller.subscribe().unwrap();
    let host_events = host.subscribe().unwrap();
    controller
        .submit_local_key(
            session.id,
            session.generation,
            local(usage::GRAVE, true, "@"),
        )
        .unwrap();
    controller
        .submit_local_key(
            session.id,
            session.generation,
            local(usage::GRAVE, false, ""),
        )
        .unwrap();
    controller
        .submit_local_key(
            session.id,
            session.generation,
            local(usage::RIGHT_ALT, true, ""),
        )
        .unwrap();
    controller
        .submit_local_key(
            session.id,
            session.generation,
            local(usage::DIGIT_0, true, "@"),
        )
        .unwrap();
    controller
        .submit_local_key(
            session.id,
            session.generation,
            local(usage::DIGIT_0, false, ""),
        )
        .unwrap();
    controller
        .submit_local_key(
            session.id,
            session.generation,
            local(usage::RIGHT_ALT, false, ""),
        )
        .unwrap();
    wait_seen(
        &applied,
        &[Seen::Text("@".to_owned()), Seen::Text("@".to_owned())],
    );
    hides(&events, "@");
    hides(&host_events, "@");
}

#[test]
fn map_and_translate_are_optional_and_a_held_key_blocks_the_change() {
    let host = start_direct();
    let controller = start_direct();
    let applied = Arc::new(Mutex::new(Vec::new()));
    host.set_desktop(Box::new(recording(Arc::clone(&applied), false)))
        .unwrap();
    pair(&host, &controller);
    let session = controller.connect(&host.endpoint_id()).unwrap();
    controller
        .submit_local_key(
            session.id,
            session.generation,
            local(usage::LEFT_SHIFT, true, ""),
        )
        .unwrap();
    assert!(matches!(
        controller.set_keyboard_mode(KeyboardMode::Map),
        Err(Error::InvalidSetting(
            "release held keys before changing the keyboard mode"
        ))
    ));
    controller
        .submit_local_key(
            session.id,
            session.generation,
            local(usage::LEFT_SHIFT, false, ""),
        )
        .unwrap();
    controller.set_keyboard_mode(KeyboardMode::Map).unwrap();
    controller
        .submit_local_key(
            session.id,
            session.generation,
            local(usage::GRAVE, true, "@"),
        )
        .unwrap();
    wait_seen(&applied, &[Seen::Event(InputEvent::key_down(usage::GRAVE))]);
    controller
        .submit_local_key(
            session.id,
            session.generation,
            local(usage::GRAVE, false, ""),
        )
        .unwrap();
    controller
        .set_keyboard_mode(KeyboardMode::Translate)
        .unwrap();
    controller
        .submit_local_key(
            session.id,
            session.generation,
            local(usage::GRAVE, true, "@"),
        )
        .unwrap();
    wait_seen(
        &applied,
        &[
            Seen::Event(InputEvent::key_down(usage::GRAVE)),
            Seen::Event(InputEvent::key_up(usage::GRAVE)),
            Seen::Chord("@".to_owned()),
        ],
    );
    let events = host.subscribe().unwrap();
    hides(&events, "@");
}

#[test]
fn a_refused_chord_leaves_the_keys_working() {
    let host = start_direct();
    let controller = start_direct();
    let applied = Arc::new(Mutex::new(Vec::new()));
    host.set_desktop(Box::new(recording(Arc::clone(&applied), true)))
        .unwrap();
    pair(&host, &controller);
    let session = controller.connect(&host.endpoint_id()).unwrap();
    controller
        .set_keyboard_mode(KeyboardMode::Translate)
        .unwrap();
    let events = host.subscribe().unwrap();
    controller
        .submit_local_key(
            session.id,
            session.generation,
            local(usage::GRAVE, true, "@"),
        )
        .unwrap();
    wait_until(|| {
        events
            .recv_timeout(Duration::from_millis(20))
            .is_some_and(|event| {
                matches!(
                    event,
                    CoreEvent::InputFailed {
                        message: "translated keys are not available",
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
fn the_keyboard_mode_is_process_local_and_character_by_default() {
    let core = start_direct();
    assert_eq!(core.keyboard_mode().unwrap(), KeyboardMode::Character);
    core.set_keyboard_mode(KeyboardMode::Map).unwrap();
    assert_eq!(core.keyboard_mode().unwrap(), KeyboardMode::Map);
    core.set_keyboard_mode(KeyboardMode::Translate).unwrap();
    assert_eq!(core.keyboard_mode().unwrap(), KeyboardMode::Translate);
    core.shutdown();
    assert!(matches!(
        core.set_keyboard_mode(KeyboardMode::Character),
        Err(Error::ShutDown)
    ));
}

#[test]
fn input_defaults_cover_live_and_future_connections_and_refuse_held_input_atomically() {
    let hosts = [start_direct(), start_direct(), start_direct()];
    let controller = start_direct();
    for host in &hosts {
        host.set_desktop(Box::new(recording(Arc::new(Mutex::new(Vec::new())), false)))
            .unwrap();
        pair(host, &controller);
    }
    let first = controller.connect(&hosts[0].endpoint_id()).unwrap();
    let pending = controller.begin_connect(&hosts[1].endpoint_id()).unwrap();
    wait_until(|| controller.controller_session(pending.id).unwrap().phase == SessionPhase::Active);
    let second = controller.controller_session(pending.id).unwrap();
    let pointer = crate::PointerSettings {
        sensitivity_percent: 175,
        reverse_wheel: true,
        swap_buttons: true,
    };
    controller
        .set_input_defaults(KeyboardMode::Map, true, pointer)
        .unwrap();
    for session in [&first, &second] {
        assert_eq!(
            controller
                .session_keyboard_mode(session.id, session.generation)
                .unwrap(),
            KeyboardMode::Map
        );
        assert!(controller
            .session_swap_control_and_command(session.id, session.generation)
            .unwrap());
        assert_eq!(
            controller
                .session_pointer_settings(session.id, session.generation)
                .unwrap(),
            pointer
        );
    }
    controller
        .submit_local_key(
            first.id,
            first.generation,
            local(usage::LEFT_CONTROL, true, ""),
        )
        .unwrap();
    assert!(controller
        .set_input_defaults(
            KeyboardMode::Character,
            false,
            crate::PointerSettings::default()
        )
        .is_err());
    assert_eq!(controller.keyboard_mode().unwrap(), KeyboardMode::Map);
    assert_eq!(
        controller
            .session_pointer_settings(second.id, second.generation)
            .unwrap(),
        pointer
    );
    controller
        .release_session_input(first.id, first.generation)
        .unwrap();
    let pending = controller.begin_connect(&hosts[2].endpoint_id()).unwrap();
    wait_until(|| controller.controller_session(pending.id).unwrap().phase == SessionPhase::Active);
    let third = controller.controller_session(pending.id).unwrap();
    assert_eq!(
        controller
            .session_pointer_settings(third.id, third.generation)
            .unwrap(),
        pointer
    );
    assert_eq!(
        controller
            .session_keyboard_mode(third.id, third.generation)
            .unwrap(),
        KeyboardMode::Map
    );
    assert!(controller
        .set_input_defaults(
            KeyboardMode::Character,
            false,
            crate::PointerSettings {
                sensitivity_percent: 0,
                ..pointer
            }
        )
        .is_err());
    assert_eq!(
        controller
            .session_pointer_settings(third.id, third.generation)
            .unwrap(),
        pointer
    );
}
