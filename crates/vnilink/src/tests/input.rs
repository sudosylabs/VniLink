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
            Outbound::Text(_)
            | Outbound::Chord(_)
            | Outbound::Quality(_)
            | Outbound::Displays(_) => {
                panic!("a text item was queued with the keys")
            }
        })
        .collect()
}

#[test]
fn pointer_samples_collapse_and_keys_do_not() {
    let outbox = InputOutbox::new();
    outbox.push(InputEvent::key_down(usage::A)).unwrap();
    outbox.push(InputEvent::pointer(1, 1, 1, 1)).unwrap();
    outbox.push(InputEvent::pointer(8, 9, 1, 1)).unwrap();
    outbox.push(InputEvent::pointer(3, 4, 1, 1)).unwrap();
    outbox.push(InputEvent::key_up(usage::A)).unwrap();
    assert_eq!(
        only_events(outbox.drain()),
        vec![
            InputEvent::key_down(usage::A),
            InputEvent::pointer(3, 4, 1, 1),
            InputEvent::key_up(usage::A),
        ]
    );
}

#[test]
fn ending_a_session_releases_held_keys_and_buttons_in_reverse() {
    let mut held = HeldInput::default();
    held.observe(InputEvent::key_down(usage::LEFT_SHIFT));
    held.observe(InputEvent::button_down(PointerButton::Left));
    held.observe(InputEvent::pointer(10, 10, 1, 1));
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
    outbox.push(InputEvent::pointer(1, 2, 1, 1)).unwrap();
    let drained = only_events(outbox.drain());
    assert_eq!(drained.len(), INPUT_QUEUE);
    assert!(drained
        .iter()
        .all(|event| matches!(event, InputEvent::Key { .. })));

    let outbox = InputOutbox::new();
    for usage_code in 0..(INPUT_QUEUE as u16 - 1) {
        outbox.push(InputEvent::key_down(usage_code)).unwrap();
    }
    outbox.push(InputEvent::pointer(1, 1, 1, 1)).unwrap();
    outbox.push(InputEvent::pointer(2, 2, 1, 1)).unwrap();
    let drained = only_events(outbox.drain());
    assert_eq!(drained.len(), INPUT_QUEUE);
    assert!(matches!(
        drained.last(),
        Some(InputEvent::Pointer { x: 2, y: 2, .. })
    ));
    assert!(!drained
        .iter()
        .any(|event| matches!(event, InputEvent::Pointer { x: 1, y: 1, .. })));

    let outbox = InputOutbox::new();
    outbox.push(InputEvent::key_down(1)).unwrap();
    outbox.push(InputEvent::pointer(1, 1, 1, 1)).unwrap();
    for usage_code in 2..INPUT_QUEUE as u16 {
        outbox.push(InputEvent::key_down(usage_code)).unwrap();
    }
    outbox.push(InputEvent::pointer(7, 8, 1, 1)).unwrap();
    let drained = only_events(outbox.drain());
    assert_eq!(drained.len(), INPUT_QUEUE);
    assert!(matches!(drained[0], InputEvent::Key { usage: 1, .. }));
    assert!(matches!(
        drained.last(),
        Some(InputEvent::Pointer { x: 7, y: 8, .. })
    ));
    assert!(!drained
        .iter()
        .any(|event| matches!(event, InputEvent::Pointer { x: 1, y: 1, .. })));
}

#[test]
fn the_first_fault_sticks_and_later_pushes_return_it() {
    let outbox = InputOutbox::new();
    outbox.fail(&Error::InputFailed(
        "the operating system rejected the event",
    ));
    outbox.fail(&Error::Unsupported("no desktop adapter is installed"));
    assert!(matches!(
        outbox.push(InputEvent::pointer(1, 1, 1, 1)),
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
    held.observe(InputEvent::pointer(4, 5, 1, 1));
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
        swap_command_key(InputEvent::pointer(1, 2, 1, 1)),
        InputEvent::pointer(1, 2, 1, 1)
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
        InputEvent::pointer(-12, 32768, 4, 2),
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
    let mut zero_display = Vec::new();
    InputEvent::pointer(1, 2, 0, 1).encode(&mut zero_display);
    assert!(matches!(
        InputEvent::decode(&zero_display),
        Err(Error::Malformed)
    ));
    let mut zero_revision = Vec::new();
    InputEvent::pointer(1, 2, 1, 0).encode(&mut zero_revision);
    assert!(matches!(
        InputEvent::decode(&zero_revision),
        Err(Error::Malformed)
    ));
    let mut too_far = Vec::new();
    InputEvent::pointer(32769, 0, 1, 1).encode(&mut too_far);
    assert!(matches!(
        InputEvent::decode(&too_far),
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

#[test]
fn topology_ids_origins_and_revisions_are_validated() {
    use crate::input::{DisplayInfo, MAX_DESKTOP_ORIGIN, MAX_DISPLAY_EDGE};
    let valid = |display: DisplayInfo| display;
    let base = DisplayInfo {
        id: 7,
        origin_x: 0,
        origin_y: 0,
        width: 1920,
        height: 1080,
        rotation_deg: 0,
        scale_percent: 100,
        revision: 3,
    };
    assert_eq!(crate::input::validate_topology(&[base]).unwrap(), 3);
    // Empty and oversized snapshots are refused.
    assert!(crate::input::validate_topology(&[]).is_err());
    assert!(crate::input::validate_topology(&vec![base; 17]).is_err());
    // Zero and duplicate ids are refused; a monitor index alone is not an id.
    assert!(crate::input::validate_topology(&[valid(DisplayInfo { id: 0, ..base })]).is_err());
    assert!(crate::input::validate_topology(&[base, base]).is_err());
    // Zero and oversized dimensions are refused.
    assert!(crate::input::validate_topology(&[valid(DisplayInfo { width: 0, ..base })]).is_err());
    assert!(crate::input::validate_topology(&[valid(DisplayInfo {
        width: MAX_DISPLAY_EDGE + 1,
        ..base
    })])
    .is_err());
    // Origins are bounded; unknown rotations and scales are refused.
    assert!(crate::input::validate_topology(&[valid(DisplayInfo {
        origin_x: MAX_DESKTOP_ORIGIN + 1,
        ..base
    })])
    .is_err());
    assert!(crate::input::validate_topology(&[valid(DisplayInfo {
        rotation_deg: 45,
        ..base
    })])
    .is_err());
    assert!(crate::input::validate_topology(&[valid(DisplayInfo {
        scale_percent: 24,
        ..base
    })])
    .is_err());
    assert!(crate::input::validate_topology(&[valid(DisplayInfo {
        scale_percent: 801,
        ..base
    })])
    .is_err());
    // Mixed revisions are refused.
    assert!(crate::input::validate_topology(&[
        base,
        DisplayInfo {
            id: 8,
            revision: 4,
            ..base
        }
    ])
    .is_err());
}

#[test]
fn topology_lookup_and_window_mapping_cover_negative_origins() {
    use crate::input::{desktop_bounds, display_for_point, map_window_to_desktop, DisplayInfo};
    let left = DisplayInfo {
        id: 11,
        origin_x: -1280,
        origin_y: 0,
        width: 1280,
        height: 1024,
        rotation_deg: 0,
        scale_percent: 100,
        revision: 9,
    };
    let right = DisplayInfo {
        id: 12,
        origin_x: 0,
        origin_y: -200,
        width: 1920,
        height: 1080,
        rotation_deg: 90,
        scale_percent: 200,
        revision: 9,
    };
    let topology = [left, right];
    assert_eq!(desktop_bounds(&topology), Some((-1280, -200, 3200, 1224)));
    // Points land on the intended display; the gap between them is empty.
    assert_eq!(display_for_point(-1280, 0, &topology), Some(left));
    assert_eq!(display_for_point(-1, 1023, &topology), Some(left));
    assert_eq!(display_for_point(0, -200, &topology), Some(right));
    assert_eq!(display_for_point(1919, 879, &topology), Some(right));
    assert_eq!(display_for_point(-1, 1024, &topology), None);
    assert_eq!(display_for_point(0, -201, &topology), None);
    assert_eq!(display_for_point(1920, 0, &topology), None);
    // Far edges are exclusive.
    assert_eq!(display_for_point(0, 880, &topology), None);

    // Window corners map onto the matching display through the negative origin.
    // The window matches the desktop size here, so mapping is a direct offset.
    let window = (3200, 1224);
    assert_eq!(
        map_window_to_desktop(0, 200, window, &topology, 9).unwrap(),
        (-1280, 0, 11)
    );
    let (x, y, id) = map_window_to_desktop(639, 640, window, &topology, 9).unwrap();
    assert_eq!((x, y, id), (-641, 440, 11));
    let (x, y, id) = map_window_to_desktop(1280, 0, window, &topology, 9).unwrap();
    assert_eq!((x, y, id), (0, -200, 12));
    let (x, y, id) = map_window_to_desktop(3199, 1079, window, &topology, 9).unwrap();
    assert_eq!((x, y, id), (1919, 879, 12));
    // The far edge is exclusive: the bottom row of the window maps past the
    // right display.
    assert!(map_window_to_desktop(1280, 1080, window, &topology, 9).is_err());
    // Outdated geometry is stale; zero window sides and bad topologies fail.
    assert!(matches!(
        map_window_to_desktop(0, 0, window, &topology, 8),
        Err(Error::StaleSession)
    ));
    assert!(map_window_to_desktop(0, 0, (0, 1280), &topology, 9).is_err());
    assert!(map_window_to_desktop(0, 0, window, &[], 9).is_err());
}

#[test]
fn pointer_motion_keeps_sub_counts_and_never_scales_absolute_positions() {
    use crate::input::{PointerPlan, PointerSettings};
    let mut plan = PointerPlan::default();
    plan.set(
        PointerSettings {
            sensitivity_percent: 1,
            ..PointerSettings::default()
        },
        false,
    )
    .unwrap();
    for _ in 0..99 {
        assert_eq!(plan.apply(InputEvent::motion(1, -1)).unwrap(), None);
    }
    assert_eq!(
        plan.apply(InputEvent::motion(1, -1)).unwrap(),
        Some(InputEvent::motion(1, -1))
    );
    assert_eq!(
        plan.apply(InputEvent::pointer(-20, 30, 1, 1)).unwrap(),
        Some(InputEvent::pointer(-20, 30, 1, 1))
    );
    assert!(plan
        .set(
            PointerSettings {
                sensitivity_percent: 0,
                ..PointerSettings::default()
            },
            false
        )
        .is_err());
    plan.set(
        PointerSettings {
            sensitivity_percent: 400,
            ..PointerSettings::default()
        },
        false,
    )
    .unwrap();
    assert_eq!(
        plan.apply(InputEvent::motion(i16::MAX, i16::MIN)).unwrap(),
        Some(InputEvent::motion(i16::MAX, i16::MIN))
    );
    let mut pending = 0;
    for _ in 0..7 {
        assert_eq!(crate::input::accumulate_detents(&mut pending, 15), 0);
    }
    assert_eq!(crate::input::accumulate_detents(&mut pending, 15), 1);
    assert_eq!(pending, 0);
    assert_eq!(crate::input::accumulate_detents(&mut pending, -119), 0);
    assert_eq!(crate::input::accumulate_detents(&mut pending, -1), -1);
}

#[test]
fn selecting_a_display_allows_an_unchanged_pointer_retry_and_survives_focus_loss() {
    let outbox = crate::input::InputOutbox::new();
    let point = InputEvent::pointer(1, 2, 1, 1);
    outbox.push(point).unwrap();
    assert_eq!(outbox.drain(), vec![crate::input::Outbound::Event(point)]);
    outbox.push(point).unwrap();
    assert!(outbox.drain().is_empty());
    outbox.push_displays(vec![1]).unwrap();
    outbox.push(point).unwrap();
    assert_eq!(
        outbox.drain(),
        vec![
            crate::input::Outbound::Displays(vec![1]),
            crate::input::Outbound::Event(point)
        ]
    );
    outbox.push_displays(vec![1]).unwrap();
    outbox.push(InputEvent::key_down(usage::A)).unwrap();
    outbox.clear_pending_input();
    assert_eq!(
        outbox.drain(),
        vec![crate::input::Outbound::Displays(vec![1])]
    );
}
