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
//! it only when the installation's clipboard sharing is on and a
//! [`HostClipboard`] adapter is installed. This module does not read or write
//! the operating-system clipboard, and it does not log the text.
//!
//! Clipboard is a separate grant from control: pairing and the control
//! confirmation do not turn it on. The grant is `SharingPolicy::clipboard`,
//! changed with [`crate::Core::change_sharing_policy`]. The frames and their
//! limits live here; opening the pipe, the grant check and the delivery
//! order are described on the Desktop clipboard pipe in the core.
//!
//! Frames are a text or the host's status for one text. Text is UTF-8, at
//! most [`MAX_CLIPBOARD_TEXT`] bytes; empty text clears the clipboard. A
//! larger text is refused before anything is written. The status says
//! applied, denied or unavailable; denied maps back to
//! [`Error::ClipboardDenied`] and every other refusal arrives as unavailable. Debug output of a frame prints the
//! length, never the text.

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
