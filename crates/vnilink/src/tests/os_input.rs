use std::collections::HashMap;

use crate::input::usage;
use crate::os_input::{clamp_extent, clamp_point, key_for_usage, platform_code, KeyCodes};

fn expect(macos: u16, windows: u16, linux: u16) -> KeyCodes {
    KeyCodes {
        macos,
        windows,
        linux,
    }
}

#[test]
fn hid_usages_map_to_physical_codes_without_injecting() {
    let mapped = [
        (usage::A, expect(0x00, 0x1E, 0x26)),
        (usage::B, expect(0x0B, 0x30, 0x38)),
        (usage::C, expect(0x08, 0x2E, 0x36)),
        (usage::D, expect(0x02, 0x20, 0x28)),
        (usage::E, expect(0x0E, 0x12, 0x1A)),
        (usage::F, expect(0x03, 0x21, 0x29)),
        (usage::G, expect(0x05, 0x22, 0x2A)),
        (usage::H, expect(0x04, 0x23, 0x2B)),
        (usage::I, expect(0x22, 0x17, 0x1F)),
        (usage::J, expect(0x26, 0x24, 0x2C)),
        (usage::K, expect(0x28, 0x25, 0x2D)),
        (usage::L, expect(0x25, 0x26, 0x2E)),
        (usage::M, expect(0x2E, 0x32, 0x3A)),
        (usage::N, expect(0x2D, 0x31, 0x39)),
        (usage::O, expect(0x1F, 0x18, 0x20)),
        (usage::P, expect(0x23, 0x19, 0x21)),
        (usage::Q, expect(0x0C, 0x10, 0x18)),
        (usage::R, expect(0x0F, 0x13, 0x1B)),
        (usage::S, expect(0x01, 0x1F, 0x27)),
        (usage::T, expect(0x11, 0x14, 0x1C)),
        (usage::U, expect(0x20, 0x16, 0x1E)),
        (usage::V, expect(0x09, 0x2F, 0x37)),
        (usage::W, expect(0x0D, 0x11, 0x19)),
        (usage::X, expect(0x07, 0x2D, 0x35)),
        (usage::Y, expect(0x10, 0x15, 0x1D)),
        (usage::Z, expect(0x06, 0x2C, 0x34)),
        (usage::DIGIT_1, expect(0x12, 0x02, 0x0A)),
        (usage::DIGIT_2, expect(0x13, 0x03, 0x0B)),
        (usage::DIGIT_3, expect(0x14, 0x04, 0x0C)),
        (usage::DIGIT_4, expect(0x15, 0x05, 0x0D)),
        (usage::DIGIT_5, expect(0x17, 0x06, 0x0E)),
        (usage::DIGIT_6, expect(0x16, 0x07, 0x0F)),
        (usage::DIGIT_7, expect(0x1A, 0x08, 0x10)),
        (usage::DIGIT_8, expect(0x1C, 0x09, 0x11)),
        (usage::DIGIT_9, expect(0x19, 0x0A, 0x12)),
        (usage::DIGIT_0, expect(0x1D, 0x0B, 0x13)),
        (usage::ENTER, expect(0x24, 0x1C, 0x24)),
        (usage::ESCAPE, expect(0x35, 0x01, 0x09)),
        (usage::BACKSPACE, expect(0x33, 0x0E, 0x16)),
        (usage::TAB, expect(0x30, 0x0F, 0x17)),
        (usage::SPACE, expect(0x31, 0x39, 0x41)),
        (usage::MINUS, expect(0x1B, 0x0C, 0x14)),
        (usage::EQUAL, expect(0x18, 0x0D, 0x15)),
        (usage::LEFT_BRACKET, expect(0x21, 0x1A, 0x22)),
        (usage::RIGHT_BRACKET, expect(0x1E, 0x1B, 0x23)),
        (usage::BACKSLASH, expect(0x2A, 0x2B, 0x33)),
        (usage::NON_US_HASH, expect(0x2A, 0x2B, 0x33)),
        (usage::SEMICOLON, expect(0x29, 0x27, 0x2F)),
        (usage::APOSTROPHE, expect(0x27, 0x28, 0x30)),
        (usage::GRAVE, expect(0x32, 0x29, 0x31)),
        (usage::COMMA, expect(0x2B, 0x33, 0x3B)),
        (usage::PERIOD, expect(0x2F, 0x34, 0x3C)),
        (usage::SLASH, expect(0x2C, 0x35, 0x3D)),
        (usage::CAPS_LOCK, expect(0x39, 0x3A, 0x42)),
        (usage::F1, expect(0x7A, 0x3B, 0x43)),
        (usage::F2, expect(0x78, 0x3C, 0x44)),
        (usage::F3, expect(0x63, 0x3D, 0x45)),
        (usage::F4, expect(0x76, 0x3E, 0x46)),
        (usage::F5, expect(0x60, 0x3F, 0x47)),
        (usage::F6, expect(0x61, 0x40, 0x48)),
        (usage::F7, expect(0x62, 0x41, 0x49)),
        (usage::F8, expect(0x64, 0x42, 0x4A)),
        (usage::F9, expect(0x65, 0x43, 0x4B)),
        (usage::F10, expect(0x6D, 0x44, 0x4C)),
        (usage::F11, expect(0x67, 0x57, 0x5F)),
        (usage::F12, expect(0x6F, 0x58, 0x60)),
        (usage::HOME, expect(0x73, 0xE047, 0x6E)),
        (usage::PAGE_UP, expect(0x74, 0xE049, 0x70)),
        (usage::DELETE, expect(0x75, 0xE053, 0x77)),
        (usage::END, expect(0x77, 0xE04F, 0x73)),
        (usage::PAGE_DOWN, expect(0x79, 0xE051, 0x75)),
        (usage::RIGHT, expect(0x7C, 0xE04D, 0x72)),
        (usage::LEFT, expect(0x7B, 0xE04B, 0x71)),
        (usage::DOWN, expect(0x7D, 0xE050, 0x74)),
        (usage::UP, expect(0x7E, 0xE048, 0x6F)),
        (usage::KEYPAD_ENTER, expect(0x4C, 0xE01C, 0x68)),
        (usage::NON_US_BACKSLASH, expect(0x0A, 0x56, 0x5E)),
        (usage::LEFT_CONTROL, expect(0x3B, 0x1D, 0x25)),
        (usage::LEFT_SHIFT, expect(0x38, 0x2A, 0x32)),
        (usage::LEFT_ALT, expect(0x3A, 0x38, 0x40)),
        (usage::LEFT_META, expect(0x37, 0xE05B, 0x85)),
        (usage::RIGHT_CONTROL, expect(0x3E, 0xE01D, 0x69)),
        (usage::RIGHT_SHIFT, expect(0x3C, 0x36, 0x3E)),
        (usage::RIGHT_ALT, expect(0x3D, 0xE038, 0x6C)),
        (usage::RIGHT_META, expect(0x36, 0xE05C, 0x86)),
    ];
    let mut macos = HashMap::new();
    let mut windows = HashMap::new();
    let mut linux = HashMap::new();
    for (usage_code, codes) in mapped {
        assert_eq!(key_for_usage(usage_code), Some(codes), "{usage_code:#x}");
        assert!(codes.linux <= u8::MAX.into(), "{usage_code:#x}");
        assert!(codes.macos < 0x80, "{usage_code:#x}");
        let high = codes.windows & 0xFF00;
        assert!(high == 0 || high == 0xE000, "{usage_code:#x} {high:#x}");
        remember(&mut macos, codes.macos, usage_code);
        remember(&mut windows, codes.windows, usage_code);
        remember(&mut linux, codes.linux, usage_code);
    }
    assert_eq!(
        key_for_usage(usage::NON_US_HASH),
        key_for_usage(usage::BACKSLASH)
    );
    assert_ne!(
        key_for_usage(usage::NON_US_BACKSLASH),
        key_for_usage(usage::BACKSLASH)
    );
    for (left, right) in [
        (usage::LEFT_CONTROL, usage::RIGHT_CONTROL),
        (usage::LEFT_SHIFT, usage::RIGHT_SHIFT),
        (usage::LEFT_ALT, usage::RIGHT_ALT),
        (usage::LEFT_META, usage::RIGHT_META),
    ] {
        let left = key_for_usage(left).unwrap();
        let right = key_for_usage(right).unwrap();
        assert_ne!(left.macos, right.macos);
        assert_ne!(left.windows, right.windows);
        assert_ne!(left.linux, right.linux);
    }
    assert_eq!(key_for_usage(0), None);
    assert_eq!(key_for_usage(0x46), None);
    assert_eq!(key_for_usage(0xFFFF), None);
    let letter_a = key_for_usage(usage::A).unwrap();
    #[cfg(target_os = "macos")]
    assert_eq!(platform_code(letter_a), letter_a.macos);
    #[cfg(target_os = "windows")]
    assert_eq!(platform_code(letter_a), letter_a.windows);
    #[cfg(all(unix, not(target_os = "macos")))]
    assert_eq!(platform_code(letter_a), letter_a.linux);
    assert_eq!(clamp_point(50, 0), 50);
    assert_eq!(clamp_point(0, 100), 0);
    assert_eq!(clamp_point(50, 100), 50);
    assert_eq!(clamp_point(100, 100), 99);
    assert_eq!(clamp_extent(0), 0);
    assert_eq!(clamp_extent(100), 100);
    assert_eq!(clamp_extent(-1), u16::MAX);
    assert_eq!(clamp_extent(i32::from(u16::MAX) + 1), u16::MAX);
}

fn remember(seen: &mut HashMap<u16, u16>, code: u16, usage_code: u16) {
    if let Some(previous) = seen.insert(code, usage_code) {
        let pair = (previous.min(usage_code), previous.max(usage_code));
        assert_eq!(
            pair,
            (usage::BACKSLASH, usage::NON_US_HASH),
            "{code:#x} is shared by {previous:#x} and {usage_code:#x}"
        );
    }
}
