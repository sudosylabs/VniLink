// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! When the controller dials a saved device again.
//!
//! A dropped link comes back for the same paired device without a new
//! pairing file. The dial loop is `run_controller` in the connect module;
//! this module decides which endings redial and how often.
//!
//! # What redials
//!
//! A drop is a connection that ends without a local disconnect, without the
//! host's close frame and without the host's shutdown close ([`LinkEnd`]).
//! The controller moves to [`crate::SessionPhase::Reconnecting`], waits [`PAUSE`],
//! loads the peer's stored relays and direct addresses (see
//! [`crate::address`]) and dials again. A local disconnect, a host close
//! frame, a host shutdown and unpairing do not redial; after
//! [`crate::Core::unpair`] the next dial fails with [`crate::Error::NotPaired`].
//!
//! # How often
//!
//! Each drop counts against [`ATTEMPTS`]. A link that stayed up longer than
//! [`PAUSE`] restarts the count, so a computer that drops once an hour never
//! runs out. Before the host has admitted the controller once, every failure
//! is final: only an admitted session retries, and only for the errors
//! [`retries`] names. When the attempts run out the session fails; the
//! presence supervisor then starts a new session with its own backoff.
//!
//! # What a redial keeps and what it fences
//!
//! Each success is a new session generation. A controller service owner
//! keeps its id and moves to [`crate::ServicePhase::Suspended`] while the link is
//! down, then resumes under the new generation; a service that was already
//! closed stays closed. Everything bound to the old generation is fenced:
//! handles, leases and pipes opened on it are cancelled and refused with
//! [`crate::Error::StaleSession`]. Queued input is dropped, and a clipboard paste
//! or text from the old Desktop is not sent on the new connection. The host
//! ends the services of the lost connection as [`crate::CloseCause::ParentLoss`],
//! releasing held keys, and a newer Hello from the same peer signals the
//! older session to stop before the new one is registered.

use std::time::Duration;

use iroh::endpoint::{ApplicationClose, Connection, ConnectionError};

use crate::error::Error;
use crate::protocol::CLOSE_SHUT_DOWN;

/// Redials after one dropped session. A dead peer stops after these fail.
pub(crate) const ATTEMPTS: u32 = 3;

/// Pause before the next dial, so the host can finish releasing held keys.
pub(crate) const PAUSE: Duration = Duration::from_millis(200);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LinkEnd {
    /// This computer called disconnect or shut down.
    LocalStop,
    /// The host wrote a close frame or shut down. The session was ended on
    /// purpose.
    RemoteClose,
    /// The connection ended with no close frame.
    Dropped,
}

/// Failures that can clear before the next dial. A refusal does not.
pub(crate) fn retries(error: &Error) -> bool {
    matches!(error, Error::Busy | Error::Timeout | Error::Network(_))
}

/// Whether the peer closed `connection` because its engine shut down
/// ([`CLOSE_SHUT_DOWN`]).
pub(crate) fn peer_shut_down(connection: &Connection) -> bool {
    matches!(
        connection.close_reason(),
        Some(ConnectionError::ApplicationClosed(ApplicationClose { error_code, .. }))
            if error_code == CLOSE_SHUT_DOWN.into()
    )
}
