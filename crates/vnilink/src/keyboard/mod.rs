//! How a controller key becomes host input.
//!
//! Character mode inserts the characters the controller operating system
//! already produced. Map mode sends the physical key. Translate mode asks the
//! host to press the chord in its own layout.

mod chord;
mod plan;

/// How [`crate::Core::submit_local_key`] turns one local key into host input.
///
/// The choice belongs to this process and is not saved. Character is the
/// default. Release held keys before changing it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(u8)]
pub enum KeyboardMode {
    /// Insert the character the controller produced. Shortcuts stay keys.
    #[default]
    Character = 0,
    /// Send the physical key. The host layout produces the character.
    /// This is RustDesk's Map mode.
    Map = 1,
    /// Ask the host to press the chord its layout uses for the character.
    /// This is RustDesk's Translate mode.
    Translate = 2,
}

impl KeyboardMode {
    pub(crate) fn from_u8(value: u8) -> Self {
        match value {
            1 => Self::Map,
            2 => Self::Translate,
            _ => Self::Character,
        }
    }
}

/// One key transition from the controller's operating system.
///
/// `usage` is the physical HID position. `text` is what that press produced.
/// It is empty on release and when the key produced no character. The library
/// does not log `text`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LocalKey<'a> {
    pub usage: u16,
    pub down: bool,
    pub text: &'a str,
}

#[cfg(test)]
pub(crate) use chord::LayoutChord;
pub(crate) use chord::{find_chord, keysym_of, ChordLevel, HostKeymap};
pub(crate) use plan::{KeyPlan, Planned};
