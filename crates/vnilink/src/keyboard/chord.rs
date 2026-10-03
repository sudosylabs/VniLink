//! Host keymap lookup for Translate mode. Tests build the map without a display.

/// Modifier level of one group-1 keysym. Level 2 is Right Alt.
///
/// A layout whose third level is not Right Alt still gets Right Alt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ChordLevel {
    Plain,
    Shift,
    AltGr,
    ShiftAltGr,
}

/// The host key that produces one character, and the modifiers that reach it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct LayoutChord {
    pub keycode: u8,
    pub level: ChordLevel,
}

/// X11 `GetKeyboardMapping` result. Tests build this without a display.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HostKeymap {
    pub min_keycode: u8,
    pub keysyms_per_keycode: u8,
    pub keysyms: Vec<u32>,
}

/// X11 keysym for a character. Latin-1 maps to itself. `0` means there is none.
pub(crate) fn keysym_of(character: char) -> u32 {
    let ucs = character as u32;
    if (0x0020..=0x007e).contains(&ucs) || (0x00a0..=0x00ff).contains(&ucs) {
        return ucs;
    }
    if (0xd800..=0xdfff).contains(&ucs)
        || (0xfdd0..=0xfdef).contains(&ucs)
        || (ucs & 0xfffe) == 0xfffe
    {
        return 0;
    }
    0x0100_0000 | ucs
}

/// Group-1 columns of one core keycode, lowest level first.
///
/// At most four keysyms follow the core protocol: those columns are the four
/// levels. A wider row follows `XkbGetCoreMap` (XKB protocol section 12.4).
/// Group 1 levels 1 and 2 are columns 0 and 1, and levels 3 and 4 are columns
/// 4 and 5. Columns 2 and 3 are the next group, or a copy of the first two
/// when the key has one group. Only group 1 is searched.
fn group1_columns(per: usize) -> [(usize, u8); 4] {
    if per <= 4 {
        [(0, 0), (1, 1), (2, 2), (3, 3)]
    } else {
        [(0, 0), (1, 1), (4, 2), (5, 3)]
    }
}

/// Lowest group-1 level, then the lowest keycode, that produces `keysym`.
pub(crate) fn find_chord(map: &HostKeymap, keysym: u32) -> Option<LayoutChord> {
    if keysym == 0 || map.keysyms_per_keycode == 0 {
        return None;
    }
    let per = usize::from(map.keysyms_per_keycode);
    let keys = map.keysyms.len() / per;
    let mut best: Option<(u8, u8)> = None;
    for index in 0..keys {
        let base = index * per;
        for (column, level) in group1_columns(per) {
            if column >= per || map.keysyms[base + column] != keysym {
                continue;
            }
            let code =
                u16::from(map.min_keycode).saturating_add(u16::try_from(index).unwrap_or(u16::MAX));
            let Ok(keycode) = u8::try_from(code) else {
                break;
            };
            let replace = best.is_none_or(|(old_level, _)| level < old_level);
            if replace {
                best = Some((level, keycode));
            }
            break;
        }
    }
    best.map(|(level, keycode)| LayoutChord {
        keycode,
        level: match level {
            0 => ChordLevel::Plain,
            1 => ChordLevel::Shift,
            2 => ChordLevel::AltGr,
            _ => ChordLevel::ShiftAltGr,
        },
    })
}
