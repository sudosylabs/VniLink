//! Geometry and physical keyboard mapping, independent of GTK and video hardware.

/// Map a point in an aspect-fit picture to the negotiated host monitor.
/// Letterbox space is not part of the remote desktop.
pub fn pointer(
    x: f64,
    y: f64,
    view: (f64, f64),
    frame: (u16, u16),
    host: (u16, u16),
) -> Option<(u16, u16)> {
    if frame.0 == 0 || frame.1 == 0 || view.0 <= 0.0 || view.1 <= 0.0 {
        return None;
    }
    let scale = (view.0 / f64::from(frame.0)).min(view.1 / f64::from(frame.1));
    let width = f64::from(frame.0) * scale;
    let height = f64::from(frame.1) * scale;
    let left = (view.0 - width) / 2.0;
    let top = (view.1 - height) / 2.0;
    if x < left || y < top || x >= left + width || y >= top + height {
        return None;
    }
    let host = if host.0 == 0 || host.1 == 0 {
        frame
    } else {
        host
    };
    Some((
        ((x - left) / width * f64::from(host.0)) as u16,
        ((y - top) / height * f64::from(host.1)) as u16,
    ))
}

/// GTK's XKB hardware keycode is the Linux evdev position plus eight.
/// Map positions rather than translated key symbols, preserving layouts.
pub fn key_usage(keycode: u32) -> Option<u16> {
    let code = keycode.checked_sub(8)?;
    Some(match code {
        1 => 0x29,
        2..=10 => 0x1e + (code - 2) as u16,
        11 => 0x27,
        12 => 0x2d,
        13 => 0x2e,
        14 => 0x2a,
        15 => 0x2b,
        16 => 0x14,
        17 => 0x1a,
        18 => 0x08,
        19 => 0x15,
        20 => 0x17,
        21 => 0x1c,
        22 => 0x18,
        23 => 0x0c,
        24 => 0x12,
        25 => 0x13,
        26 => 0x2f,
        27 => 0x30,
        28 => 0x28,
        29 => 0xe0,
        30 => 0x04,
        31 => 0x16,
        32 => 0x07,
        33 => 0x09,
        34 => 0x0a,
        35 => 0x0b,
        36 => 0x0d,
        37 => 0x0e,
        38 => 0x0f,
        39 => 0x33,
        40 => 0x34,
        41 => 0x35,
        42 => 0xe1,
        43 => 0x31,
        44 => 0x1d,
        45 => 0x1b,
        46 => 0x06,
        47 => 0x19,
        48 => 0x05,
        49 => 0x11,
        50 => 0x10,
        51 => 0x36,
        52 => 0x37,
        53 => 0x38,
        54 => 0xe5,
        55 => 0x55,
        56 => 0xe2,
        57 => 0x2c,
        58 => 0x39,
        59..=68 => 0x3a + (code - 59) as u16,
        69 => 0x53,
        70 => 0x47,
        71 => 0x5f,
        72 => 0x60,
        73 => 0x61,
        74 => 0x56,
        75 => 0x5c,
        76 => 0x5d,
        77 => 0x5e,
        78 => 0x57,
        79 => 0x59,
        80 => 0x5a,
        81 => 0x5b,
        82 => 0x62,
        83 => 0x63,
        86 => 0x64,
        87 => 0x44,
        88 => 0x45,
        96 => 0x58,
        97 => 0xe4,
        98 => 0x54,
        100 => 0xe6,
        102 => 0x4a,
        103 => 0x52,
        104 => 0x4b,
        105 => 0x50,
        106 => 0x4f,
        107 => 0x4d,
        108 => 0x51,
        109 => 0x4e,
        110 => 0x49,
        111 => 0x4c,
        125 => 0xe3,
        126 => 0xe7,
        127 => 0x65,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fitted_pointer_excludes_bars_and_scales_to_original_monitor() {
        assert_eq!(
            pointer(0., 0., (1000., 1000.), (1920, 1080), (3840, 2160)),
            None
        );
        assert_eq!(
            pointer(500., 500., (1000., 1000.), (1920, 1080), (3840, 2160)),
            Some((1920, 1080))
        );
        assert_eq!(
            pointer(999., 780., (1000., 1000.), (1920, 1080), (3840, 2160)),
            Some((3836, 2155))
        );
        assert_eq!(pointer(500., 500., (1000., 1000.), (0, 0), (0, 0)), None);
    }
    #[test]
    fn keys_keep_positions_and_distinct_modifiers() {
        assert_eq!(key_usage(38), Some(vnilink::usage::A));
        assert_eq!(key_usage(24), Some(vnilink::usage::Q));
        assert_eq!(key_usage(37), Some(0xe0));
        assert_eq!(key_usage(105), Some(0xe4));
        assert_eq!(key_usage(50), Some(0xe1));
        assert_eq!(key_usage(62), Some(0xe5));
        assert_eq!(key_usage(113), Some(vnilink::usage::LEFT));
        assert_eq!(key_usage(0), None);
        assert_eq!(key_usage(500), None);
    }
}
