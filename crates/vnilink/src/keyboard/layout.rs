//! Host layout tables for Translate mode. The table is the host's chords.
//! Character mode does not use it. A missing character is a refusal, not a
//! guess from another layout.

use crate::error::Error;
use crate::input::usage;

/// Modifiers that reach one character on the host layout.
///
/// AltGr is the third level. On X11 that is Right Alt. On macOS the same
/// level is Option, which the injector presses as Alt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ModifierLevel {
    Plain = 0,
    Shift = 1,
    AltGr = 2,
    ShiftAltGr = 3,
}

impl ModifierLevel {
    pub(crate) fn from_u8(value: u8) -> Option<Self> {
        Some(match value {
            0 => Self::Plain,
            1 => Self::Shift,
            2 => Self::AltGr,
            3 => Self::ShiftAltGr,
            _ => return None,
        })
    }

    /// Windows `VkKeyScan` shift byte. Control or Alt alone is a shortcut,
    /// not a character level. Control+Alt is AltGr.
    pub fn from_shift_state(state: u8) -> Option<Self> {
        match state & 0x07 {
            0 => Some(Self::Plain),
            1 => Some(Self::Shift),
            6 => Some(Self::AltGr),
            7 => Some(Self::ShiftAltGr),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Entry {
    character: char,
    usage: u16,
    level: ModifierLevel,
}

/// Characters the host can type by pressing one of its own keys.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostLayout {
    entries: Vec<Entry>,
}

impl HostLayout {
    /// Builds a table. The lowest modifier level wins for a repeated character.
    /// An empty table, a control character, or an unknown level is refused.
    pub fn from_entries(entries: &[(char, u16, u8)]) -> Result<Self, Error> {
        if entries.is_empty() {
            return Err(Error::InvalidSetting("the host layout is empty"));
        }
        let mut stored: Vec<Entry> = Vec::with_capacity(entries.len());
        for (character, usage, level) in entries {
            if character.is_control() || *usage == 0 {
                return Err(Error::InvalidSetting(
                    "a layout entry is not a typed character",
                ));
            }
            let Some(level) = ModifierLevel::from_u8(*level) else {
                return Err(Error::InvalidSetting("a layout modifier is invalid"));
            };
            if let Some(existing) = stored
                .iter_mut()
                .find(|entry| entry.character == *character)
            {
                if level < existing.level || (level == existing.level && *usage < existing.usage) {
                    *existing = Entry {
                        character: *character,
                        usage: *usage,
                        level,
                    };
                }
            } else {
                stored.push(Entry {
                    character: *character,
                    usage: *usage,
                    level,
                });
            }
        }
        Ok(Self { entries: stored })
    }

    pub fn chord(&self, character: char) -> Option<(u16, ModifierLevel)> {
        self.entries
            .iter()
            .find(|entry| entry.character == character)
            .map(|entry| (entry.usage, entry.level))
    }

    /// US QWERTY letters, digits, and the punctuation the physical keys name.
    pub fn us_qwerty() -> Self {
        let mut entries = Vec::new();
        for (index, character) in ('a'..='z').enumerate() {
            let usage = usage::A + u16::try_from(index).unwrap_or(0);
            entries.push((character, usage, 0));
            entries.push((character.to_ascii_uppercase(), usage, 1));
        }
        let digits = [
            (usage::DIGIT_1, '1', '!'),
            (usage::DIGIT_2, '2', '@'),
            (usage::DIGIT_3, '3', '#'),
            (usage::DIGIT_4, '4', '$'),
            (usage::DIGIT_5, '5', '%'),
            (usage::DIGIT_6, '6', '^'),
            (usage::DIGIT_7, '7', '&'),
            (usage::DIGIT_8, '8', '*'),
            (usage::DIGIT_9, '9', '('),
            (usage::DIGIT_0, '0', ')'),
        ];
        for (usage_code, plain, shifted) in digits {
            entries.push((plain, usage_code, 0));
            entries.push((shifted, usage_code, 1));
        }
        entries.extend([
            ('-', usage::MINUS, 0),
            ('_', usage::MINUS, 1),
            ('=', usage::EQUAL, 0),
            ('+', usage::EQUAL, 1),
            ('[', usage::LEFT_BRACKET, 0),
            ('{', usage::LEFT_BRACKET, 1),
            (']', usage::RIGHT_BRACKET, 0),
            ('}', usage::RIGHT_BRACKET, 1),
            ('\\', usage::BACKSLASH, 0),
            ('|', usage::BACKSLASH, 1),
            (';', usage::SEMICOLON, 0),
            (':', usage::SEMICOLON, 1),
            ('\'', usage::APOSTROPHE, 0),
            ('"', usage::APOSTROPHE, 1),
            ('`', usage::GRAVE, 0),
            ('~', usage::GRAVE, 1),
            (',', usage::COMMA, 0),
            ('<', usage::COMMA, 1),
            ('.', usage::PERIOD, 0),
            ('>', usage::PERIOD, 1),
            ('/', usage::SLASH, 0),
            ('?', usage::SLASH, 1),
            (' ', usage::SPACE, 0),
        ]);
        Self::from_entries(&entries).expect("the US table is valid")
    }

    /// French AZERTY letters and the digit row. `a` is the physical Q key.
    pub fn fr_azerty() -> Self {
        let mut entries = Vec::new();
        let letters = [
            ('a', 'A', usage::Q),
            ('z', 'Z', usage::W),
            ('e', 'E', usage::E),
            ('r', 'R', usage::R),
            ('t', 'T', usage::T),
            ('y', 'Y', usage::Y),
            ('u', 'U', usage::U),
            ('i', 'I', usage::I),
            ('o', 'O', usage::O),
            ('p', 'P', usage::P),
            ('q', 'Q', usage::A),
            ('s', 'S', usage::S),
            ('d', 'D', usage::D),
            ('f', 'F', usage::F),
            ('g', 'G', usage::G),
            ('h', 'H', usage::H),
            ('j', 'J', usage::J),
            ('k', 'K', usage::K),
            ('l', 'L', usage::L),
            ('m', 'M', usage::SEMICOLON),
            ('w', 'W', usage::Z),
            ('x', 'X', usage::X),
            ('c', 'C', usage::C),
            ('v', 'V', usage::V),
            ('b', 'B', usage::B),
            ('n', 'N', usage::N),
        ];
        for (plain, shifted, usage_code) in letters {
            entries.push((plain, usage_code, 0));
            entries.push((shifted, usage_code, 1));
        }
        let digits = [
            (usage::DIGIT_1, '&', '1'),
            (usage::DIGIT_2, 'é', '2'),
            (usage::DIGIT_3, '"', '3'),
            (usage::DIGIT_4, '\'', '4'),
            (usage::DIGIT_5, '(', '5'),
            (usage::DIGIT_6, '-', '6'),
            (usage::DIGIT_7, 'è', '7'),
            (usage::DIGIT_8, '_', '8'),
            (usage::DIGIT_9, 'ç', '9'),
            (usage::DIGIT_0, 'à', '0'),
        ];
        for (usage_code, plain, shifted) in digits {
            entries.push((plain, usage_code, 0));
            entries.push((shifted, usage_code, 1));
        }
        entries.push((' ', usage::SPACE, 0));
        Self::from_entries(&entries).expect("the AZERTY table is valid")
    }
}

/// One injection step. A click is a press and a release of the same key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LayoutStep {
    Down(u16),
    Up(u16),
    Click(u16),
}

/// The complete chord sequence for `text`, or an error if any character is
/// missing. A missing character does not return a partial sequence.
pub(crate) fn layout_steps(layout: &HostLayout, text: &str) -> Result<Vec<LayoutStep>, Error> {
    let mut steps = Vec::new();
    for character in text.chars() {
        let Some((usage_code, level)) = layout.chord(character) else {
            return Err(Error::InputFailed(
                "this character is not on the host keyboard",
            ));
        };
        match level {
            ModifierLevel::Plain => {}
            ModifierLevel::Shift => steps.push(LayoutStep::Down(usage::LEFT_SHIFT)),
            ModifierLevel::AltGr => steps.push(LayoutStep::Down(usage::RIGHT_ALT)),
            ModifierLevel::ShiftAltGr => {
                steps.push(LayoutStep::Down(usage::LEFT_SHIFT));
                steps.push(LayoutStep::Down(usage::RIGHT_ALT));
            }
        }
        steps.push(LayoutStep::Click(usage_code));
        match level {
            ModifierLevel::Plain => {}
            ModifierLevel::Shift => steps.push(LayoutStep::Up(usage::LEFT_SHIFT)),
            ModifierLevel::AltGr => steps.push(LayoutStep::Up(usage::RIGHT_ALT)),
            ModifierLevel::ShiftAltGr => {
                steps.push(LayoutStep::Up(usage::RIGHT_ALT));
                steps.push(LayoutStep::Up(usage::LEFT_SHIFT));
            }
        }
    }
    Ok(steps)
}
