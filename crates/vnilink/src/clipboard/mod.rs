// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! Clipboard text a Desktop carries, as frames on its own pipe.
//!
//! The text rides its own stream so a paste does not wait behind keyboard
//! input, and keyboard input does not wait behind a paste. The host applies
//! it only when that controller's clipboard grant is on. This module does not
//! read or write the operating-system clipboard, and it does not log the text.

use crate::error::Error;

/// Largest clipboard paste, in UTF-8 bytes. Empty text clears the clipboard.
pub const MAX_CLIPBOARD_TEXT: usize = 256 * 1024;

mod frame;

pub(crate) use frame::{check_len, check_text, decode, encode, Frame};
pub(crate) use frame::{status_code, status_result};

/// Applies text on the controlled computer.
///
/// The platform owns the operating-system clipboard. This method must not
/// call back into [`crate::Core`]. The text is private: do not log it.
pub trait HostClipboard: Send {
    fn set_text(&mut self, text: &str) -> Result<(), Error>;
}
