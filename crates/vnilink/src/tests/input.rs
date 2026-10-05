use crate::error::Error;
use crate::input::{
    check_key_text, map_point, swap_command_key, usage, HeldInput, InputEvent, InputOutbox,
    Outbound, PointerButton, INPUT_QUEUE, MAX_KEY_TEXT,
};

fn only_events(items: Vec<Outbound>) -> Vec<InputEvent> {
    items
        .into_iter()
        .map(|item| match item {
            Outbound::Event(event) => event,
            Outbound::Text(_) | Outbound::Chord(_) | Outbound::Quality(_) => {
                panic!("a text item was queued with the keys")
            }
        })
        .collect()
}

#[test]
fn pointer_samples_collapse_and_keys_do_not() {
    let outbox = InputOutbox::new();
    outbox.push(InputEvent::key_down(usage::A)).unwrap();
    outbox.push(InputEvent::pointer(1, 1)).unwrap();
    outbox.push(InputEvent::pointer(8, 9)).unwrap();
    outbox.push(InputEvent::pointer(3, 4)).unwrap();
    outbox.push(InputEvent::key_up(usage::A)).unwrap();
    assert_eq!(
        only_events(outbox.drain()),
        vec![
            InputEvent::key_down(usage::A),
            InputEvent::pointer(3, 4),
            InputEvent::key_up(usage::A),
        ]
    );
}

#[test]
fn ending_a_session_releases_held_keys_and_buttons_in_reverse() {
    let mut held = HeldInput::default();
    held.observe(InputEvent::key_down(usage::LEFT_SHIFT));
    held.observe(InputEvent::button_down(PointerButton::Left));
    held.observe(InputEvent::pointer(10, 10));
    held.observe(InputEvent::key_down(usage::LEFT_SHIFT));
    assert_eq!(
        held.release_events(),
        vec![
            InputEvent::button_up(PointerButton::Left),
            InputEvent::key_up(usage::LEFT_SHIFT),
        ]
    );
}

#[test]
fn a_full_queue_keeps_key_transitions_and_folds_pointer_samples() {
    let outbox = InputOutbox::new();
    for usage_code in 0..INPUT_QUEUE as u16 {
        outbox.push(InputEvent::key_down(usage_code)).unwrap();
    }
    assert!(matches!(
        outbox.push(InputEvent::key_down(usage::A)),
        Err(Error::Busy)
    ));
    outbox.push(InputEvent::pointer(1, 2)).unwrap();
    let drained = only_events(outbox.drain());
    assert_eq!(drained.len(), INPUT_QUEUE);
    assert!(drained
        .iter()
        .all(|event| matches!(event, InputEvent::Key { .. })));

    let outbox = InputOutbox::new();
    for usage_code in 0..(INPUT_QUEUE as u16 - 1) {
        outbox.push(InputEvent::key_down(usage_code)).unwrap();
    }
    outbox.push(InputEvent::pointer(1, 1)).unwrap();
    outbox.push(InputEvent::pointer(2, 2)).unwrap();
    let drained = only_events(outbox.drain());
    assert_eq!(drained.len(), INPUT_QUEUE);
    assert!(matches!(
        drained.last(),
        Some(InputEvent::Pointer { x: 2, y: 2 })
    ));
    assert!(!drained
        .iter()
        .any(|event| matches!(event, InputEvent::Pointer { x: 1, y: 1 })));

    let outbox = InputOutbox::new();
    outbox.push(InputEvent::key_down(1)).unwrap();
    outbox.push(InputEvent::pointer(1, 1)).unwrap();
    for usage_code in 2..INPUT_QUEUE as u16 {
        outbox.push(InputEvent::key_down(usage_code)).unwrap();
    }
    outbox.push(InputEvent::pointer(7, 8)).unwrap();
    let drained = only_events(outbox.drain());
    assert_eq!(drained.len(), INPUT_QUEUE);
    assert!(matches!(drained[0], InputEvent::Key { usage: 1, .. }));
    assert!(matches!(
        drained.last(),
        Some(InputEvent::Pointer { x: 7, y: 8 })
    ));
    assert!(!drained
        .iter()
        .any(|event| matches!(event, InputEvent::Pointer { x: 1, y: 1 })));
}

#[test]
fn the_first_fault_sticks_and_later_pushes_return_it() {
    let outbox = InputOutbox::new();
    outbox.fail(&Error::InputFailed(
        "the operating system rejected the event",
    ));
    outbox.fail(&Error::Unsupported("no desktop adapter is installed"));
    assert!(matches!(
        outbox.push(InputEvent::pointer(1, 1)),
        Err(Error::InputFailed(
            "the operating system rejected the event"
        ))
    ));
    assert!(outbox.drain().is_empty());

    let outbox = InputOutbox::new();
    outbox.fail(&Error::StaleSession);
    assert!(matches!(
        outbox.push(InputEvent::key_down(usage::A)),
        Err(Error::StaleSession)
    ));

    let outbox = InputOutbox::new();
    outbox.fail(&Error::Malformed);
    assert!(matches!(
        outbox.push(InputEvent::key_up(usage::A)),
        Err(Error::Malformed)
    ));

    let outbox = InputOutbox::new();
    outbox.fail(&Error::Busy);
    assert!(matches!(
        outbox.push(InputEvent::key_down(usage::A)),
        Err(Error::InputFailed("input was rejected"))
    ));
}

#[test]
fn releasing_a_key_or_button_removes_it_from_the_held_set() {
    let mut held = HeldInput::default();
    held.observe(InputEvent::key_down(usage::A));
    held.observe(InputEvent::button_down(PointerButton::Left));
    held.observe(InputEvent::key_up(usage::A));
    held.observe(InputEvent::button_up(PointerButton::Left));
    held.observe(InputEvent::pointer(4, 5));
    held.observe(InputEvent::wheel(1, -1));
    assert!(held.release_events().is_empty());

    held.observe(InputEvent::key_down(usage::A));
    held.observe(InputEvent::key_down(usage::B));
    held.observe(InputEvent::button_down(PointerButton::Right));
    held.observe(InputEvent::key_up(usage::A));
    held.observe(InputEvent::button_down(PointerButton::Right));
    assert_eq!(
        held.release_events(),
        vec![
            InputEvent::button_up(PointerButton::Right),
            InputEvent::key_up(usage::B),
        ]
    );
}

#[test]
fn command_swap_exchanges_only_control_and_meta() {
    assert_eq!(
        swap_command_key(InputEvent::key_down(usage::LEFT_META)),
        InputEvent::key_down(usage::LEFT_CONTROL)
    );
    assert_eq!(
        swap_command_key(InputEvent::key_up(usage::LEFT_CONTROL)),
        InputEvent::key_up(usage::LEFT_META)
    );
    assert_eq!(
        swap_command_key(InputEvent::key_down(usage::RIGHT_CONTROL)),
        InputEvent::key_down(usage::RIGHT_META)
    );
    assert_eq!(
        swap_command_key(InputEvent::key_up(usage::RIGHT_META)),
        InputEvent::key_up(usage::RIGHT_CONTROL)
    );
    assert_eq!(
        swap_command_key(InputEvent::key_down(usage::LEFT_SHIFT)),
        InputEvent::key_down(usage::LEFT_SHIFT)
    );
    assert_eq!(
        swap_command_key(InputEvent::pointer(1, 2)),
        InputEvent::pointer(1, 2)
    );
}

#[test]
fn finished_text_is_bounded_and_cleared_with_the_queue() {
    assert!(check_key_text("héllo").is_ok());
    assert!(check_key_text(&"a".repeat(MAX_KEY_TEXT)).is_ok());
    assert!(matches!(
        check_key_text(""),
        Err(Error::InvalidSetting("text entry is empty"))
    ));
    assert!(matches!(
        check_key_text(&"a".repeat(MAX_KEY_TEXT + 1)),
        Err(Error::InvalidSetting("text entry is longer than 128 bytes"))
    ));
    assert!(matches!(
        check_key_text("a\0b"),
        Err(Error::InvalidSetting("text entry contains a null byte"))
    ));

    let outbox = InputOutbox::new();
    outbox.push(InputEvent::key_down(usage::A)).unwrap();
    outbox.push_text("héllo".to_owned()).unwrap();
    outbox.push(InputEvent::key_up(usage::A)).unwrap();
    let drained = outbox.drain();
    assert!(!format!("{drained:?}").contains("héllo"));
    assert_eq!(
        drained,
        vec![
            Outbound::Event(InputEvent::key_down(usage::A)),
            Outbound::Text("héllo".to_owned()),
            Outbound::Event(InputEvent::key_up(usage::A)),
        ]
    );

    let outbox = InputOutbox::new();
    outbox.push_text("gone".to_owned()).unwrap();
    outbox.clear();
    assert!(outbox.drain().is_empty());
    for _ in 0..INPUT_QUEUE {
        outbox.push(InputEvent::key_down(usage::A)).unwrap();
    }
    assert!(matches!(
        outbox.push_text("late".to_owned()),
        Err(Error::Busy)
    ));
}

#[test]
fn malformed_input_bytes_are_rejected() {
    let round_trips = [
        InputEvent::key_down(usage::DIGIT_0),
        InputEvent::key_up(usage::KEYPAD_ENTER),
        InputEvent::button_down(PointerButton::Middle),
        InputEvent::button_up(PointerButton::Middle),
        InputEvent::pointer(0, 65535),
        InputEvent::wheel(0, 0),
        InputEvent::wheel(i16::MIN, i16::MAX),
    ];
    for event in round_trips {
        let mut bytes = Vec::new();
        event.encode(&mut bytes);
        assert_eq!(InputEvent::decode(&bytes).unwrap(), event);
    }
    assert!(matches!(InputEvent::decode(&[]), Err(Error::Malformed)));
    assert!(matches!(InputEvent::decode(&[9]), Err(Error::Malformed)));
    assert!(matches!(
        InputEvent::decode(&[1, 0, 0x04]),
        Err(Error::Malformed)
    ));
    assert!(matches!(
        InputEvent::decode(&[1, 0, 0x04, 2]),
        Err(Error::Malformed)
    ));
    assert!(matches!(
        InputEvent::decode(&[2, 9, 1]),
        Err(Error::Malformed)
    ));
    assert!(matches!(
        InputEvent::decode(&[2, 1, 2]),
        Err(Error::Malformed)
    ));
    assert!(matches!(
        InputEvent::decode(&[3, 0, 1]),
        Err(Error::Malformed)
    ));
    assert!(matches!(
        InputEvent::decode(&[4, 0, 1]),
        Err(Error::Malformed)
    ));
}

#[test]
fn pointer_positions_scale_between_window_and_monitor() {
    // Identity and origin.
    assert_eq!(map_point(0, 0, (1920, 1080), (1920, 1080)), (0, 0));
    assert_eq!(
        map_point(1919, 1079, (1920, 1080), (1920, 1080)),
        (1919, 1079)
    );
    // Downscale by exactly two rounds to the nearest host pixel.
    assert_eq!(map_point(0, 0, (1920, 1080), (960, 540)), (0, 0));
    assert_eq!(map_point(960, 540, (1920, 1080), (960, 540)), (480, 270));
    assert_eq!(map_point(1, 1, (1920, 1080), (960, 540)), (1, 1));
    // Upscale and the far edge clamps onto the host instead of past it.
    assert_eq!(
        map_point(960, 540, (960, 540), (1920, 1080)),
        (1920 - 1, 1080 - 1)
    );
    assert_eq!(
        map_point(u16::MAX, u16::MAX, (960, 540), (1920, 1080)),
        (1919, 1079)
    );
    // A zero side has no scale to read or no pixels to name; axes are
    // independent, so only the zero side collapses.
    assert_eq!(map_point(50, 60, (0, 1080), (1920, 1080)), (0, 60));
    assert_eq!(map_point(50, 60, (1920, 1080), (0, 0)), (0, 0));
}
