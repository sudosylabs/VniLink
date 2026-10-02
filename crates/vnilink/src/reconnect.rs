//! When the controller dials a saved device again.
//!
//! A drop is a connection that ends without a local disconnect and without
//! the host's close frame. The controller loads the peer's saved relays and
//! direct addresses, drops queued input, and dials again. Each success is a
//! new session generation. Input from the old generation is not written on
//! the new connection. A local disconnect, a host close, revoke, or shutdown
//! does not dial again.

use std::time::Duration;

use crate::error::Error;

/// Redials after one dropped session. A dead peer stops after these fail.
pub(crate) const ATTEMPTS: u32 = 3;

/// Pause before the next dial, so the host can finish releasing held keys.
pub(crate) const PAUSE: Duration = Duration::from_millis(200);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LinkEnd {
    /// This computer called disconnect or shut down.
    LocalStop,
    /// The host wrote a close frame. The session was ended on purpose.
    RemoteClose,
    /// The connection ended with no close frame.
    Dropped,
}

/// Failures that can clear before the next dial. A refusal does not.
pub(crate) fn retries(error: &Error) -> bool {
    matches!(error, Error::Busy | Error::Timeout | Error::Network(_))
}
