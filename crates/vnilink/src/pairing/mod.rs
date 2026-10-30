// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! Pairing with a `.vnl` file.
//!
//! The signed invitation is the proof. The file is how two computers hand
//! that invitation over. Nothing in this folder opens a network connection,
//! and the product does not add a pairing server. After the file is confirmed,
//! Connect uses the relays this installation already chose, including the
//! public Iroh relays.
//!
//! # Steps
//!
//! 1. The computer that will be controlled calls [`crate::Core::export_pairing_file`].
//!    The app writes [`PairingFile::bytes`] to a `.vnl` file and shows
//!    [`PairingFile::check`]. `allow_control` is that computer's own switch.
//!    It is not written into the file.
//! 2. The person moves the file with a channel they already have.
//! 3. The other computer calls [`crate::Core::open_pairing_file`]. Trust is
//!    unchanged. [`crate::Core::confirm_pairing`] saves the writer when the
//!    typed digits match the writer's screen. That saves an address book
//!    entry. It does not grant control of either computer.
//! 4. The controller calls [`crate::Core::control_check`] and shows those
//!    digits, then calls [`crate::Core::connect`]. If the writer's switch was
//!    on and the file has not expired, the writer sees a control request with
//!    the same digits. [`crate::Core::confirm_control_request`] grants control.
//!    Clipboard and file transfer stay off.
//!
//! `core.rs` forwards here for the file bytes and the digits. The wait for
//! the host's answer lives next to the session handshake, because that is
//! where an incoming connection is accepted or rejected.
//!
//! The parts that are still open are listed in `crates/vnilink/PAIRING.md`.

mod check;
mod file;

pub use file::{PairingFile, PairingPreview};

pub(crate) use check::{control_digits, file_digits};
pub(crate) use file::{enclose, inspect};

/// File name suffix the app should use. The library does not write the file.
pub const PAIRING_FILE_EXTENSION: &str = "vnl";

pub(crate) fn require_same_check(
    preview: &PairingPreview,
    typed: &str,
) -> Result<(), crate::error::Error> {
    if typed == preview.check {
        Ok(())
    } else {
        Err(crate::error::Error::PairingCheckMismatch)
    }
}
