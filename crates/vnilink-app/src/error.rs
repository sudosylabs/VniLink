//! Flat FFI error mapping. Every variant keeps its distinguishable reason;
//! messages name the reason only.

use vnilink::Error;

/// Errors an application adapter caller can match on. Carried strings name the reason;
/// they never contain secrets, pixels, or bitstreams.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum FfiError {
    #[error("the core has shut down")]
    ShutDown,
    #[error("invalid relay: {message}")]
    InvalidRelay { message: String },
    #[error("invalid setting: {message}")]
    InvalidSetting { message: String },
    #[error("invalid relay URL: {message}")]
    InvalidRelayUrl { message: String },
    #[error("invitation is invalid: {message}")]
    InvalidInvitation { message: String },
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
    #[error("not available: {message}")]
    Unsupported { message: String },
    #[error("input was not applied: {message}")]
    InputFailed { message: String },
    #[error("the peer speaks a different protocol version")]
    IncompatibleVersion,
    #[error("the control message is malformed")]
    Malformed,
    #[error("timed out waiting for the peer")]
    Timeout,
    #[error("network error: {message}")]
    Network { message: String },
    #[error("too many paired devices")]
    TooManyDevices,
    #[error("the credential store is unavailable")]
    StoreUnavailable,
    #[error("the credential store is locked")]
    StoreLocked,
    #[error("the installation record is not valid")]
    CorruptRecord,
    #[error("the pairing file is not valid")]
    InvalidPairingFile,
    #[error("the pairing check does not match")]
    PairingCheckMismatch,
    #[error("there is no control request waiting")]
    NoControlRequest,
}

pub(crate) fn map_error(error: Error) -> FfiError {
    match error {
        Error::ShutDown => FfiError::ShutDown,
        Error::InvalidRelay(message) => FfiError::InvalidRelay {
            message: message.to_string(),
        },
        Error::InvalidSetting(message) => FfiError::InvalidSetting {
            message: message.to_string(),
        },
        Error::InvalidRelayUrl { reason, .. } => FfiError::InvalidRelayUrl { message: reason },
        Error::InvalidInvitation(message) => FfiError::InvalidInvitation {
            message: message.to_string(),
        },
        Error::InvitationExpired => FfiError::InvitationExpired,
        Error::InvitationReplayed => FfiError::InvitationReplayed,
        Error::InvitationCacheFull => FfiError::InvitationCacheFull,
        Error::NotPaired => FfiError::NotPaired,
        Error::NotHosting => FfiError::NotHosting,
        Error::Busy => FfiError::Busy,
        Error::Unauthorized => FfiError::Unauthorized,
        Error::ClipboardDenied => FfiError::ClipboardDenied,
        Error::TransferDenied => FfiError::TransferDenied,
        Error::FileExists => FfiError::FileExists,
        Error::NoSession => FfiError::NoSession,
        Error::StaleSession => FfiError::StaleSession,
        Error::Unsupported(message) => FfiError::Unsupported {
            message: message.to_string(),
        },
        Error::InputFailed(message) => FfiError::InputFailed {
            message: message.to_string(),
        },
        Error::IncompatibleVersion => FfiError::IncompatibleVersion,
        Error::Malformed => FfiError::Malformed,
        Error::Timeout => FfiError::Timeout,
        Error::Network(message) => FfiError::Network { message },
        Error::TooManyDevices(_) => FfiError::TooManyDevices,
        Error::StoreUnavailable => FfiError::StoreUnavailable,
        Error::StoreLocked => FfiError::StoreLocked,
        Error::CorruptRecord => FfiError::CorruptRecord,
        Error::InvalidPairingFile => FfiError::InvalidPairingFile,
        Error::PairingCheckMismatch => FfiError::PairingCheckMismatch,
        Error::NoControlRequest => FfiError::NoControlRequest,
    }
}
