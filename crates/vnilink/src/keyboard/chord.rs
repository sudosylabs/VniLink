// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

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

#[cfg(test)]
mod tests {
    use super::*;

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
}
