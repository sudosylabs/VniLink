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
    #[error("the file already exists")]
    FileExists,
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
            Self::FileExists => "file exists",
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
            _ => None,
        }
    }

    pub(crate) fn network(err: impl fmt::Display) -> Self {
        Self::network_text(err.to_string())
    }

    pub(crate) fn network_debug(err: impl fmt::Debug) -> Self {
        Self::network_text(format!("{err:?}"))
    }

    fn network_text(mut text: String) -> Self {
        const MAX: usize = 256;
        if text.len() > MAX {
            text.truncate(MAX);
        }
        Self::Network(text)
    }
}
