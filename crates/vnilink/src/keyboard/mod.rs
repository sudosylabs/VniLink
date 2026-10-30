// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! How a controller key becomes host input.
//!
//! [`crate::RemoteDesktopService::submit_local_key`] takes the physical HID
//! usage of a press and the text the controller operating system produced for
//! it; the [`KeyboardMode`] decides what the host receives. The mode and the
//! Control/Command swap are copied from [`crate::Core`]'s defaults when a
//! desktop opens, belong to that desktop afterwards, and are refused while
//! it holds keys or buttons. A new desktop starts with nothing held.
//!
//! Character mode inserts the characters the controller operating system
//! already produced, so `@` from the top-left key of a Mac French keyboard
//! and from AltGr+`0` of a PC French keyboard both arrive as `@`. Because the
//! host must not apply a chord of its own on top of the character, Shift, Alt
//! and AltGr stay on the controller until a key that produced no character
//! needs them (Shift+Left, Alt+Tab); then the planner presses them first and
//! releases them before the next character. Control and Command are always
//! keys: while one is held every press is physical, so Command+C stays a
//! shortcut. A press whose text is empty or holds a control character (Enter,
//! Escape, Backspace, Tab, arrows) is sent as its physical key, never as
//! inserted text. A composition step marked [`LocalKey::composing`] is
//! withheld, so a dead key is not injected and then committed again as text.
//!
//! Map mode sends every press as its physical usage and ignores the text, and
//! the host layout decides the glyph. Composition steps are sent too, so the
//! host can compose.
//!
//! Translate mode sends a typed character as a request for the host to press
//! the chord its layout uses (`ControlMessage::Chord`); shortcuts stay
//! physical as in Character mode. A host adapter that implements
//! [`crate::HostDesktop::type_chord`] answers it; the default refuses and
//! leaves the desktop up. Of the built-in injectors only Linux with X11 does
//! (`os_input`): it reads the host keymap and presses Shift and Right Alt
//! for the character, refusing one that group 1 of the keymap lacks.
//! Windows and macOS refuse Translate.
//!
//! The Control/Command swap exchanges the two (both sides) before a physical
//! key is queued; the host injects and releases the usage it received. It is
//! off by default. Finished text from
//! [`crate::RemoteDesktopService::submit_text`] is a separate path, a
//! phrase of at most [`crate::MAX_KEY_TEXT`] bytes without a null byte that
//! the host types as characters. It is neither a clipboard paste nor under
//! the clipboard grant, and is never logged or put on an event.
//!
//! Input frames carry the desktop's stamp (service id and generation) and the
//! host applies only those matching its desktop, so text queued for an old
//! generation is not applied to a new one. Releasing input drops what the
//! queue still holds.

mod capture;
mod chord;
mod layout;
mod plan;

pub use capture::SystemKeys;

pub(crate) use layout::layout_steps;
pub use layout::{HostLayout, LayoutStep, ModifierLevel};

/// How [`crate::RemoteDesktopService::submit_local_key`] turns one local key
/// into host input.
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
/// It is empty on release and when the key produced no character. `composing`
/// marks a dead key or an input-method step that has not committed a character.
/// The library does not log `text`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LocalKey<'a> {
    pub usage: u16,
    pub down: bool,
    pub text: &'a str,
    /// True when this transition is an uncommitted composition step.
    /// Character and Translate modes do not inject it. Map mode still sends
    /// the physical key so the host layout can compose.
    pub composing: bool,
}

impl<'a> LocalKey<'a> {
    pub const fn new(usage: u16, down: bool, text: &'a str) -> Self {
        Self {
            usage,
            down,
            text,
            composing: false,
        }
    }

    /// A dead key or other composition step with no committed character.
    pub const fn composing(usage: u16, down: bool) -> Self {
        Self {
            usage,
            down,
            text: "",
            composing: true,
        }
    }
}

pub(crate) use chord::{find_chord, keysym_of, ChordLevel, HostKeymap};
pub(crate) use plan::{KeyPlan, Planned};
