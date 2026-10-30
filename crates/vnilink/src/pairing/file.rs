// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! Bytes of a `.vnl` pairing file.
//!
//! ```text
//! magic      b"VNLF"
//! version    u8 = 1
//! token_len  u32 big-endian
//! token      UTF-8 signed invitation
//! ```
//!
//! The app writes [`PairingFile::bytes`] to a file whose name ends in `.vnl`.
//! This module does not choose a path and does not open the disk. The
//! invitation inside the file is the proof. The file is how the two people
//! move it, with AirDrop, a message, email, or a USB stick.

use std::time::SystemTime;

use iroh::PublicKey;

use crate::error::Error;
use crate::invitation::{self, MAX_INVITATION_CHARS};

use super::check;

const MAGIC: &[u8; 4] = b"VNLF";
const VERSION: u8 = 1;
const HEADER_LEN: usize = 4 + 1 + 4;

/// What the writer of a `.vnl` file shows. `bytes` go into the file.
///
/// [`Debug`] leaves the bytes out. They contain the signed invitation.
#[derive(Clone, PartialEq, Eq)]
pub struct PairingFile {
    pub bytes: Vec<u8>,
    /// Three digits to show next to the file.
    pub check: String,
    /// Unix milliseconds when the invitation inside the file expires.
    pub expires_at_ms: u64,
}

impl std::fmt::Debug for PairingFile {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PairingFile")
            .field("check", &self.check)
            .field("expires_at_ms", &self.expires_at_ms)
            .field("bytes", &"[redacted]")
            .finish()
    }
}

/// A file that has been read and checked, and not saved yet.
///
/// `token` is the signed invitation. Confirm accepts it. External code cannot
/// build a preview; it comes from [`crate::Core::open_pairing_file`].
#[derive(Clone, PartialEq, Eq)]
pub struct PairingPreview {
    pub endpoint_id: String,
    /// Three digits. They must match the screen that wrote the file.
    pub check: String,
    pub expires_at_ms: u64,
    pub(crate) token: String,
}

impl std::fmt::Debug for PairingPreview {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PairingPreview")
            .field("endpoint_id", &self.endpoint_id)
            .field("check", &self.check)
            .field("expires_at_ms", &self.expires_at_ms)
            .field("token", &"[redacted]")
            .finish()
    }
}

pub(crate) fn enclose(token: &str) -> Result<Vec<u8>, Error> {
    if token.is_empty() || token.len() > MAX_INVITATION_CHARS {
        return Err(Error::InvalidPairingFile);
    }
    let mut bytes = Vec::with_capacity(HEADER_LEN + token.len());
    bytes.extend_from_slice(MAGIC);
    bytes.push(VERSION);
    let token_len = u32::try_from(token.len()).map_err(|_| Error::InvalidPairingFile)?;
    bytes.extend_from_slice(&token_len.to_be_bytes());
    bytes.extend_from_slice(token.as_bytes());
    Ok(bytes)
}

pub(crate) fn inspect(
    bytes: &[u8],
    now: SystemTime,
    self_id: [u8; 32],
) -> Result<PairingPreview, Error> {
    let token = token_from_file(bytes)?;
    let (opened, nonce) = invitation::open(&token, now)?;
    if opened.endpoint_id == self_id {
        return Err(Error::InvalidInvitation(
            "an installation cannot pair with itself",
        ));
    }
    let endpoint_id = PublicKey::from_bytes(&opened.endpoint_id)
        .map(|key| key.to_string())
        .map_err(|_| Error::InvalidInvitation("endpoint id is not a valid key"))?;
    Ok(PairingPreview {
        endpoint_id,
        check: check::file_digits(&opened.endpoint_id, &nonce, opened.expires_at_ms),
        expires_at_ms: opened.expires_at_ms,
        token,
    })
}

fn token_from_file(bytes: &[u8]) -> Result<String, Error> {
    if bytes.len() < HEADER_LEN || &bytes[..4] != MAGIC || bytes[4] != VERSION {
        return Err(Error::InvalidPairingFile);
    }
    let token_len = usize::try_from(u32::from_be_bytes(read_array(bytes)?))
        .map_err(|_| Error::InvalidPairingFile)?;
    if token_len == 0 || token_len > MAX_INVITATION_CHARS {
        return Err(Error::InvalidPairingFile);
    }
    let end = HEADER_LEN
        .checked_add(token_len)
        .ok_or(Error::InvalidPairingFile)?;
    if end != bytes.len() {
        return Err(Error::InvalidPairingFile);
    }
    std::str::from_utf8(&bytes[HEADER_LEN..end])
        .map(|token| token.to_owned())
        .map_err(|_| Error::InvalidPairingFile)
}

fn read_array(bytes: &[u8]) -> Result<[u8; 4], Error> {
    let mut array = [0u8; 4];
    array.copy_from_slice(&bytes[5..9]);
    Ok(array)
}
