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
}

impl Error {
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
