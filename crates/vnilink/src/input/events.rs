//! Pointer buttons, input events, and text checks.

use crate::error::Error;

use super::{usage, MAX_KEY_TEXT};

/// Button on the pointer device. Scroll is a separate event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PointerButton {
    Left = 1,
    Right = 2,
    Middle = 3,
}

fn direction(value: u8) -> Result<bool, Error> {
    match value {
        0 => Ok(false),
        1 => Ok(true),
        _ => Err(Error::Malformed),
    }
}

impl PointerButton {
    pub(crate) fn from_u8(value: u8) -> Result<Self, Error> {
        match value {
            1 => Ok(Self::Left),
            2 => Ok(Self::Right),
            3 => Ok(Self::Middle),
            _ => Err(Error::Malformed),
        }
    }
}

/// One keyboard or pointer change. Key and button transitions are ordered.
/// Consecutive pointer positions collapse to the newest one before they are sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputEvent {
    Key {
        usage: u16,
        down: bool,
    },
    Button {
        button: PointerButton,
        down: bool,
    },
    /// Absolute pixels on the host's one monitor. Origin is the top left.
    Pointer {
        x: u16,
        y: u16,
    },
    /// Wheel detents. Positive `dy` scrolls down. Positive `dx` scrolls right.
    Wheel {
        dx: i16,
        dy: i16,
    },
}

impl InputEvent {
    pub fn key_down(usage: u16) -> Self {
        Self::Key { usage, down: true }
    }

    pub fn key_up(usage: u16) -> Self {
        Self::Key { usage, down: false }
    }

    pub fn button_down(button: PointerButton) -> Self {
        Self::Button { button, down: true }
    }

    pub fn button_up(button: PointerButton) -> Self {
        Self::Button {
            button,
            down: false,
        }
    }

    pub fn pointer(x: u16, y: u16) -> Self {
        Self::Pointer { x, y }
    }

    pub fn wheel(dx: i16, dy: i16) -> Self {
        Self::Wheel { dx, dy }
    }

    pub(crate) fn encode(self, bytes: &mut Vec<u8>) {
        match self {
            Self::Key { usage, down } => {
                bytes.push(1);
                bytes.extend_from_slice(&usage.to_be_bytes());
                bytes.push(u8::from(down));
            }
            Self::Button { button, down } => {
                bytes.push(2);
                bytes.push(button as u8);
                bytes.push(u8::from(down));
            }
            Self::Pointer { x, y } => {
                bytes.push(3);
                bytes.extend_from_slice(&x.to_be_bytes());
                bytes.extend_from_slice(&y.to_be_bytes());
            }
            Self::Wheel { dx, dy } => {
                bytes.push(4);
                bytes.extend_from_slice(&dx.to_be_bytes());
                bytes.extend_from_slice(&dy.to_be_bytes());
            }
        }
    }

    pub(crate) fn decode(bytes: &[u8]) -> Result<Self, Error> {
        if bytes.is_empty() {
            return Err(Error::Malformed);
        }
        match bytes[0] {
            1 if bytes.len() == 4 => Ok(Self::Key {
                usage: u16::from_be_bytes([bytes[1], bytes[2]]),
                down: direction(bytes[3])?,
            }),
            1 => Err(Error::Malformed),
            2 if bytes.len() == 3 => Ok(Self::Button {
                button: PointerButton::from_u8(bytes[1])?,
                down: direction(bytes[2])?,
            }),
            2 => Err(Error::Malformed),
            3 if bytes.len() == 5 => Ok(Self::Pointer {
                x: u16::from_be_bytes([bytes[1], bytes[2]]),
                y: u16::from_be_bytes([bytes[3], bytes[4]]),
            }),
            3 => Err(Error::Malformed),
            4 if bytes.len() == 5 => Ok(Self::Wheel {
                dx: i16::from_be_bytes([bytes[1], bytes[2]]),
                dy: i16::from_be_bytes([bytes[3], bytes[4]]),
            }),
            _ => Err(Error::Malformed),
        }
    }
}

/// Rejects text the controller must not send. Empty text is not a clear.
pub(crate) fn check_key_text(text: &str) -> Result<(), Error> {
    if text.is_empty() {
        return Err(Error::InvalidSetting("text entry is empty"));
    }
    if let Some(message) = key_text_problem(text) {
        return Err(Error::InvalidSetting(message));
    }
    Ok(())
}

/// A problem in text that already arrived. `None` means the host may type it.
/// An empty string is not a problem; the host skips it.
pub(crate) fn key_text_problem(text: &str) -> Option<&'static str> {
    if text.len() > MAX_KEY_TEXT {
        Some("text entry is longer than 128 bytes")
    } else if text.as_bytes().contains(&0) {
        Some("text entry contains a null byte")
    } else {
        None
    }
}

/// Exchanges the physical Control and Command keys. Other keys stay put.
pub(crate) fn swap_command_key(event: InputEvent) -> InputEvent {
    match event {
        InputEvent::Key { usage, down } => InputEvent::Key {
            usage: match usage {
                usage::LEFT_CONTROL => usage::LEFT_META,
                usage::RIGHT_CONTROL => usage::RIGHT_META,
                usage::LEFT_META => usage::LEFT_CONTROL,
                usage::RIGHT_META => usage::RIGHT_CONTROL,
                other => other,
            },
            down,
        },
        other => other,
    }
}
