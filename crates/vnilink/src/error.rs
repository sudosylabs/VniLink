// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

use std::fmt;

/// Failures a native host can show without inspecting Iroh types.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("the core has shut down")]
    ShutDown,
    #[error("{0}")]
    InvalidRelay(&'static str),
    #[error("{0}")]
    InvalidSetting(&'static str),
    #[error("relay URL {index} is invalid: {reason}")]
    InvalidRelayUrl { index: usize, reason: String },
    #[error("invitation is invalid: {0}")]
    InvalidInvitation(&'static str),
    #[error("invitation has expired")]
    InvitationExpired,
    #[error("invitation was already used")]
    InvitationReplayed,
    #[error("too many outstanding invitations")]
    InvitationCacheFull,
    #[error("device is not paired")]
    NotPaired,
    #[error("hosting is not enabled")]
    NotHosting,
    #[error("the host already has a controlling session")]
    Busy,
    #[error("the device is not authorized to control this host")]
    Unauthorized,
    #[error("clipboard is not granted")]
    ClipboardDenied,
    #[error("file transfer is not granted")]
    TransferDenied,
    /// The disk refused what a transfer stages or writes.
    #[error("{}", .0.message())]
    Disk(DiskFault),
    #[error("the file already exists")]
    FileExists,
    /// A change on a drive whose file ids are entry positions (FAT,
    /// exFAT) that the drive cannot prove acts on the listed entry: deleting
    /// or trashing a folder, or any change made without the fingerprint its
    /// listing gave.
    #[error("the drive cannot prove the entry is the one listed")]
    UnprovenEntry,
    #[error("there is no active session")]
    NoSession,
    #[error("the input belongs to an old session")]
    StaleSession,
    #[error("{0} is not available yet")]
    Unsupported(&'static str),
    #[error("input was not applied: {0}")]
    InputFailed(&'static str),
    #[error("the peer speaks a different protocol version")]
    IncompatibleVersion,
    #[error("the control message is malformed")]
    Malformed,
    #[error("timed out waiting for the peer")]
    Timeout,
    #[error("network error: {0}")]
    Network(String),
    #[error("at most {0} paired devices are supported")]
    TooManyDevices(usize),
    /// The installation store could not complete an operation. Nothing was
    /// committed, or the previous record is still the committed record.
    /// Callers roll the in-memory change back and may continue.
    #[error("the installation store is unavailable")]
    StoreUnavailable,
    /// The platform credential store refused access. This is not a SQLite or
    /// filesystem failure.
    #[error("the credential store is locked")]
    StoreLocked,
    #[error("the installation record is not valid")]
    CorruptRecord,
    /// A save may have committed and its durable outcome is not known.
    /// The core stops remote work. Reopen the installation after restarting
    /// VniLink; do not retry the failed operation in this process.
    #[error("the installation store must be reopened and VniLink restarted")]
    StoreRecoveryRequired,
    #[error("the pairing file is not valid")]
    InvalidPairingFile,
    #[error("the pairing check does not match")]
    PairingCheckMismatch,
    #[error("there is no control request waiting")]
    NoControlRequest,
    #[error("the other computer's owner declined")]
    Declined,
    #[error("the other computer's owner has not accepted this computer")]
    NotAdmitted,
    #[error("nobody at the other computer answered in time")]
    Unanswered,
    #[error("there is no admission request waiting")]
    NoAdmissionRequest,
    /// The file and the volume's free reserve need more space than is free.
    #[error("this file needs {needed} bytes free and {free} bytes are free")]
    InsufficientStorage { needed: u64, free: u64 },
}

/// Why a disk write failed, as far as a transfer tells the user.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiskFault {
    /// The destination's disk is full, or its quota is used up.
    NoSpace,
    /// Writing failed for a reason other than space.
    Failed,
}

impl DiskFault {
    /// The fixed text a job and the window show.
    pub fn message(self) -> &'static str {
        match self {
            Self::NoSpace => "there is not enough space for this file",
            Self::Failed => "the file could not be staged",
        }
    }
}

impl Error {
    /// What failed, by name, for records (`crate::trace`): unlike its text,
    /// never a library's message, an address or a path.
    pub fn name(&self) -> &'static str {
        match self {
            Self::ShutDown => "shut down",
            Self::InvalidRelay(_) | Self::InvalidRelayUrl { .. } => "invalid relay",
            Self::InvalidSetting(_) => "invalid setting",
            Self::InvalidInvitation(_) => "invalid invitation",
            Self::InvitationExpired => "invitation expired",
            Self::InvitationReplayed => "invitation replayed",
            Self::InvitationCacheFull => "too many invitations",
            Self::NotPaired => "not paired",
            Self::NotHosting => "not hosting",
            Self::Busy => "busy",
            Self::Unauthorized => "unauthorized",
            Self::ClipboardDenied => "clipboard denied",
            Self::TransferDenied => "transfer denied",
            Self::Disk(_) => "disk",
            Self::FileExists => "file exists",
            Self::UnprovenEntry => "unproven entry",
            Self::NoSession => "no session",
            Self::StaleSession => "stale session",
            Self::Unsupported(_) => "unsupported",
            Self::InputFailed(_) => "input failed",
            Self::IncompatibleVersion => "incompatible version",
            Self::Malformed => "malformed",
            Self::Timeout => "timeout",
            Self::Network(_) => "network",
            Self::TooManyDevices(_) => "too many devices",
            Self::StoreUnavailable => "store unavailable",
            Self::StoreLocked => "store locked",
            Self::CorruptRecord => "corrupt record",
            Self::StoreRecoveryRequired => "store recovery required",
            Self::InvalidPairingFile => "invalid pairing file",
            Self::PairingCheckMismatch => "pairing check mismatch",
            Self::NoControlRequest => "no control request",
            Self::Declined => "declined",
            Self::NotAdmitted => "not admitted",
            Self::Unanswered => "unanswered",
            Self::NoAdmissionRequest => "no admission request",
            Self::InsufficientStorage { .. } => "insufficient storage",
        }
    }

    /// The fixed reason some errors carry, such as which setting or backend:
    /// product text, safe to record.
    pub fn reason(&self) -> Option<&'static str> {
        match self {
            Self::InvalidRelay(reason)
            | Self::InvalidSetting(reason)
            | Self::InvalidInvitation(reason)
            | Self::Unsupported(reason)
            | Self::InputFailed(reason) => Some(reason),
            Self::Disk(fault) => Some(fault.message()),
            _ => None,
        }
    }

    pub(crate) fn network(err: impl fmt::Display) -> Self {
        Self::network_text(err.to_string())
    }

    pub(crate) fn network_debug(err: impl fmt::Debug) -> Self {
        Self::network_text(format!("{err:?}"))
    }

    /// At most 256 bytes of `text`, cut before a character the limit would
    /// split: OS messages are localized and need not be ASCII.
    fn network_text(mut text: String) -> Self {
        const MAX: usize = 256;
        text.truncate(text.floor_char_boundary(MAX));
        Self::Network(text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn network_messages_are_truncated_to_256_bytes() {
        match Error::network("abc") {
            Error::Network(text) => assert_eq!(text, "abc"),
            other => panic!("short network error was {other:?}"),
        }
        match Error::network("x".repeat(256)) {
            Error::Network(text) => assert_eq!(text.len(), 256),
            other => panic!("exact network error was {other:?}"),
        }
        match Error::network("y".repeat(300)) {
            Error::Network(text) => {
                assert_eq!(text.len(), 256);
                assert!(text.chars().all(|character| character == 'y'));
            }
            other => panic!("long network error was {other:?}"),
        }
        match Error::network_debug("z".repeat(400)) {
            Error::Network(text) => assert_eq!(text.len(), 256),
            other => panic!("debug network error was {other:?}"),
        }
    }

    #[test]
    fn a_network_message_is_cut_before_a_character_the_limit_splits() {
        // A localized OS message: "é" is two bytes, starting at byte 255.
        let message = format!("{}é and more", "x".repeat(255));
        match Error::network(message) {
            Error::Network(text) => assert_eq!(text, "x".repeat(255)),
            other => panic!("long localized network error was {other:?}"),
        }
    }
}
