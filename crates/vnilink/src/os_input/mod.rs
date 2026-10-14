use enigo::{Axis, Coordinate, Direction, Enigo, Keyboard, Mouse, NewConError, Settings};

use crate::error::Error;
use crate::input::{HostDesktop, InputEvent};

/// Linux evdev keycode for a USB HID keyboard usage. Portals take evdev
/// codes, unlike X11's evdev-plus-eight keycodes. Does not inject input.
pub fn linux_evdev_keycode(usage: u16) -> Option<i32> {
    key_for_usage(usage)
        .and_then(|code| code.linux.checked_sub(8))
        .map(i32::from)
}

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
    #[cfg(all(unix, not(target_os = "macos")))]
    wheel_remainder: (i32, i32),
    /// X11 connection used to read the keymap for Translate mode. Opened on
    /// the first chord, not by a unit test.
    #[cfg(all(unix, not(target_os = "macos")))]
    display: Option<x11rb::rust_connection::RustConnection>,
}

impl OsInput {
    pub fn open() -> Result<Self, Error> {
        Self::open_with_prompt(true)
    }
    pub(crate) fn open_with_prompt(prompt: bool) -> Result<Self, Error> {
        let settings = Settings {
            open_prompt_to_get_permissions: prompt,
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
            #[cfg(all(unix, not(target_os = "macos")))]
            wheel_remainder: (0, 0),
            #[cfg(all(unix, not(target_os = "macos")))]
            display: None,
        })
    }
}

impl OsInput {
    /// Applies one event on this thread. The [`HostDesktop`] wrapper routes
    /// through the input owner while input is blocked; the owner's own
    /// backend calls here so validation runs exactly once per event.
    pub(crate) fn apply_direct(&mut self, event: InputEvent) -> Result<(), Error> {
        match event {
            InputEvent::Key { usage, down } => {
                let codes = key_for_usage(usage).ok_or(Error::InputFailed(
                    "this key is not mapped on this operating system",
                ))?;
                let direction = if down {
                    Direction::Press
                } else {
                    Direction::Release
                };
                self.enigo
                    .raw(platform_code(codes), direction)
                    .map_err(apply_error)
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
            InputEvent::Pointer { x, y, .. } => {
                let bound = u32::try_from(crate::input::MAX_DESKTOP_ORIGIN).unwrap_or(u32::MAX);
                if x.unsigned_abs() > bound || y.unsigned_abs() > bound {
                    return Err(Error::InputFailed("the pointer is outside the desktop"));
                }
                self.enigo
                    .move_mouse(x, y, Coordinate::Abs)
                    .map_err(apply_error)
            }
            InputEvent::Motion { dx, dy } => self
                .enigo
                .move_mouse(i32::from(dx), i32::from(dy), Coordinate::Rel)
                .map_err(apply_error),
            InputEvent::PreciseWheel { dx, dy } => {
                if dx.unsigned_abs() > crate::input::MAX_PRECISE_WHEEL as u32
                    || dy.unsigned_abs() > crate::input::MAX_PRECISE_WHEEL as u32
                {
                    return Err(Error::InputFailed(
                        "scroll delta is outside the supported range",
                    ));
                }
                #[cfg(any(target_os = "macos", target_os = "windows"))]
                {
                    if dx != 0 {
                        self.enigo
                            .scroll_precise(dx, Axis::Horizontal)
                            .map_err(apply_error)?;
                    }
                    if dy != 0 {
                        self.enigo
                            .scroll_precise(dy, Axis::Vertical)
                            .map_err(apply_error)?;
                    }
                    Ok(())
                }
                #[cfg(all(unix, not(target_os = "macos")))]
                {
                    // XTest offers detents, not fractional axes. Retain the
                    // signed remainder rather than rounding each small sample.
                    let dx = crate::input::accumulate_detents(&mut self.wheel_remainder.0, dx);
                    let dy = crate::input::accumulate_detents(&mut self.wheel_remainder.1, dy);
                    self.apply(InputEvent::wheel(dx as i16, dy as i16))
                }
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

    pub(crate) fn type_text_direct(&mut self, text: &str) -> Result<(), Error> {
        if let Some(message) = crate::input::key_text_problem(text) {
            return Err(Error::InputFailed(message));
        }
        self.enigo.text(text).map_err(apply_error)
    }

    pub(crate) fn type_chord_direct(&mut self, text: &str) -> Result<(), Error> {
        if let Some(message) = crate::input::key_text_problem(text) {
            return Err(Error::InputFailed(message));
        }
        #[cfg(all(unix, not(target_os = "macos")))]
        {
            return self.type_layout_chord(text);
        }
        #[cfg(not(all(unix, not(target_os = "macos"))))]
        {
            let _ = text;
            Err(Error::InputFailed("translated keys are not available"))
        }
    }
}

impl HostDesktop for OsInput {
    fn apply(&mut self, event: InputEvent) -> Result<(), Error> {
        // While input is blocked, only the owner thread may inject.
        #[cfg(target_os = "windows")]
        if crate::system::owner::is_blocked() {
            return crate::system::owner::inject(crate::system::owner::InjectOp::Apply(event));
        }
        self.apply_direct(event)
    }

    fn type_text(&mut self, text: &str) -> Result<(), Error> {
        #[cfg(target_os = "windows")]
        if crate::system::owner::is_blocked() {
            return crate::system::owner::inject(crate::system::owner::InjectOp::TypeText(
                text.to_owned(),
            ));
        }
        self.type_text_direct(text)
    }

    fn type_chord(&mut self, text: &str) -> Result<(), Error> {
        #[cfg(target_os = "windows")]
        if crate::system::owner::is_blocked() {
            return crate::system::owner::inject(crate::system::owner::InjectOp::TypeChord(
                text.to_owned(),
            ));
        }
        self.type_chord_direct(text)
    }

    fn monitor(&self) -> (u16, u16) {
        (self.width, self.height)
    }
}

#[cfg(all(unix, not(target_os = "macos")))]
mod chord;
mod codes;

use codes::enigo_button;

#[cfg(test)]
pub(crate) use codes::KeyCodes;
pub(crate) use codes::{clamp_extent, clamp_point, key_for_usage, platform_code};

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
