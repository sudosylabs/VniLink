// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! Translate mode on Linux. The X display opens on the first chord, not in tests.
// trace: records every ignored failure

use enigo::{Direction, Keyboard};

use x11rb::connection::Connection;
use x11rb::protocol::xproto::ConnectionExt;

use super::apply_error;
use super::codes::{key_for_usage, platform_code};
use crate::error::Error;
use crate::input::usage;
use crate::keyboard::{self, ChordLevel};

#[cfg(all(unix, not(target_os = "macos")))]
impl super::OsInput {
    pub(super) fn type_layout_chord(&mut self, text: &str) -> Result<(), Error> {
        for character in text.chars() {
            let keysym = keyboard::keysym_of(character);
            let map = self.host_keymap()?;
            let Some(chord) = keyboard::find_chord(&map, keysym) else {
                return Err(Error::InputFailed(
                    "this character is not on the host keyboard",
                ));
            };
            self.hold_level(chord.level)?;
            let typed = self
                .enigo
                .raw(u16::from(chord.keycode), Direction::Click)
                .map_err(apply_error);
            let released = self.drop_level(chord.level);
            typed.and(released)?;
        }
        Ok(())
    }

    fn host_keymap(&mut self) -> Result<keyboard::HostKeymap, Error> {
        if self.display.is_none() {
            match x11rb::connect(None) {
                Ok((connection, _screen)) => self.display = Some(connection),
                Err(_) => return Err(Error::InputFailed("translated keys are not available")),
            }
        }
        let Some(connection) = self.display.as_ref() else {
            return Err(Error::InputFailed("translated keys are not available"));
        };
        match read_keymap(connection) {
            Ok(map) => Ok(map),
            Err(error) => {
                self.display = None;
                Err(error)
            }
        }
    }

    fn hold_level(&mut self, level: ChordLevel) -> Result<(), Error> {
        match level {
            ChordLevel::Plain => Ok(()),
            ChordLevel::Shift => self.raw_usage(usage::LEFT_SHIFT, Direction::Press),
            ChordLevel::AltGr => self.raw_usage(usage::RIGHT_ALT, Direction::Press),
            ChordLevel::ShiftAltGr => {
                self.raw_usage(usage::LEFT_SHIFT, Direction::Press)?;
                if let Err(error) = self.raw_usage(usage::RIGHT_ALT, Direction::Press) {
                    crate::ignored!(
                        crate::trace::INPUT,
                        "the shift held for a failed shortcut is released at best",
                        self.raw_usage(usage::LEFT_SHIFT, Direction::Release)
                    );
                    return Err(error);
                }
                Ok(())
            }
        }
    }

    fn drop_level(&mut self, level: ChordLevel) -> Result<(), Error> {
        match level {
            ChordLevel::Plain => Ok(()),
            ChordLevel::Shift => self.raw_usage(usage::LEFT_SHIFT, Direction::Release),
            ChordLevel::AltGr => self.raw_usage(usage::RIGHT_ALT, Direction::Release),
            ChordLevel::ShiftAltGr => {
                let alt = self.raw_usage(usage::RIGHT_ALT, Direction::Release);
                let shift = self.raw_usage(usage::LEFT_SHIFT, Direction::Release);
                alt.and(shift)
            }
        }
    }

    fn raw_usage(&mut self, usage_code: u16, direction: Direction) -> Result<(), Error> {
        let codes = key_for_usage(usage_code).ok_or(Error::InputFailed(
            "this key is not mapped on this operating system",
        ))?;
        self.enigo
            .raw(platform_code(codes), direction)
            .map_err(apply_error)
    }
}

#[cfg(all(unix, not(target_os = "macos")))]
fn read_keymap(
    connection: &x11rb::rust_connection::RustConnection,
) -> Result<keyboard::HostKeymap, Error> {
    let setup = connection.setup();
    let min = setup.min_keycode;
    let max = setup.max_keycode;
    let count = max.saturating_sub(min).saturating_add(1);
    let reply = connection
        .get_keyboard_mapping(min, count)
        .map_err(|_| Error::InputFailed("translated keys are not available"))?
        .reply()
        .map_err(|_| Error::InputFailed("translated keys are not available"))?;
    Ok(keyboard::HostKeymap {
        min_keycode: min,
        keysyms_per_keycode: reply.keysyms_per_keycode,
        keysyms: reply.keysyms,
    })
}
