use std::collections::VecDeque;
use std::sync::Mutex;

use tokio::sync::Notify;

use crate::error::Error;

/// How many input events may wait to be written. Pointer movement collapses
/// instead of filling this queue. Key and button transitions do not.
pub(crate) const INPUT_QUEUE: usize = 64;

/// USB HID Keyboard/Keypad usages (page 0x07).
///
/// These name a physical key, not a character. `A` is the key a US keyboard
/// labels A. The host injects that key into its own layout.
pub mod usage {
    pub const A: u16 = 0x04;
    pub const B: u16 = 0x05;
    pub const C: u16 = 0x06;
    pub const D: u16 = 0x07;
    pub const E: u16 = 0x08;
    pub const F: u16 = 0x09;
    pub const G: u16 = 0x0A;
    pub const H: u16 = 0x0B;
    pub const I: u16 = 0x0C;
    pub const J: u16 = 0x0D;
    pub const K: u16 = 0x0E;
    pub const L: u16 = 0x0F;
    pub const M: u16 = 0x10;
    pub const N: u16 = 0x11;
    pub const O: u16 = 0x12;
    pub const P: u16 = 0x13;
    pub const Q: u16 = 0x14;
    pub const R: u16 = 0x15;
    pub const S: u16 = 0x16;
    pub const T: u16 = 0x17;
    pub const U: u16 = 0x18;
    pub const V: u16 = 0x19;
    pub const W: u16 = 0x1A;
    pub const X: u16 = 0x1B;
    pub const Y: u16 = 0x1C;
    pub const Z: u16 = 0x1D;
    pub const DIGIT_1: u16 = 0x1E;
    pub const DIGIT_2: u16 = 0x1F;
    pub const DIGIT_3: u16 = 0x20;
    pub const DIGIT_4: u16 = 0x21;
    pub const DIGIT_5: u16 = 0x22;
    pub const DIGIT_6: u16 = 0x23;
    pub const DIGIT_7: u16 = 0x24;
    pub const DIGIT_8: u16 = 0x25;
    pub const DIGIT_9: u16 = 0x26;
    pub const DIGIT_0: u16 = 0x27;
    pub const ENTER: u16 = 0x28;
    pub const ESCAPE: u16 = 0x29;
    pub const BACKSPACE: u16 = 0x2A;
    pub const TAB: u16 = 0x2B;
    pub const SPACE: u16 = 0x2C;
    pub const MINUS: u16 = 0x2D;
    pub const EQUAL: u16 = 0x2E;
    pub const LEFT_BRACKET: u16 = 0x2F;
    pub const RIGHT_BRACKET: u16 = 0x30;
    pub const BACKSLASH: u16 = 0x31;
    pub const SEMICOLON: u16 = 0x33;
    pub const APOSTROPHE: u16 = 0x34;
    pub const GRAVE: u16 = 0x35;
    pub const COMMA: u16 = 0x36;
    pub const PERIOD: u16 = 0x37;
    pub const SLASH: u16 = 0x38;
    pub const CAPS_LOCK: u16 = 0x39;
    pub const F1: u16 = 0x3A;
    pub const F2: u16 = 0x3B;
    pub const F3: u16 = 0x3C;
    pub const F4: u16 = 0x3D;
    pub const F5: u16 = 0x3E;
    pub const F6: u16 = 0x3F;
    pub const F7: u16 = 0x40;
    pub const F8: u16 = 0x41;
    pub const F9: u16 = 0x42;
    pub const F10: u16 = 0x43;
    pub const F11: u16 = 0x44;
    pub const F12: u16 = 0x45;
    pub const HOME: u16 = 0x4A;
    pub const PAGE_UP: u16 = 0x4B;
    pub const DELETE: u16 = 0x4C;
    pub const END: u16 = 0x4D;
    pub const PAGE_DOWN: u16 = 0x4E;
    pub const RIGHT: u16 = 0x4F;
    pub const LEFT: u16 = 0x50;
    pub const DOWN: u16 = 0x51;
    pub const UP: u16 = 0x52;
    pub const KEYPAD_ENTER: u16 = 0x58;
    pub const LEFT_CONTROL: u16 = 0xE0;
    pub const LEFT_SHIFT: u16 = 0xE1;
    pub const LEFT_ALT: u16 = 0xE2;
    pub const LEFT_META: u16 = 0xE3;
    pub const RIGHT_CONTROL: u16 = 0xE4;
    pub const RIGHT_SHIFT: u16 = 0xE5;
    pub const RIGHT_ALT: u16 = 0xE6;
    pub const RIGHT_META: u16 = 0xE7;
}

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

/// Keys and buttons this session has pressed and not yet released.
#[derive(Debug, Default)]
pub(crate) struct HeldInput {
    order: Vec<Held>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Held {
    Key(u16),
    Button(PointerButton),
}

impl HeldInput {
    pub(crate) fn observe(&mut self, event: InputEvent) {
        match event {
            InputEvent::Key { usage, down: true } => self.push(Held::Key(usage)),
            InputEvent::Key { usage, down: false } => self.remove(Held::Key(usage)),
            InputEvent::Button { button, down: true } => self.push(Held::Button(button)),
            InputEvent::Button {
                button,
                down: false,
            } => self.remove(Held::Button(button)),
            InputEvent::Pointer { .. } | InputEvent::Wheel { .. } => {}
        }
    }

    pub(crate) fn release_events(&self) -> Vec<InputEvent> {
        self.order
            .iter()
            .rev()
            .map(|held| match *held {
                Held::Key(usage) => InputEvent::key_up(usage),
                Held::Button(button) => InputEvent::button_up(button),
            })
            .collect()
    }

    fn push(&mut self, held: Held) {
        if !self.order.contains(&held) {
            self.order.push(held);
        }
    }

    fn remove(&mut self, held: Held) {
        self.order.retain(|seen| *seen != held);
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum InputFault {
    Unsupported(&'static str),
    Stale,
    Failed(&'static str),
    Malformed,
}

impl InputFault {
    pub(crate) fn from_error(error: &Error) -> Self {
        match error {
            Error::Unsupported(message) => Self::Unsupported(message),
            Error::StaleSession => Self::Stale,
            Error::InputFailed(message) => Self::Failed(message),
            Error::Malformed => Self::Malformed,
            _ => Self::Failed("input was rejected"),
        }
    }

    pub(crate) fn error(self) -> Error {
        match self {
            Self::Unsupported(message) => Error::Unsupported(message),
            Self::Stale => Error::StaleSession,
            Self::Failed(message) => Error::InputFailed(message),
            Self::Malformed => Error::Malformed,
        }
    }

    pub(crate) fn message(self) -> &'static str {
        match self {
            Self::Unsupported(message) | Self::Failed(message) => message,
            Self::Stale => "the input belongs to an old session",
            Self::Malformed => "the control message is malformed",
        }
    }
}

/// Controller-side queue. Pointer samples replace the pending sample.
pub(crate) struct InputOutbox {
    queue: Mutex<VecDeque<InputEvent>>,
    fault: Mutex<Option<InputFault>>,
    notify: Notify,
}

impl InputOutbox {
    pub(crate) fn new() -> Self {
        Self {
            queue: Mutex::new(VecDeque::new()),
            fault: Mutex::new(None),
            notify: Notify::new(),
        }
    }

    pub(crate) fn push(&self, event: InputEvent) -> Result<(), Error> {
        if let Some(fault) = self.fault() {
            return Err(fault.error());
        }
        let mut queue = self
            .queue
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        if let InputEvent::Pointer { .. } = event {
            if let Some(last) = queue.back_mut() {
                if matches!(last, InputEvent::Pointer { .. }) {
                    *last = event;
                    self.notify.notify_one();
                    return Ok(());
                }
            }
        }
        if queue.len() >= INPUT_QUEUE {
            if matches!(event, InputEvent::Pointer { .. }) {
                if let Some(index) = queue
                    .iter()
                    .position(|queued| matches!(queued, InputEvent::Pointer { .. }))
                {
                    queue.remove(index);
                } else {
                    return Ok(());
                }
            } else {
                return Err(Error::Busy);
            }
        }
        queue.push_back(event);
        drop(queue);
        self.notify.notify_one();
        Ok(())
    }

    pub(crate) fn drain(&self) -> Vec<InputEvent> {
        let mut queue = self
            .queue
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        queue.drain(..).collect()
    }

    pub(crate) fn fail(&self, error: &Error) {
        let mut fault = self
            .fault
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        if fault.is_none() {
            *fault = Some(InputFault::from_error(error));
        }
    }

    pub(crate) fn fault(&self) -> Option<InputFault> {
        *self
            .fault
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
    }

    pub(crate) fn notified(&self) -> tokio::sync::futures::Notified<'_> {
        self.notify.notified()
    }
}

/// Applies authorized input on the host. The native app can supply its own
/// implementation. [`crate::Core::enable_os_input`] installs one that injects
/// into the operating system.
pub trait HostDesktop: Send {
    fn apply(&mut self, event: InputEvent) -> Result<(), Error>;

    /// Pixel size of the one monitor this session controls. `(0, 0)` if unknown.
    fn monitor(&self) -> (u16, u16);
}
