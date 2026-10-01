use enigo::Key;

use crate::input::usage;
use crate::os_input::{clamp_extent, clamp_point, key_for_usage};

#[test]
fn letters_modifiers_and_arrows_map_without_injecting() {
    assert_eq!(key_for_usage(usage::A), Some(Key::Unicode('a')));
    assert_eq!(key_for_usage(usage::ESCAPE), Some(Key::Escape));
    assert_eq!(key_for_usage(usage::LEFT_SHIFT), Some(Key::Shift));
    assert_eq!(key_for_usage(usage::LEFT), Some(Key::LeftArrow));
    assert_eq!(key_for_usage(0), None);
}

#[test]
fn every_named_usage_maps_without_opening_the_injector() {
    let mapped = [
        (usage::A, Key::Unicode('a')),
        (usage::B, Key::Unicode('b')),
        (usage::C, Key::Unicode('c')),
        (usage::D, Key::Unicode('d')),
        (usage::E, Key::Unicode('e')),
        (usage::F, Key::Unicode('f')),
        (usage::G, Key::Unicode('g')),
        (usage::H, Key::Unicode('h')),
        (usage::I, Key::Unicode('i')),
        (usage::J, Key::Unicode('j')),
        (usage::K, Key::Unicode('k')),
        (usage::L, Key::Unicode('l')),
        (usage::M, Key::Unicode('m')),
        (usage::N, Key::Unicode('n')),
        (usage::O, Key::Unicode('o')),
        (usage::P, Key::Unicode('p')),
        (usage::Q, Key::Unicode('q')),
        (usage::R, Key::Unicode('r')),
        (usage::S, Key::Unicode('s')),
        (usage::T, Key::Unicode('t')),
        (usage::U, Key::Unicode('u')),
        (usage::V, Key::Unicode('v')),
        (usage::W, Key::Unicode('w')),
        (usage::X, Key::Unicode('x')),
        (usage::Y, Key::Unicode('y')),
        (usage::Z, Key::Unicode('z')),
        (usage::DIGIT_1, Key::Unicode('1')),
        (usage::DIGIT_2, Key::Unicode('2')),
        (usage::DIGIT_3, Key::Unicode('3')),
        (usage::DIGIT_4, Key::Unicode('4')),
        (usage::DIGIT_5, Key::Unicode('5')),
        (usage::DIGIT_6, Key::Unicode('6')),
        (usage::DIGIT_7, Key::Unicode('7')),
        (usage::DIGIT_8, Key::Unicode('8')),
        (usage::DIGIT_9, Key::Unicode('9')),
        (usage::DIGIT_0, Key::Unicode('0')),
        (usage::ENTER, Key::Return),
        (usage::KEYPAD_ENTER, Key::Return),
        (usage::ESCAPE, Key::Escape),
        (usage::BACKSPACE, Key::Backspace),
        (usage::TAB, Key::Tab),
        (usage::SPACE, Key::Space),
        (usage::MINUS, Key::Unicode('-')),
        (usage::EQUAL, Key::Unicode('=')),
        (usage::LEFT_BRACKET, Key::Unicode('[')),
        (usage::RIGHT_BRACKET, Key::Unicode(']')),
        (usage::BACKSLASH, Key::Unicode('\\')),
        (usage::SEMICOLON, Key::Unicode(';')),
        (usage::APOSTROPHE, Key::Unicode('\'')),
        (usage::GRAVE, Key::Unicode('`')),
        (usage::COMMA, Key::Unicode(',')),
        (usage::PERIOD, Key::Unicode('.')),
        (usage::SLASH, Key::Unicode('/')),
        (usage::CAPS_LOCK, Key::CapsLock),
        (usage::F1, Key::F1),
        (usage::F2, Key::F2),
        (usage::F3, Key::F3),
        (usage::F4, Key::F4),
        (usage::F5, Key::F5),
        (usage::F6, Key::F6),
        (usage::F7, Key::F7),
        (usage::F8, Key::F8),
        (usage::F9, Key::F9),
        (usage::F10, Key::F10),
        (usage::F11, Key::F11),
        (usage::F12, Key::F12),
        (usage::HOME, Key::Home),
        (usage::PAGE_UP, Key::PageUp),
        (usage::DELETE, Key::Delete),
        (usage::END, Key::End),
        (usage::PAGE_DOWN, Key::PageDown),
        (usage::RIGHT, Key::RightArrow),
        (usage::LEFT, Key::LeftArrow),
        (usage::DOWN, Key::DownArrow),
        (usage::UP, Key::UpArrow),
        (usage::LEFT_CONTROL, Key::Control),
        (usage::RIGHT_CONTROL, Key::Control),
        (usage::LEFT_SHIFT, Key::Shift),
        (usage::RIGHT_SHIFT, Key::Shift),
        (usage::LEFT_ALT, Key::Alt),
        (usage::RIGHT_ALT, Key::Alt),
        (usage::LEFT_META, Key::Meta),
        (usage::RIGHT_META, Key::Meta),
    ];
    for (usage_code, key) in mapped {
        assert_eq!(key_for_usage(usage_code), Some(key), "{usage_code}");
    }
    assert_eq!(key_for_usage(0xFFFF), None);
    assert_eq!(clamp_point(50, 0), 50);
    assert_eq!(clamp_point(0, 100), 0);
    assert_eq!(clamp_point(50, 100), 50);
    assert_eq!(clamp_point(100, 100), 99);
    assert_eq!(clamp_extent(0), 0);
    assert_eq!(clamp_extent(100), 100);
    assert_eq!(clamp_extent(-1), u16::MAX);
    assert_eq!(clamp_extent(i32::from(u16::MAX) + 1), u16::MAX);
}
