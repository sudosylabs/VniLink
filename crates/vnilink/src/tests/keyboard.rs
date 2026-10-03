use crate::input::{usage, InputEvent};
use crate::keyboard::*;

fn down<'a>(usage: u16, text: &'a str) -> LocalKey<'a> {
    LocalKey {
        usage,
        down: true,
        text,
    }
}

fn up(usage: u16) -> LocalKey<'static> {
    LocalKey {
        usage,
        down: false,
        text: "",
    }
}

fn events(planned: &[Planned]) -> Vec<InputEvent> {
    planned
        .iter()
        .filter_map(|item| match item {
            Planned::Event(event) => Some(*event),
            Planned::Text(_) | Planned::Chord(_) => None,
        })
        .collect()
}

#[test]
fn character_mode_inserts_at_from_either_chord() {
    let mut plan = KeyPlan::new();
    let mac = plan
        .apply(KeyboardMode::Character, false, down(usage::GRAVE, "@"))
        .unwrap();
    assert_eq!(mac, vec![Planned::Text("@".to_owned())]);
    assert!(plan
        .apply(KeyboardMode::Character, false, up(usage::GRAVE))
        .unwrap()
        .is_empty());

    plan.apply(KeyboardMode::Character, false, down(usage::RIGHT_ALT, ""))
        .unwrap();
    let pc = plan
        .apply(KeyboardMode::Character, false, down(usage::DIGIT_0, "@"))
        .unwrap();
    assert_eq!(pc, vec![Planned::Text("@".to_owned())]);
    assert!(events(
        &plan
            .apply(KeyboardMode::Character, false, up(usage::DIGIT_0))
            .unwrap()
    )
    .is_empty());
    assert!(events(
        &plan
            .apply(KeyboardMode::Character, false, up(usage::RIGHT_ALT))
            .unwrap()
    )
    .is_empty());
}

#[test]
fn character_mode_inserts_the_french_m_and_keeps_shortcuts() {
    let mut plan = KeyPlan::new();
    let typed = plan
        .apply(KeyboardMode::Character, false, down(usage::SEMICOLON, "m"))
        .unwrap();
    assert_eq!(typed, vec![Planned::Text("m".to_owned())]);

    let mut plan = KeyPlan::new();
    let command = plan
        .apply(KeyboardMode::Character, true, down(usage::LEFT_META, ""))
        .unwrap();
    let letter = plan
        .apply(KeyboardMode::Character, true, down(usage::C, "c"))
        .unwrap();
    assert_eq!(
        events(&[command, letter].concat()),
        vec![
            InputEvent::key_down(usage::LEFT_CONTROL),
            InputEvent::key_down(usage::C),
        ]
    );
}

#[test]
fn shift_is_a_key_for_selection_and_not_for_a_symbol() {
    let mut plan = KeyPlan::new();
    assert!(plan
        .apply(KeyboardMode::Character, false, down(usage::LEFT_SHIFT, ""))
        .unwrap()
        .is_empty());
    let symbol = plan
        .apply(KeyboardMode::Character, false, down(usage::GRAVE, "#"))
        .unwrap();
    assert_eq!(symbol, vec![Planned::Text("#".to_owned())]);
    let arrow = plan
        .apply(KeyboardMode::Character, false, down(usage::LEFT, ""))
        .unwrap();
    assert_eq!(
        arrow,
        vec![
            Planned::Event(InputEvent::key_down(usage::LEFT_SHIFT)),
            Planned::Event(InputEvent::key_down(usage::LEFT)),
        ]
    );
}

#[test]
fn map_sends_the_position_and_translate_sends_a_chord() {
    let mut plan = KeyPlan::new();
    let mapped = plan
        .apply(KeyboardMode::Map, false, down(usage::GRAVE, "@"))
        .unwrap();
    assert_eq!(
        mapped,
        vec![Planned::Event(InputEvent::key_down(usage::GRAVE))]
    );

    let mut plan = KeyPlan::new();
    let translated = plan
        .apply(KeyboardMode::Translate, false, down(usage::GRAVE, "@"))
        .unwrap();
    assert_eq!(translated, vec![Planned::Chord("@".to_owned())]);
    let shortcut = plan
        .apply(
            KeyboardMode::Translate,
            false,
            down(usage::LEFT_CONTROL, ""),
        )
        .unwrap();
    let letter = plan
        .apply(KeyboardMode::Translate, false, down(usage::C, "c"))
        .unwrap();
    assert_eq!(
        events(&[shortcut, letter].concat()),
        vec![
            InputEvent::key_down(usage::LEFT_CONTROL),
            InputEvent::key_down(usage::C),
        ]
    );
}

#[test]
fn enter_and_a_missing_character_stay_physical() {
    let mut plan = KeyPlan::new();
    let entered = plan
        .apply(KeyboardMode::Character, false, down(usage::ENTER, "\n"))
        .unwrap();
    assert_eq!(
        entered,
        vec![Planned::Event(InputEvent::key_down(usage::ENTER))]
    );
    let bare = plan
        .apply(KeyboardMode::Character, false, down(usage::A, ""))
        .unwrap();
    assert_eq!(bare, vec![Planned::Event(InputEvent::key_down(usage::A))]);
}

#[test]
fn a_pc_french_row_resolves_at_one_and_bracket() {
    let mut keysyms = vec![0u32; 4 * 4];
    keysyms[0..4].copy_from_slice(&[0x26, 0x31, 0, 0]);
    keysyms[8..12].copy_from_slice(&[0xe0, 0x30, 0x40, 0]);
    keysyms[12..16].copy_from_slice(&[0x29, 0xb0, 0x5d, 0]);
    let map = HostKeymap {
        min_keycode: 8,
        keysyms_per_keycode: 4,
        keysyms,
    };
    assert_eq!(keysym_of('@'), 0x40);
    assert_eq!(keysym_of('é'), 0xe9);
    assert_eq!(keysym_of('€'), 0x0100_20ac);
    assert_eq!(
        find_chord(&map, keysym_of('@')),
        Some(LayoutChord {
            keycode: 10,
            level: ChordLevel::AltGr,
        })
    );
    assert_eq!(
        find_chord(&map, keysym_of('1')),
        Some(LayoutChord {
            keycode: 8,
            level: ChordLevel::Shift,
        })
    );
    assert_eq!(
        find_chord(&map, keysym_of(']')),
        Some(LayoutChord {
            keycode: 11,
            level: ChordLevel::AltGr,
        })
    );
    assert_eq!(
        find_chord(&map, keysym_of('&')),
        Some(LayoutChord {
            keycode: 8,
            level: ChordLevel::Plain,
        })
    );
    assert_eq!(find_chord(&map, keysym_of('€')), None);

    let mut both = map.keysyms.clone();
    both.extend_from_slice(&[0x40, 0, 0, 0]);
    let map = HostKeymap {
        keysyms: both,
        ..map
    };
    assert_eq!(
        find_chord(&map, keysym_of('@')),
        Some(LayoutChord {
            keycode: 12,
            level: ChordLevel::Plain,
        })
    );
}

#[test]
fn a_ten_wide_french_core_map_uses_group_one() {
    // Live `fr` basic rows, X core width 10. Columns 2 and 3 are the US
    // group. `@` there is not AltGr. Group 1 level 3 is column 4.
    let mut keysyms = vec![0u32; 10 * (38 - 11 + 1)];
    {
        let mut put = |keycode: u8, syms: [u32; 6]| {
            let start = usize::from(keycode - 11) * 10;
            keysyms[start..start + 6].copy_from_slice(&syms);
        };
        put(11, [0x00e9, 0x0032, 0x0032, 0x0040, 0x007e, 0x0ac3]);
        put(19, [0x00e0, 0x0030, 0x0030, 0x0029, 0x0040, 0x00b0]);
        put(20, [0x0029, 0x00b0, 0x002d, 0x005f, 0x005d, 0x00bf]);
        put(24, [0x0061, 0x0041, 0x0071, 0x0051, 0x00e6, 0x00c6]);
        put(38, [0x0071, 0x0051, 0x0061, 0x0041, 0x0040, 0x07d9]);
    }
    let map = HostKeymap {
        min_keycode: 11,
        keysyms_per_keycode: 10,
        keysyms,
    };
    assert_eq!(
        find_chord(&map, keysym_of('@')),
        Some(LayoutChord {
            keycode: 19,
            level: ChordLevel::AltGr,
        })
    );
    assert_eq!(
        find_chord(&map, keysym_of(']')),
        Some(LayoutChord {
            keycode: 20,
            level: ChordLevel::AltGr,
        })
    );
    assert_eq!(
        find_chord(&map, keysym_of('2')),
        Some(LayoutChord {
            keycode: 11,
            level: ChordLevel::Shift,
        })
    );
    assert_eq!(
        find_chord(&map, keysym_of('~')),
        Some(LayoutChord {
            keycode: 11,
            level: ChordLevel::AltGr,
        })
    );
    assert_eq!(
        find_chord(&map, keysym_of('æ')),
        Some(LayoutChord {
            keycode: 24,
            level: ChordLevel::AltGr,
        })
    );
    assert_eq!(
        find_chord(&map, keysym_of('¿')),
        Some(LayoutChord {
            keycode: 20,
            level: ChordLevel::ShiftAltGr,
        })
    );
    assert_eq!(
        find_chord(&map, keysym_of('q')),
        Some(LayoutChord {
            keycode: 38,
            level: ChordLevel::Plain,
        })
    );
    assert_eq!(
        find_chord(&map, keysym_of(')')),
        Some(LayoutChord {
            keycode: 20,
            level: ChordLevel::Plain,
        })
    );
}

#[test]
fn a_single_group_wide_row_skips_the_copied_columns() {
    // `XkbGetCoreMap` writes one group of four levels as ABABC D.
    let map = HostKeymap {
        min_keycode: 24,
        keysyms_per_keycode: 6,
        keysyms: vec![0x61, 0x41, 0x61, 0x41, 0xe6, 0xc6],
    };
    assert_eq!(
        find_chord(&map, keysym_of('a')),
        Some(LayoutChord {
            keycode: 24,
            level: ChordLevel::Plain,
        })
    );
    assert_eq!(
        find_chord(&map, keysym_of('æ')),
        Some(LayoutChord {
            keycode: 24,
            level: ChordLevel::AltGr,
        })
    );
    assert_eq!(
        find_chord(&map, keysym_of('Æ')),
        Some(LayoutChord {
            keycode: 24,
            level: ChordLevel::ShiftAltGr,
        })
    );
}
