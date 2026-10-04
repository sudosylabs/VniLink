//! Clipboard stream for one control session.

use super::*;

use std::sync::Arc;

use crate::clipboard::{ClipboardRequest, ClipboardSender, HostClipboard};
use crate::error::Error;
use crate::events::CoreEvent;
use crate::session::Grant;

impl Core {
    /// Installs the adapter that receives clipboard text on this host.
    ///
    /// The library does not read or write the operating-system clipboard.
    /// Refused while a controlling session is active.
    pub fn set_clipboard(&self, clipboard: Box<dyn HostClipboard>) -> Result<(), Error> {
        let state = self.alive_state()?;
        if state
            .incoming
            .as_ref()
            .is_some_and(|session| !finished(&session.phase))
        {
            return Err(Error::Busy);
        }
        drop(state);
        *lock(&self.shared.clipboard) = Some(clipboard);
        Ok(())
    }

    /// Sends clipboard text to the controlled computer.
    ///
    /// Returns when the host has applied it or refused it. The clipboard grant
    /// is required. Control alone is not enough. Text longer than
    /// [`MAX_CLIPBOARD_TEXT`](crate::MAX_CLIPBOARD_TEXT) is rejected here.
    /// Empty text clears the host clipboard. This does not read the local
    /// operating-system clipboard.
    pub fn submit_clipboard(
        &self,
        session_id: u64,
        generation: u64,
        text: &str,
    ) -> Result<(), Error> {
        crate::clipboard::check_text(text)?;
        let tx = {
            let state = self.alive_state()?;
            clipboard_sender(&state, false, session_id, Some(generation))?
        };
        clipboard_round_trip(tx, text)
    }

    /// Sends this host's clipboard text to the controller.
    ///
    /// The controller reads it with [`Core::clipboard_text`]. Nothing is sent
    /// when the clipboard grant is off. Empty text clears the controller's
    /// copy. This does not read the operating-system clipboard.
    pub fn publish_clipboard(&self, session_id: u64, text: &str) -> Result<(), Error> {
        crate::clipboard::check_text(text)?;
        let tx = {
            let state = self.alive_state()?;
            let session = live_session(&state, true, session_id)?;
            if !state.trust.granted(&session.peer_key, Grant::Clipboard) {
                return Err(Error::ClipboardDenied);
            }
            session
                .clipboard
                .clone()
                .ok_or(Error::Unsupported("clipboard stream is not open"))?
        };
        clipboard_round_trip(tx, text)
    }

    /// Latest clipboard text published by the controlled computer.
    ///
    /// `None` before the first paste of this session. The text is replaced,
    /// not queued. Do not log the returned string.
    pub fn clipboard_text(&self) -> Result<Option<String>, Error> {
        self.ensure_alive()?;
        Ok(lock(&self.shared.clipboard_inbox).clone())
    }
}

pub(super) async fn open_controller_clipboard(
    shared: &Arc<Shared>,
    connection: &iroh::endpoint::Connection,
    session_id: u64,
    stop_rx: &tokio::sync::watch::Receiver<bool>,
) -> Result<tokio::task::JoinHandle<()>, Error> {
    let (tx, rx) = tokio::sync::mpsc::channel(1);
    let (opened_tx, opened_rx) = tokio::sync::oneshot::channel();
    let shared_text = Arc::clone(shared);
    let connection = iroh::endpoint::Connection::clone(connection);
    let stop_rx = tokio::sync::watch::Receiver::clone(stop_rx);
    let task = tokio::spawn(async move {
        crate::clipboard::attach(
            connection,
            true,
            rx,
            stop_rx,
            Some(opened_tx),
            move |text| {
                *lock(&shared_text.clipboard_inbox) = Some(text.to_owned());
                shared_text
                    .events
                    .publish(CoreEvent::ClipboardReceived { id: session_id });
                Ok(())
            },
            || Ok(()),
            true,
        )
        .await;
    });
    let task =
        match crate::sidestream::await_opened(task, opened_rx, "the clipboard stream did not open")
            .await
        {
            Ok(task) => task,
            Err(error) => return Err(error),
        };
    {
        let mut state = lock(&shared.state);
        if let Some(session) = state.outgoing.as_mut() {
            if session.id == session_id {
                session.clipboard = Some(tx);
            }
        }
    }
    *lock(&shared.clipboard_inbox) = None;
    Ok(task)
}

pub(super) fn clipboard_sender(
    state: &State,
    incoming: bool,
    session_id: u64,
    generation: Option<u64>,
) -> Result<ClipboardSender, Error> {
    let session = live_session(state, incoming, session_id)?;
    if let Some(generation) = generation {
        if session.generation != generation {
            return Err(Error::StaleSession);
        }
    }
    session
        .clipboard
        .clone()
        .ok_or(Error::Unsupported("clipboard stream is not open"))
}

pub(super) fn clipboard_round_trip(tx: ClipboardSender, text: &str) -> Result<(), Error> {
    round_trip(&tx, "clipboard stream is closed", |reply| {
        ClipboardRequest {
            text: text.to_owned(),
            reply,
        }
    })
}

pub(super) fn apply_clipboard(shared: &Shared, peer: &[u8; 32], text: &str) -> Result<(), Error> {
    if !lock(&shared.state).trust.granted(peer, Grant::Clipboard) {
        return Err(Error::ClipboardDenied);
    }
    let mut slot = lock(&shared.clipboard);
    let Some(adapter) = slot.as_mut() else {
        return Err(Error::Unsupported("no clipboard adapter is installed"));
    };
    adapter.set_text(text)
}
