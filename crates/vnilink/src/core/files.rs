//! File-transfer stream for one control session.

use super::*;

use std::sync::Arc;
use std::time::Duration;

use crate::error::Error;
use crate::events::CoreEvent;
use crate::session::SessionPhase;
use crate::transfer::{self, TransferRequest, TransferSender};

impl Core {
    /// Chooses the only directory this installation will write transferred files into.
    ///
    /// The directory is created when it is missing. A symlink is followed only
    /// for this path. Files inside it are not allowed to leave it, and a
    /// symlink there is not a destination. Refused while a controlling session
    /// is active. The transfer grant is separate and stays off until
    /// [`Core::authorize`] sets it.
    pub fn set_transfer_directory(&self, path: impl AsRef<std::path::Path>) -> Result<(), Error> {
        let state = self.alive_state()?;
        if state
            .incoming
            .as_ref()
            .is_some_and(|session| !finished(&session.phase))
        {
            return Err(Error::Busy);
        }
        drop(state);
        let canonical = transfer::prepare_directory(path.as_ref())?;
        *lock(&self.shared.transfer_dir) = Some(canonical);
        Ok(())
    }

    /// Sends one file to the controlled computer.
    ///
    /// Returns when the host has written it or refused it. The transfer grant
    /// is required. Control alone is not enough. A name that leaves the host's
    /// transfer directory, or a file larger than
    /// [`MAX_FILE_BYTES`](crate::MAX_FILE_BYTES), is rejected here. `replace`
    /// must be set to change an existing file. Nothing is logged.
    pub fn submit_file(
        &self,
        session_id: u64,
        generation: u64,
        name: &str,
        bytes: &[u8],
        replace: bool,
    ) -> Result<(), Error> {
        transfer::check_submission(name, bytes)?;
        let tx = {
            let state = self.alive_state()?;
            transfer_sender(&state, false, session_id, Some(generation))?
        };
        transfer_round_trip(tx, name, bytes, replace)
    }

    /// Sends one file from this host to the controller.
    ///
    /// The controller writes it under its own transfer directory. Nothing is
    /// sent when the transfer grant is off. `replace` must be set to change
    /// an existing file. Nothing is logged.
    pub fn publish_file(
        &self,
        session_id: u64,
        name: &str,
        bytes: &[u8],
        replace: bool,
    ) -> Result<(), Error> {
        transfer::check_submission(name, bytes)?;
        let tx = {
            let state = self.alive_state()?;
            let session = state.incoming.as_ref().ok_or(Error::NoSession)?;
            if session.phase != SessionPhase::Active || session.id != session_id {
                return Err(Error::NoSession);
            }
            if !state.trust.transfer_granted(&session.peer_key) {
                return Err(Error::TransferDenied);
            }
            session
                .transfer
                .clone()
                .ok_or(Error::Unsupported("transfer stream is not open"))?
        };
        transfer_round_trip(tx, name, bytes, replace)
    }
}

pub(super) async fn open_controller_transfer(
    shared: &Arc<Shared>,
    connection: &iroh::endpoint::Connection,
    session_id: u64,
    stop_rx: &tokio::sync::watch::Receiver<bool>,
) -> Result<tokio::task::JoinHandle<()>, Error> {
    let (tx, rx) = tokio::sync::mpsc::channel(1);
    let (opened_tx, opened_rx) = tokio::sync::oneshot::channel();
    let shared_inspect = Arc::clone(shared);
    let shared_write = Arc::clone(shared);
    let connection = iroh::endpoint::Connection::clone(connection);
    let stop_rx = tokio::sync::watch::Receiver::clone(stop_rx);
    let task = tokio::spawn(async move {
        transfer::attach(
            connection,
            true,
            rx,
            stop_rx,
            Some(opened_tx),
            move |name, size, _replace| {
                transfer::check_name(name)?;
                transfer::checked_size(size)?;
                if lock(&shared_inspect.transfer_dir).is_none() {
                    return Err(Error::Unsupported("no transfer directory is installed"));
                }
                Ok(())
            },
            move |name, bytes, replace| {
                let root = lock(&shared_write.transfer_dir)
                    .clone()
                    .ok_or(Error::Unsupported("no transfer directory is installed"))?;
                transfer::write_file(&root, name, bytes, replace)?;
                shared_write
                    .events
                    .publish(CoreEvent::TransferCompleted { id: session_id });
                Ok(())
            },
            || Ok(()),
        )
        .await;
    });
    match tokio::time::timeout(Duration::from_secs(10), opened_rx).await {
        Ok(Ok(Ok(()))) => {}
        Ok(Ok(Err(error))) => {
            task.abort();
            return Err(error);
        }
        _ => {
            task.abort();
            return Err(Error::network("the transfer stream did not open"));
        }
    }
    {
        let mut state = lock(&shared.state);
        if let Some(session) = state.outgoing.as_mut() {
            if session.id == session_id {
                session.transfer = Some(tx);
            }
        }
    }
    Ok(task)
}

pub(super) fn transfer_sender(
    state: &State,
    incoming: bool,
    session_id: u64,
    generation: Option<u64>,
) -> Result<TransferSender, Error> {
    let session = if incoming {
        state.incoming.as_ref()
    } else {
        state.outgoing.as_ref()
    }
    .ok_or(Error::NoSession)?;
    if session.phase != SessionPhase::Active || session.id != session_id {
        return Err(Error::NoSession);
    }
    if let Some(generation) = generation {
        if session.generation != generation {
            return Err(Error::StaleSession);
        }
    }
    session
        .transfer
        .clone()
        .ok_or(Error::Unsupported("transfer stream is not open"))
}

pub(super) fn transfer_round_trip(
    tx: TransferSender,
    name: &str,
    bytes: &[u8],
    replace: bool,
) -> Result<(), Error> {
    let (reply_tx, reply_rx) = std::sync::mpsc::sync_channel(1);
    match tx.try_send(TransferRequest {
        name: name.to_owned(),
        bytes: bytes.to_vec(),
        replace,
        reply: reply_tx,
    }) {
        Ok(()) => {}
        Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => return Err(Error::Busy),
        Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => {
            return Err(Error::Unsupported("transfer stream is closed"));
        }
    }
    match reply_rx.recv_timeout(Duration::from_secs(10)) {
        Ok(result) => result,
        Err(_) => Err(Error::Timeout),
    }
}

pub(super) fn apply_transfer(
    shared: &Shared,
    peer: &[u8; 32],
    name: &str,
    bytes: &[u8],
    replace: bool,
) -> Result<(), Error> {
    if !lock(&shared.state).trust.transfer_granted(peer) {
        return Err(Error::TransferDenied);
    }
    let root = lock(&shared.transfer_dir)
        .clone()
        .ok_or(Error::Unsupported("no transfer directory is installed"))?;
    transfer::write_file(&root, name, bytes, replace)
}
