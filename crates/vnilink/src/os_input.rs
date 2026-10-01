use enigo::{
    Axis, Button, Coordinate, Direction, Enigo, Key, Keyboard, Mouse, NewConError, Settings,
};

use crate::error::Error;
use crate::input::{usage, HostDesktop, InputEvent, PointerButton};

/// Injects key and pointer events with the operating system's input API.
///
/// macOS uses Core Graphics and needs Accessibility permission. The first call
/// may show the system prompt. Windows uses SendInput, which does not reach
/// an elevated or secure desktop. Linux uses X11 by default; Wayland is not
/// claimed. This does not capture or mirror the screen.
pub struct OsInput {
    enigo: Enigo,
    width: u16,
    height: u16,
}

impl OsInput {
    pub fn open() -> Result<Self, Error> {
        let settings = Settings {
            open_prompt_to_get_permissions: true,
            release_keys_when_dropped: true,
            independent_of_keyboard_state: true,
            ..Settings::default()
        };
        let enigo = Enigo::new(&settings).map_err(setup_error)?;
        let (width, height) = match enigo.main_display() {
            Ok((width, height)) => (clamp_extent(width), clamp_extent(height)),
            Err(_) => (0, 0),
        };
        Ok(Self {
            enigo,
            width,
            height,
        })
    }
}

impl HostDesktop for OsInput {
    fn apply(&mut self, event: InputEvent) -> Result<(), Error> {
        match event {
            InputEvent::Key { usage, down } => {
                let key = key_for_usage(usage).ok_or(Error::InputFailed(
                    "this key is not mapped on this operating system",
                ))?;
                let direction = if down {
                    Direction::Press
                } else {
                    Direction::Release
                };
                self.enigo.key(key, direction).map_err(apply_error)
            }
            InputEvent::Button { button, down } => {
                let direction = if down {
                    Direction::Press
                } else {
                    Direction::Release
                };
                self.enigo
                    .button(enigo_button(button), direction)
                    .map_err(apply_error)
            }
            InputEvent::Pointer { x, y } => {
                let x = clamp_point(x, self.width);
                let y = clamp_point(y, self.height);
                self.enigo
                    .move_mouse(x, y, Coordinate::Abs)
                    .map_err(apply_error)
            }
            InputEvent::Wheel { dx, dy } => {
                if dx != 0 {
                    self.enigo
                        .scroll(i32::from(dx), Axis::Horizontal)
                        .map_err(apply_error)?;
                }
                if dy != 0 {
                    self.enigo
                        .scroll(i32::from(dy), Axis::Vertical)
                        .map_err(apply_error)?;
                }
                Ok(())
            }
        }
    }

    fn monitor(&self) -> (u16, u16) {
        (self.width, self.height)
    }
}

pub(crate) fn key_for_usage(usage_code: u16) -> Option<Key> {
    Some(match usage_code {
        usage::A => Key::Unicode('a'),
        usage::B => Key::Unicode('b'),
        usage::C => Key::Unicode('c'),
        usage::D => Key::Unicode('d'),
        usage::E => Key::Unicode('e'),
        usage::F => Key::Unicode('f'),
        usage::G => Key::Unicode('g'),
        usage::H => Key::Unicode('h'),
        usage::I => Key::Unicode('i'),
        usage::J => Key::Unicode('j'),
        usage::K => Key::Unicode('k'),
        usage::L => Key::Unicode('l'),
        usage::M => Key::Unicode('m'),
        usage::N => Key::Unicode('n'),
        usage::O => Key::Unicode('o'),
        usage::P => Key::Unicode('p'),
        usage::Q => Key::Unicode('q'),
        usage::R => Key::Unicode('r'),
        usage::S => Key::Unicode('s'),
        usage::T => Key::Unicode('t'),
        usage::U => Key::Unicode('u'),
        usage::V => Key::Unicode('v'),
        usage::W => Key::Unicode('w'),
        usage::X => Key::Unicode('x'),
        usage::Y => Key::Unicode('y'),
        usage::Z => Key::Unicode('z'),
        usage::DIGIT_1 => Key::Unicode('1'),
        usage::DIGIT_2 => Key::Unicode('2'),
        usage::DIGIT_3 => Key::Unicode('3'),
        usage::DIGIT_4 => Key::Unicode('4'),
        usage::DIGIT_5 => Key::Unicode('5'),
        usage::DIGIT_6 => Key::Unicode('6'),
        usage::DIGIT_7 => Key::Unicode('7'),
        usage::DIGIT_8 => Key::Unicode('8'),
        usage::DIGIT_9 => Key::Unicode('9'),
        usage::DIGIT_0 => Key::Unicode('0'),
        usage::ENTER | usage::KEYPAD_ENTER => Key::Return,
        usage::ESCAPE => Key::Escape,
        usage::BACKSPACE => Key::Backspace,
        usage::TAB => Key::Tab,
        usage::SPACE => Key::Space,
        usage::MINUS => Key::Unicode('-'),
        usage::EQUAL => Key::Unicode('='),
        usage::LEFT_BRACKET => Key::Unicode('['),
        usage::RIGHT_BRACKET => Key::Unicode(']'),
        usage::BACKSLASH => Key::Unicode('\\'),
        usage::SEMICOLON => Key::Unicode(';'),
        usage::APOSTROPHE => Key::Unicode('\''),
        usage::GRAVE => Key::Unicode('`'),
        usage::COMMA => Key::Unicode(','),
        usage::PERIOD => Key::Unicode('.'),
        usage::SLASH => Key::Unicode('/'),
        usage::CAPS_LOCK => Key::CapsLock,
        usage::F1 => Key::F1,
        usage::F2 => Key::F2,
        usage::F3 => Key::F3,
        usage::F4 => Key::F4,
        usage::F5 => Key::F5,
        usage::F6 => Key::F6,
        usage::F7 => Key::F7,
        usage::F8 => Key::F8,
        usage::F9 => Key::F9,
        usage::F10 => Key::F10,
        usage::F11 => Key::F11,
        usage::F12 => Key::F12,
        usage::HOME => Key::Home,
        usage::PAGE_UP => Key::PageUp,
        usage::DELETE => Key::Delete,
        usage::END => Key::End,
        usage::PAGE_DOWN => Key::PageDown,
        usage::RIGHT => Key::RightArrow,
        usage::LEFT => Key::LeftArrow,
        usage::DOWN => Key::DownArrow,
        usage::UP => Key::UpArrow,
        usage::LEFT_CONTROL | usage::RIGHT_CONTROL => Key::Control,
        usage::LEFT_SHIFT | usage::RIGHT_SHIFT => Key::Shift,
        usage::LEFT_ALT | usage::RIGHT_ALT => Key::Alt,
        usage::LEFT_META | usage::RIGHT_META => Key::Meta,
        _ => return None,
    })
}

fn enigo_button(button: PointerButton) -> Button {
    match button {
        PointerButton::Left => Button::Left,
        PointerButton::Right => Button::Right,
        PointerButton::Middle => Button::Middle,
    }
}

pub(crate) fn clamp_extent(value: i32) -> u16 {
    u16::try_from(value).unwrap_or(u16::MAX)
}

pub(crate) fn clamp_point(value: u16, extent: u16) -> i32 {
    if extent == 0 {
        i32::from(value)
    } else {
        i32::from(value.min(extent - 1))
    }
}

fn setup_error(error: NewConError) -> Error {
    match error {
        NewConError::NoPermission => {
            Error::InputFailed("the operating system has not granted input permission")
        }
        _ => Error::InputFailed("the operating system rejected input setup"),
    }
}

fn apply_error(error: enigo::InputError) -> Error {
    match error {
        enigo::InputError::InvalidInput(_) => {
            Error::InputFailed("this key is not mapped on this operating system")
        }
        _ => Error::InputFailed("the operating system rejected the event"),
    }
}
