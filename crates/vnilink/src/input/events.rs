//! Pointer buttons, input events, and text checks.

use crate::error::Error;

use super::{usage, MAX_KEY_TEXT};

/// Button on the pointer device. Scroll is a separate event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PointerButton {
    Left = 1,
    Right = 2,
    Middle = 3,
    /// The side button usually labeled Back.
    Back = 4,
    /// The side button usually labeled Forward.
    Forward = 5,
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
            4 => Ok(Self::Back),
            5 => Ok(Self::Forward),
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
    /// Desktop pixels on one display. The origin may be negative. `display`
    /// is the stable id and `revision` is the topology snapshot the controller
    /// used. A mismatch is rejected and is not moved onto another display.
    Pointer {
        x: i32,
        y: i32,
        display: u64,
        revision: u64,
    },
    /// Wheel detents. Positive `dy` scrolls down. Positive `dx` scrolls right.
    Wheel {
        dx: i16,
        dy: i16,
    },
    /// High-resolution scroll. [`super::WHEEL_DETENT`] units are one detent.
    /// Positive `dy` scrolls down. Positive `dx` scrolls right. A value
    /// outside ±[`super::MAX_PRECISE_WHEEL`] is malformed.
    PreciseWheel {
        dx: i32,
        dy: i32,
    },
    /// Relative device counts, not desktop pixels. Pointer sensitivity applies
    /// only to this event. `(0, 0)` is not motion.
    Motion {
        dx: i16,
        dy: i16,
    },
}

impl InputEvent {
    /// What a record may say of this event: its kind, never which key or
    /// where (`crate::trace`).
    pub fn kind(self) -> &'static str {
        match self {
            Self::Key { down: true, .. } => "key down",
            Self::Key { down: false, .. } => "key up",
            Self::Button { down: true, .. } => "button down",
            Self::Button { down: false, .. } => "button up",
            Self::Pointer { .. } => "pointer",
            Self::Wheel { .. } => "wheel",
            Self::PreciseWheel { .. } => "precise wheel",
            Self::Motion { .. } => "motion",
        }
    }

    pub(crate) fn validate(self) -> Result<(), Error> {
        let mut wire = Vec::with_capacity(25);
        self.encode(&mut wire);
        Self::decode(&wire).map(|_| ())
    }

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

    pub fn pointer(x: i32, y: i32, display: u64, revision: u64) -> Self {
        Self::Pointer {
            x,
            y,
            display,
            revision,
        }
    }

    pub fn wheel(dx: i16, dy: i16) -> Self {
        Self::Wheel { dx, dy }
    }

    pub fn precise_wheel(dx: i32, dy: i32) -> Self {
        Self::PreciseWheel { dx, dy }
    }

    pub fn motion(dx: i16, dy: i16) -> Self {
        Self::Motion { dx, dy }
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
            Self::Pointer {
                x,
                y,
                display,
                revision,
            } => {
                bytes.push(3);
                bytes.extend_from_slice(&x.to_be_bytes());
                bytes.extend_from_slice(&y.to_be_bytes());
                bytes.extend_from_slice(&display.to_be_bytes());
                bytes.extend_from_slice(&revision.to_be_bytes());
            }
            Self::Wheel { dx, dy } => {
                bytes.push(4);
                bytes.extend_from_slice(&dx.to_be_bytes());
                bytes.extend_from_slice(&dy.to_be_bytes());
            }
            Self::PreciseWheel { dx, dy } => {
                bytes.push(5);
                bytes.extend_from_slice(&dx.to_be_bytes());
                bytes.extend_from_slice(&dy.to_be_bytes());
            }
            Self::Motion { dx, dy } => {
                bytes.push(6);
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
            3 if bytes.len() == 25 => {
                let x = i32::from_be_bytes([bytes[1], bytes[2], bytes[3], bytes[4]]);
                let y = i32::from_be_bytes([bytes[5], bytes[6], bytes[7], bytes[8]]);
                let display = u64::from_be_bytes(bytes[9..17].try_into().expect("8 bytes"));
                let revision = u64::from_be_bytes(bytes[17..25].try_into().expect("8 bytes"));
                let bound = u32::try_from(super::MAX_DESKTOP_ORIGIN).unwrap_or(u32::MAX);
                if display == 0
                    || revision == 0
                    || x.unsigned_abs() > bound
                    || y.unsigned_abs() > bound
                {
                    return Err(Error::Malformed);
                }
                Ok(Self::Pointer {
                    x,
                    y,
                    display,
                    revision,
                })
            }
            3 => Err(Error::Malformed),
            4 if bytes.len() == 5 => Ok(Self::Wheel {
                dx: i16::from_be_bytes([bytes[1], bytes[2]]),
                dy: i16::from_be_bytes([bytes[3], bytes[4]]),
            }),
            5 if bytes.len() == 9 => {
                let dx = i32::from_be_bytes([bytes[1], bytes[2], bytes[3], bytes[4]]);
                let dy = i32::from_be_bytes([bytes[5], bytes[6], bytes[7], bytes[8]]);
                if dx == 0 && dy == 0
                    || dx.unsigned_abs() > super::MAX_PRECISE_WHEEL as u32
                    || dy.unsigned_abs() > super::MAX_PRECISE_WHEEL as u32
                {
                    return Err(Error::Malformed);
                }
                Ok(Self::PreciseWheel { dx, dy })
            }
            6 if bytes.len() == 5 => {
                let dx = i16::from_be_bytes([bytes[1], bytes[2]]);
                let dy = i16::from_be_bytes([bytes[3], bytes[4]]);
                if dx == 0 && dy == 0 {
                    return Err(Error::Malformed);
                }
                Ok(Self::Motion { dx, dy })
            }
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

/// Exchanges the primary buttons. Back, Forward, and Middle stay put.
/// A drag that starts on one button must end on that same swapped button.
pub(crate) fn swap_pointer_button(event: InputEvent) -> InputEvent {
    match event {
        InputEvent::Button { button, down } => InputEvent::Button {
            button: match button {
                PointerButton::Left => PointerButton::Right,
                PointerButton::Right => PointerButton::Left,
                other => other,
            },
            down,
        },
        other => other,
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
