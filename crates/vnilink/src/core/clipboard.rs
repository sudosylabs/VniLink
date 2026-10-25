//! Clipboard stream for one control session.
// trace: records every ignored failure

use super::*;

use std::sync::Arc;

use crate::clipboard::{
    ClipboardPayload, ClipboardRequest, ClipboardSender, FileSelection, HostClipboard,
    StoredFileSelection,
};
use crate::error::Error;
use crate::events::CoreEvent;
use crate::session::Grant;

impl Core {
    /// Installs the adapter that receives clipboard text on this host.
    ///
    /// The library does not read or write the operating-system clipboard.
    /// An idle neutral connection may configure it before Desktop Open.
    pub fn set_clipboard(&self, clipboard: Box<dyn HostClipboard>) -> Result<(), Error> {
        let _engine = self.engine();
        let mut slot = lock(&self.shared.clipboard);
        let state = self.alive_state()?;
        if state
            .incoming
            .values()
            .any(|session| !finished(&session.phase) && !session.services_only)
            || self
                .shared
                .desktop_streams
                .load(std::sync::atomic::Ordering::Acquire)
                != 0
        {
            return Err(Error::Busy);
        }
        let _gate = self
            .shared
            .desktop_gate
            .try_lock()
            .map_err(|_| Error::Busy)?;
        *slot = Some(clipboard);
        self.shared
            .clipboard_available
            .store(true, std::sync::atomic::Ordering::Release);
        tracing::info!(target: crate::trace::CLIPBOARD, "clipboard adapter installed");
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
        let _engine = self.engine();
        let result = (|| {
            crate::clipboard::check_text(text)?;
            let tx = {
                let state = self.alive_state()?;
                clipboard_sender(&state, false, session_id, Some(generation))?
            };
            clipboard_round_trip(tx, text)
        })();
        note_sent("text", "to the host", &result);
        result
    }

    /// Sends this host's clipboard text to the controller.
    ///
    /// The controller reads it with [`Core::clipboard_text`]. Nothing is sent
    /// when the clipboard grant is off. Empty text clears the controller's
    /// copy. This does not read the operating-system clipboard.
    pub fn publish_clipboard(&self, session_id: u64, text: &str) -> Result<(), Error> {
        let _engine = self.engine();
        let result = (|| {
            crate::clipboard::check_text(text)?;
            let tx = {
                let state = self.alive_state()?;
                let session = live_session(&state, true, session_id)?;
                if !state.trust.granted(&session.peer_key, Grant::Clipboard) {
                    return Err(Error::ClipboardDenied);
                }
                // Host clipboard changes publish only to the selected session;
                // native code targets it via `clipboard_owner_session`.
                if state.clipboard_owner != Some(session_id) {
                    return Err(Error::Busy);
                }
                session
                    .clipboard
                    .clone()
                    .ok_or(Error::Unsupported("clipboard stream is not open"))?
            };
            clipboard_round_trip(tx, text)
        })();
        note_sent("text", "to the controller", &result);
        result
    }

    /// Hosted session currently selected for host clipboard sync, if a live
    /// hosted session exists. Native code publishes OS clipboard changes to
    /// this session only.
    pub fn clipboard_owner_session(&self) -> Result<Option<SessionInfo>, Error> {
        let _engine = self.engine();
        let state = self.alive_state()?;
        Ok(state
            .clipboard_owner
            .and_then(|id| state.incoming.get(&id))
            .filter(|s| !finished(&s.phase))
            .map(session_info))
    }

    /// Latest clipboard text published by the controlled computer.
    ///
    /// `None` before the first paste of this session. The text is replaced,
    /// not queued. Do not log the returned string.
    pub fn clipboard_text(&self) -> Result<Option<String>, Error> {
        let _engine = self.engine();
        self.ensure_alive()?;
        let state = self.alive_state()?;
        if state
            .outgoing
            .values()
            .filter(|s| !finished(&s.phase))
            .count()
            > 1
        {
            return Err(Error::Busy);
        }
        Ok(state
            .single_outgoing()
            .and_then(|s| s.controller.as_ref())
            .and_then(|c| lock(&c.clipboard).clone()))
    }
    /// Latest clipboard for this exact active controller generation.
    pub fn session_clipboard_text(
        &self,
        id: u64,
        generation: u64,
    ) -> Result<Option<String>, Error> {
        let controller = self.controller_state(id, generation)?;
        let text = lock(&controller.clipboard).clone();
        Ok(text)
    }

    /// Sends a file selection offer to the controlled computer.
    ///
    /// Returns when the host has stored or refused it. The clipboard grant is
    /// required. Only metadata crosses: names, sizes and directory markers.
    /// No payload byte rides this path; a paste materializes through the
    /// Files engine against the stored revision. An empty selection clears
    /// the host's copy. This does not read the local file manager selection.
    pub fn submit_clipboard_files(
        &self,
        session_id: u64,
        generation: u64,
        selection: &FileSelection,
    ) -> Result<(), Error> {
        let _engine = self.engine();
        let result = (|| {
            selection.validate()?;
            let (controller, tx) = {
                let state = self.alive_state()?;
                let tx = clipboard_sender(&state, false, session_id, Some(generation))?;
                let controller = state
                    .outgoing
                    .get(&session_id)
                    .and_then(|s| s.controller.clone())
                    .ok_or(Error::NoSession)?;
                (controller, tx)
            };
            // Issue before sending so concurrent offers never share a revision.
            let revision = controller
                .published_rev
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
                .saturating_add(1)
                .max(1);
            clipboard_round_trip_files(tx, revision, selection)?;
            // Record only delivered offers: a paste serves exactly the latest own
            // offer, so an undelivered offer must not authorize a fetch. An empty
            // offer stores an empty selection at a fresh revision, so the chain
            // stays unbroken and older revisions stay stale.
            *lock(&controller.published_files) = Some(StoredFileSelection {
                revision,
                selection: selection.clone(),
            });
            Ok(())
        })();
        note_sent("files", "to the host", &result);
        result
    }

    /// Sends this host's file selection to the controller.
    ///
    /// The controller reads it with [`Core::clipboard_files`]. Nothing is sent
    /// when the clipboard grant is off. An empty selection clears the
    /// controller's copy. This does not read the operating-system selection.
    pub fn publish_clipboard_files(
        &self,
        session_id: u64,
        selection: &FileSelection,
    ) -> Result<(), Error> {
        let _engine = self.engine();
        let result = (|| {
            selection.validate()?;
            let (revision, tx) = {
                let state = self.alive_state()?;
                let session = live_session(&state, true, session_id)?;
                if !state.trust.granted(&session.peer_key, Grant::Clipboard) {
                    return Err(Error::ClipboardDenied);
                }
                // Host selections publish only to the selected session, like text.
                if state.clipboard_owner != Some(session_id) {
                    return Err(Error::Busy);
                }
                // Issue before sending so concurrent offers never share a revision.
                let revision = session
                    .published_rev
                    .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
                    .saturating_add(1)
                    .max(1);
                let tx = session
                    .clipboard
                    .clone()
                    .ok_or(Error::Unsupported("clipboard stream is not open"))?;
                (revision, tx)
            };
            clipboard_round_trip_files(tx, revision, selection)?;
            if let Some(session) = lock(&self.shared.state).incoming.get_mut(&session_id) {
                // An empty offer stores an empty selection at a fresh revision,
                // so the chain stays unbroken and older revisions stay stale.
                session.published_files = Some(StoredFileSelection {
                    revision,
                    selection: selection.clone(),
                });
            }
            Ok(())
        })();
        note_sent("files", "to the controller", &result);
        result
    }

    /// Latest file selection published by the controlled computer.
    ///
    /// `None` before the first offer of this session or after a clear. The
    /// selection is replaced, not queued, and each offer carries a fresh
    /// source-assigned revision. Do not log the returned paths.
    pub fn clipboard_files(&self) -> Result<Option<StoredFileSelection>, Error> {
        let _engine = self.engine();
        self.ensure_alive()?;
        let state = self.alive_state()?;
        if state
            .outgoing
            .values()
            .filter(|s| !finished(&s.phase))
            .count()
            > 1
        {
            return Err(Error::Busy);
        }
        Ok(state
            .single_outgoing()
            .and_then(|s| s.controller.as_ref())
            .and_then(|c| lock(&c.clipboard_files).clone())
            .filter(|stored| !stored.selection.entries.is_empty()))
    }

    /// Latest file selection for this exact active controller generation.
    pub fn session_clipboard_files(
        &self,
        id: u64,
        generation: u64,
    ) -> Result<Option<StoredFileSelection>, Error> {
        let controller = self.controller_state(id, generation)?;
        let stored = lock(&controller.clipboard_files).clone();
        Ok(stored.filter(|stored| !stored.selection.entries.is_empty()))
    }

    /// Latest file selection a controller offered to this host on one live
    /// hosted session. Native code offers it to the local file manager and
    /// materializes a user-confirmed paste through the Files engine against
    /// the stored revision. Cleared on revoke because a selection stays a
    /// live capability after the grant is gone.
    pub fn incoming_clipboard_files(
        &self,
        session_id: u64,
    ) -> Result<Option<StoredFileSelection>, Error> {
        let state = self.alive_state()?;
        let session = live_session(&state, true, session_id)?;
        Ok(session
            .clipboard_files
            .clone()
            .filter(|stored| !stored.selection.entries.is_empty()))
    }
}

/// Records a paste that did not reach the other computer: refused here
/// (grant, owner, stream) or answered with a refusal there. What crossed is
/// recorded by the stream.
fn note_sent(kind: &'static str, direction: &'static str, result: &Result<(), Error>) {
    if let Err(error) = result {
        tracing::info!(
            target: crate::trace::CLIPBOARD,
            kind,
            direction,
            refused = error.name(),
            reason = error.reason(),
            "clipboard not delivered"
        );
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
    let controller = lock(&shared.state)
        .outgoing
        .get(&session_id)
        .and_then(|s| s.controller.clone())
        .ok_or(Error::NoSession)?;
    *lock(&controller.clipboard) = None;
    *lock(&controller.clipboard_files) = None;
    *lock(&controller.published_files) = None;
    let connection = iroh::endpoint::Connection::clone(connection);
    let stop_rx = tokio::sync::watch::Receiver::clone(stop_rx);
    let task = tokio::spawn(async move {
        crate::clipboard::attach(
            connection,
            true,
            rx,
            stop_rx,
            Some(opened_tx),
            move |payload| match payload {
                ClipboardPayload::Text(text) => {
                    *lock(&controller.clipboard) = Some(text.to_owned());
                    shared_text
                        .events
                        .publish(CoreEvent::ClipboardReceived { id: session_id });
                    Ok(())
                }
                ClipboardPayload::Files {
                    revision,
                    selection,
                } => {
                    // The revision is source-assigned: the stored offer names
                    // the exact revision a paste must pin, so receipts can
                    // never drift from the source's published chain.
                    *lock(&controller.clipboard_files) = Some(StoredFileSelection {
                        revision: *revision,
                        selection: selection.clone(),
                    });
                    shared_text
                        .events
                        .publish(CoreEvent::ClipboardFilesReceived { id: session_id });
                    Ok(())
                }
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
        if let Some(session) = state.outgoing.get_mut(&session_id) {
            if session.id == session_id {
                session.clipboard = Some(tx);
            }
        }
    }
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
    clipboard_round_trip_payload(tx, ClipboardPayload::Text(text.to_owned()))
}

pub(super) fn clipboard_round_trip_files(
    tx: ClipboardSender,
    revision: u64,
    selection: &FileSelection,
) -> Result<(), Error> {
    clipboard_round_trip_payload(
        tx,
        ClipboardPayload::Files {
            revision,
            selection: selection.clone(),
        },
    )
}

fn clipboard_round_trip_payload(
    tx: ClipboardSender,
    payload: ClipboardPayload,
) -> Result<(), Error> {
    round_trip(&tx, "clipboard stream is closed", |reply| {
        ClipboardRequest { payload, reply }
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
    let applied = adapter.set_text(text);
    if let Err(error) = &applied {
        tracing::warn!(
            target: crate::trace::CLIPBOARD,
            bytes = text.len(),
            failure = error.name(),
            reason = error.reason(),
            "this computer's clipboard refused the text"
        );
    }
    applied
}

/// Stores a controller's file selection on one hosted session. The revision
/// is source-assigned: it names the exact revision a paste must pin, so
/// receipts can never drift from the source's published chain. A clear stores
/// an empty selection at a fresh revision instead of dropping the revision
/// chain, so a paste pinned to any older revision stays stale.
pub(super) fn apply_clipboard_files(
    shared: &Shared,
    peer: &[u8; 32],
    session_id: u64,
    revision: u64,
    selection: &FileSelection,
) -> Result<(), Error> {
    selection.validate().map_err(|_| Error::Malformed)?;
    let mut state = lock(&shared.state);
    if !state.trust.granted(peer, Grant::Clipboard) {
        return Err(Error::ClipboardDenied);
    }
    let session = state
        .incoming
        .get_mut(&session_id)
        .filter(|s| !finished(&s.phase))
        .ok_or(Error::NoSession)?;
    session.clipboard_files = Some(StoredFileSelection {
        revision,
        selection: selection.clone(),
    });
    shared
        .events
        .publish(CoreEvent::ClipboardFilesReceived { id: session_id });
    Ok(())
}

/// Withdraws a peer's file selections on every session: received offers and
/// own offers alike. Unlike inert text, a stored selection stays a live
/// paste capability after the grant is gone.
pub(super) fn drop_peer_clipboard_files(shared: &Shared, peer: &[u8; 32]) {
    let mut state = lock(&shared.state);
    for session in state.incoming.values_mut().filter(|s| &s.peer_key == peer) {
        session.clipboard_files = None;
        session.published_files = None;
    }
    let controllers: Vec<_> = state
        .outgoing
        .values()
        .filter(|s| &s.peer_key == peer)
        .filter_map(|s| s.controller.clone())
        .collect();
    drop(state);
    for controller in controllers {
        *lock(&controller.clipboard_files) = None;
        *lock(&controller.published_files) = None;
    }
}

/// Whether one hosted session currently owns host clipboard sync. Peer text
/// applies to the OS clipboard only from the selected session, so one peer's
/// selection never lands while another peer is syncing.
pub(super) fn clipboard_selected(state: &State, session_id: u64) -> bool {
    state.clipboard_owner == Some(session_id)
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
        host.set_desktop_sharing(Grants {
            clipboard: true,
            ..Grants::default()
        })
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

#[cfg(test)]
mod selection_confinement_tests {
    use super::*;
    use crate::tests::support::start_direct;

    fn selection(path: &str) -> crate::clipboard::FileSelection {
        crate::clipboard::FileSelection {
            entries: vec![crate::clipboard::ClipboardFileEntry {
                path: path.into(),
                size: 1,
                dir: false,
            }],
        }
    }

    #[test]
    fn malformed_selections_die_before_touching_state() {
        let core = start_direct();
        for path in ["../outside.txt", "/etc/hosts", "a/../../b", ""] {
            let bad = selection(path);
            assert!(
                matches!(
                    apply_clipboard_files(&core.shared, &[9; 32], 7, 1, &bad),
                    Err(crate::Error::Malformed)
                ),
                "{path} stored"
            );
        }
        core.shutdown();
    }
}
