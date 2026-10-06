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
        self.shared
            .clipboard_available
            .store(true, std::sync::atomic::Ordering::Release);
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

#[cfg(test)]
mod snapshot_tests {
    use super::*;
    use std::sync::mpsc;

    struct WaitingClipboard {
        entered: mpsc::Sender<()>,
        release: mpsc::Receiver<()>,
    }
    impl HostClipboard for WaitingClipboard {
        fn set_text(&mut self, _: &str) -> Result<(), Error> {
            self.entered.send(()).unwrap();
            self.release.recv().unwrap();
            Ok(())
        }
    }
    #[test]
    fn snapshot_does_not_wait_for_a_native_clipboard_callback() {
        let config = || CoreConfig {
            listen: Listen::Loopback,
            relay: RelaySettings::custom(vec![
                crate::CustomRelay::new("http://127.0.0.1:9").unwrap()
            ])
            .unwrap(),
            ..CoreConfig::default()
        };
        let host = Arc::new(Core::start(config()).unwrap());
        let peer = Core::start(config()).unwrap();
        assert!(!host.snapshot().unwrap().capabilities.clipboard);
        host.accept_invitation(&peer.create_invitation(Duration::from_secs(60)).unwrap())
            .unwrap();
        host.authorize(
            &peer.endpoint_id(),
            Grants {
                clipboard: true,
                ..Grants::default()
            },
        )
        .unwrap();
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        host.set_clipboard(Box::new(WaitingClipboard {
            entered: entered_tx,
            release: release_rx,
        }))
        .unwrap();
        let writer = host.clone();
        let key = parse_endpoint_id(&peer.endpoint_id()).unwrap();
        let write = std::thread::spawn(move || apply_clipboard(&writer.shared, &key, "fixture"));
        entered_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        let observer = host.clone();
        let (done_tx, done_rx) = mpsc::channel();
        let read = std::thread::spawn(move || {
            let _ = done_tx.send(observer.snapshot().unwrap().capabilities.clipboard);
        });
        let available = done_rx.recv_timeout(Duration::from_millis(200));
        // Always release the adapter, including the failing implementation.
        release_tx.send(()).unwrap();
        write.join().unwrap().unwrap();
        read.join().unwrap();
        peer.shutdown();
        host.shutdown();
        assert_eq!(
            available,
            Ok(true),
            "the UI snapshot blocked the native clipboard writer"
        );
    }
}
