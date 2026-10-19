//! Clipboard stream for one control session.

use super::*;

use std::sync::Arc;

use crate::clipboard::{
    ClipboardPayload, ClipboardRequest, ClipboardSender, FileSelection, HostClipboard,
    PreparedPaste, StoredFileSelection,
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
    }

    /// Hosted session currently selected for host clipboard sync, if a live
    /// hosted session exists. Native code publishes OS clipboard changes to
    /// this session only.
    pub fn clipboard_owner_session(&self) -> Result<Option<SessionInfo>, Error> {
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
        // Record only delivered offers: paste preparation serves exactly the
        // latest own offer, so an undelivered offer must not authorize a fetch.
        // An empty offer stores an empty selection at a fresh revision, so
        // the chain stays unbroken and older preparations stay stale.
        *lock(&controller.published_files) = Some(StoredFileSelection {
            revision,
            selection: selection.clone(),
        });
        Ok(())
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
            // so the chain stays unbroken and older preparations stay stale.
            session.published_files = Some(StoredFileSelection {
                revision,
                selection: selection.clone(),
            });
        }
        Ok(())
    }

    /// Latest file selection published by the controlled computer.
    ///
    /// `None` before the first offer of this session or after a clear. The
    /// selection is replaced, not queued, and each offer carries a fresh
    /// source-assigned revision. Do not log the returned paths.
    pub fn clipboard_files(&self) -> Result<Option<StoredFileSelection>, Error> {
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

    /// Installs the resolver that maps offered selection paths to absolute
    /// source files for paste preparation. Either role can serve pastes, so
    /// both cores install it. The resolver owns the file-manager selection
    /// scope; preparation refuses whatever it refuses.
    pub fn set_clipboard_file_source(
        &self,
        source: Option<std::sync::Arc<dyn crate::clipboard::HostClipboardFiles>>,
    ) -> Result<(), Error> {
        self.ensure_alive()?;
        *lock(&self.shared.clipboard_file_source) = source;
        Ok(())
    }

    /// Prepares a paste of a controller's offer on one hosted session: asks
    /// the controller to hash and offer the revision's files, then returns
    /// their content hashes and source-measured sizes for fetching through
    /// the blob APIs. Needs the Transfer grant; the clipboard grant only
    /// covers the offer itself. Call on a background thread: hashing runs at
    /// disk speed and the wait ends on readiness, source failure, session
    /// loss or the preparation budget. One preparation per session at a time.
    pub fn prepare_incoming_paste(
        &self,
        session_id: u64,
        revision: u64,
    ) -> Result<PreparedPaste, Error> {
        let (revision, selection, tx) = {
            let state = self.alive_state()?;
            let session = live_session(&state, true, session_id)?;
            if !state.trust.granted(&session.peer_key, Grant::Transfer) {
                return Err(Error::TransferDenied);
            }
            let stored = session.clipboard_files.clone().filter(|stored| {
                !stored.selection.entries.is_empty() && stored.revision == revision
            });
            let Some(stored) = stored else {
                return Err(Error::StaleSession);
            };
            let tx = session
                .clipboard
                .clone()
                .ok_or(Error::Unsupported("clipboard stream is not open"))?;
            (stored.revision, stored.selection, tx)
        };
        self.prepare_paste(true, session_id, None, revision, &selection, tx)
    }

    /// Prepares a paste of the host's offer on one controller session. Same
    /// contract as [`Core::prepare_incoming_paste`] in the other direction.
    pub fn prepare_outgoing_paste(
        &self,
        id: u64,
        generation: u64,
        revision: u64,
    ) -> Result<PreparedPaste, Error> {
        let controller = self.controller_state(id, generation)?;
        let tx = {
            let state = self.alive_state()?;
            let session = live_session(&state, false, id)?;
            if !state.trust.granted(&session.peer_key, Grant::Transfer) {
                return Err(Error::TransferDenied);
            }
            session
                .clipboard
                .clone()
                .ok_or(Error::Unsupported("clipboard stream is not open"))?
        };
        let stored = lock(&controller.clipboard_files)
            .clone()
            .filter(|stored| !stored.selection.entries.is_empty() && stored.revision == revision);
        let Some(stored) = stored else {
            return Err(Error::StaleSession);
        };
        self.prepare_paste(
            false,
            id,
            Some(generation),
            stored.revision,
            &stored.selection,
            tx,
        )
    }

    fn prepare_paste(
        &self,
        incoming: bool,
        session_id: u64,
        generation: Option<u64>,
        revision: u64,
        selection: &FileSelection,
        tx: ClipboardSender,
    ) -> Result<PreparedPaste, Error> {
        // Directories need no hashing: the receiver recreates them from its
        // stored selection, so a directories-only paste never hits the wire.
        if selection.entries.iter().all(|entry| entry.dir) {
            return Ok(PreparedPaste::default());
        }
        let (wait_tx, wait_rx) = std::sync::mpsc::sync_channel(1);
        let nonce = self
            .shared
            .paste_nonce
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        {
            let mut waiters = lock(&self.shared.paste_waiters);
            if waiters.contains_key(&(incoming, session_id)) {
                return Err(Error::Busy);
            }
            waiters.insert(
                (incoming, session_id),
                PasteWaiter {
                    nonce,
                    // Files only, mirroring the source: directories are
                    // recreated from the stored selection without fetching.
                    paths: selection
                        .entries
                        .iter()
                        .filter(|entry| !entry.dir)
                        .map(|entry| entry.path.clone())
                        .collect(),
                    tx: wait_tx,
                },
            );
        }
        let send = clipboard_round_trip_payload(
            tx,
            ClipboardPayload::Prepare {
                nonce,
                revision,
                selection: selection.clone(),
            },
        );
        if let Err(error) = send {
            remove_paste_waiter(&self.shared, incoming, session_id, nonce);
            return Err(error);
        }
        let deadline = std::time::Instant::now() + PASTE_PREPARE_BUDGET;
        loop {
            match wait_rx.recv_timeout(std::time::Duration::from_millis(100)) {
                Ok(result) => {
                    remove_paste_waiter(&self.shared, incoming, session_id, nonce);
                    return result;
                }
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                    remove_paste_waiter(&self.shared, incoming, session_id, nonce);
                    return Err(Error::NoSession);
                }
            }
            if std::time::Instant::now() >= deadline {
                remove_paste_waiter(&self.shared, incoming, session_id, nonce);
                return Err(Error::Timeout);
            }
            let alive = (|| {
                let state = self.alive_state().ok()?;
                let session = live_session(&state, incoming, session_id).ok()?;
                if let Some(generation) = generation {
                    if session.generation != generation {
                        return None;
                    }
                }
                if session.clipboard.as_ref().is_some_and(|tx| tx.is_closed()) {
                    return None;
                }
                Some(())
            })();
            if alive.is_none() {
                remove_paste_waiter(&self.shared, incoming, session_id, nonce);
                return Err(Error::NoSession);
            }
        }
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
                    // the exact revision preparation must pin, so receipts
                    // can never drift from the source's published chain.
                    *lock(&controller.clipboard_files) = Some(StoredFileSelection {
                        revision: *revision,
                        selection: selection.clone(),
                    });
                    shared_text
                        .events
                        .publish(CoreEvent::ClipboardFilesReceived { id: session_id });
                    Ok(())
                }
                ClipboardPayload::Prepare {
                    nonce,
                    revision,
                    selection,
                } => {
                    let peer = lock(&shared_text.state)
                        .outgoing
                        .get(&session_id)
                        .map(|session| session.peer_key)
                        .ok_or(Error::NoSession)?;
                    spawn_paste_preparation(
                        &shared_text,
                        false,
                        session_id,
                        peer,
                        *nonce,
                        *revision,
                        selection,
                    )
                }
                ClipboardPayload::FilesReady { nonce, outcome } => {
                    deliver_paste_ready(&shared_text, false, session_id, *nonce, outcome);
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

/// Backstop for one paste preparation. Hashing runs at disk speed, so the
/// wait usually ends on readiness or session loss long before this.
const PASTE_PREPARE_BUDGET: std::time::Duration = std::time::Duration::from_secs(600);

pub(super) struct PasteWaiter {
    nonce: u64,
    paths: Vec<String>,
    tx: std::sync::mpsc::SyncSender<Result<PreparedPaste, Error>>,
}

/// Removes one waiter only when it is still ours. A replaced selection
/// registers a new waiter under the same session key; unqualified cleanup
/// would steal it.
fn remove_paste_waiter(shared: &Shared, incoming: bool, session_id: u64, nonce: u64) {
    let mut waiters = lock(&shared.paste_waiters);
    if waiters
        .get(&(incoming, session_id))
        .is_some_and(|waiter| waiter.nonce == nonce)
    {
        waiters.remove(&(incoming, session_id));
    }
}

/// Delivers a source's ready outcome to the session's waiter. A nonce
/// mismatch means a late answer for a replaced selection: it is ignored
/// without removing the live waiter. A length mismatch means a hostile
/// or broken peer. Late frames with no waiter are dropped.
pub(super) fn deliver_paste_ready(
    shared: &Shared,
    incoming: bool,
    session_id: u64,
    nonce: u64,
    outcome: &crate::clipboard::PasteReadyOutcome,
) {
    let waiter = {
        let mut waiters = lock(&shared.paste_waiters);
        match waiters.get(&(incoming, session_id)) {
            Some(live) if live.nonce != nonce => return,
            _ => waiters.remove(&(incoming, session_id)),
        }
    };
    let Some(waiter) = waiter else {
        return;
    };
    let result = match outcome {
        crate::clipboard::PasteReadyOutcome::Ready(files) => {
            if files.len() != waiter.paths.len() {
                Err(Error::Malformed)
            } else {
                Ok(PreparedPaste {
                    files: waiter
                        .paths
                        .into_iter()
                        .zip(files.iter())
                        .map(|(path, (hash, size))| crate::clipboard::PreparedFile {
                            path,
                            hash: *hash,
                            size: *size,
                        })
                        .collect(),
                })
            }
        }
        crate::clipboard::PasteReadyOutcome::TransferDenied => Err(Error::TransferDenied),
        crate::clipboard::PasteReadyOutcome::Unauthorized => Err(Error::Unauthorized),
        crate::clipboard::PasteReadyOutcome::Unavailable => {
            Err(Error::Unsupported("paste source is unavailable"))
        }
    };
    let _ = waiter.tx.send(result);
}

/// Releases one serving-side preparation reservation when its worker
/// settles on any path, including spawn failure and session loss.
struct PastePreparingGuard {
    shared: std::sync::Arc<Shared>,
    key: (bool, u64),
}

impl Drop for PastePreparingGuard {
    fn drop(&mut self) {
        lock(&self.shared.paste_preparing).remove(&self.key);
    }
}

/// Answers a paste preparation request on a blocking thread, so slow disks
/// never stall the clipboard stream. One worker per session side: a second
/// concurrent prepare is Busy instead of spawning another detached thread.
/// The worker captures the session generation and rechecks liveness plus
/// the Transfer grant after slow hashing: a result for a replaced or dead
/// session is dropped (the requester detects the loss), while a result
/// computed before a mid-work revocation is downgraded to TransferDenied
/// instead of delivering hashes. Every admitted prepare gets
/// exactly one ready outcome, including refusals: the dial side of the
/// stream writes no statuses, so a refusal must ride the ready frame to
/// reach a host-originated prepare. Only files hash; the receiver
/// recreates directories from its stored selection. Returns the status
/// answer for the request itself.
/// True when the session's latest own offer is still the requested revision
/// and covers the selection. Checked before hashing and again after: a
/// re-offer mid-work — even with identical paths — must not leave stale
/// hashes authorized.
fn offer_covers(
    shared: &Shared,
    incoming: bool,
    session_id: u64,
    revision: u64,
    selection: &FileSelection,
) -> bool {
    let state = lock(&shared.state);
    let offered = if incoming {
        state
            .incoming
            .get(&session_id)
            .and_then(|s| s.published_files.clone())
    } else {
        state
            .outgoing
            .get(&session_id)
            .and_then(|s| s.controller.clone())
            .and_then(|c| lock(&c.published_files).clone())
    };
    offered
        .is_some_and(|offered| offered.revision == revision && offered.selection.covers(selection))
}

pub(crate) fn spawn_paste_preparation(
    shared: &std::sync::Arc<Shared>,
    incoming: bool,
    session_id: u64,
    peer: [u8; 32],
    nonce: u64,
    revision: u64,
    selection: &FileSelection,
) -> Result<(), Error> {
    selection.validate().map_err(|_| Error::Malformed)?;
    let runtime = tokio::runtime::Handle::try_current().map_err(|_| Error::NoSession)?;
    let generation = {
        let state = lock(&shared.state);
        let sessions = if incoming {
            &state.incoming
        } else {
            &state.outgoing
        };
        sessions
            .get(&session_id)
            .filter(|session| !finished(&session.phase))
            .map(|session| session.generation)
            .ok_or(Error::NoSession)?
    };
    if !lock(&shared.paste_preparing).insert((incoming, session_id)) {
        return Err(Error::Busy);
    }
    let guard = PastePreparingGuard {
        shared: std::sync::Arc::clone(shared),
        key: (incoming, session_id),
    };
    let shared = std::sync::Arc::clone(shared);
    let selection = selection.clone();
    std::thread::Builder::new()
        .name("vnilink-paste-prepare".into())
        .spawn(move || {
            let _guard = guard;
            let outcome = prepare_outcome(
                &shared, &runtime, incoming, session_id, peer, revision, &selection,
            );
            let (tx, granted) = {
                let state = lock(&shared.state);
                let sessions = if incoming {
                    &state.incoming
                } else {
                    &state.outgoing
                };
                let live = sessions.get(&session_id).filter(|session| {
                    session.generation == generation && !finished(&session.phase)
                });
                // Dead or replaced session: the requester detects the loss
                // on its own poll; there is no one to answer.
                let Some(session) = live else {
                    return;
                };
                let granted = state.trust.granted(&peer, Grant::Transfer);
                (session.clipboard.clone(), granted)
            };
            let Some(tx) = tx else { return };
            // Grant lost mid-work: answer TransferDenied like the start
            // check. Computed hashes are discarded, never delivered.
            // Otherwise the offer may have changed mid-work: when it no
            // longer covers the request, stale hashes are refused, never
            // delivered.
            let outcome = if !granted {
                crate::clipboard::PasteReadyOutcome::TransferDenied
            } else if !offer_covers(&shared, incoming, session_id, revision, &selection) {
                crate::clipboard::PasteReadyOutcome::Unauthorized
            } else {
                outcome
            };
            // The reply half is dropped: the ready frame is the answer, and
            // the stream tolerates a gone reply on both sides.
            let (reply_tx, _) = std::sync::mpsc::sync_channel(1);
            let _ = tx.blocking_send(ClipboardRequest {
                payload: ClipboardPayload::FilesReady { nonce, outcome },
                reply: reply_tx,
            });
        })
        .map_err(|_| Error::Unsupported("paste preparation did not start"))?;
    Ok(())
}

fn prepare_outcome(
    shared: &Shared,
    runtime: &tokio::runtime::Handle,
    incoming: bool,
    session_id: u64,
    peer: [u8; 32],
    revision: u64,
    selection: &FileSelection,
) -> crate::clipboard::PasteReadyOutcome {
    use crate::clipboard::PasteReadyOutcome;
    if !lock(&shared.state).trust.granted(&peer, Grant::Transfer) {
        return PasteReadyOutcome::TransferDenied;
    }
    // A paste serves only the requested revision of this session's latest
    // own offer. The Transfer grant alone never authorizes a fetch of
    // unoffered files, and a same-path re-offer is a new revision.
    if !offer_covers(shared, incoming, session_id, revision, selection) {
        return PasteReadyOutcome::Unauthorized;
    }
    if lock(&shared.clipboard_file_source).is_none() {
        return PasteReadyOutcome::Unavailable;
    }
    let mut ready: Vec<([u8; 32], u64)> = Vec::with_capacity(selection.entries.len());
    for entry in selection.entries.iter().filter(|entry| !entry.dir) {
        let source = lock(&shared.clipboard_file_source).clone();
        let resolved = source
            .as_ref()
            .map(|source| source.resolve_source(&entry.path))
            .unwrap_or_else(|| Err(Error::Unsupported("paste source is unavailable")));
        let Ok(resolved) = resolved else {
            return PasteReadyOutcome::Unavailable;
        };
        match runtime.block_on(super::blob::offer_file_for_peer(
            shared,
            peer,
            &resolved,
            u64::MAX,
            true,
        )) {
            Ok((hash, size)) => ready.push((*hash.as_bytes(), size)),
            Err(_) => return PasteReadyOutcome::Unavailable,
        }
    }
    PasteReadyOutcome::Ready(ready)
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

/// Stores a controller's file selection on one hosted session. The revision
/// is source-assigned: it names the exact revision preparation must pin, so
/// receipts can never drift from the source's published chain. A clear stores
/// an empty selection at a fresh revision instead of dropping the revision
/// chain, so a paste prepared against any older revision stays stale.
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
mod review_correlation_tests {
    use super::*;
    #[test]
    fn review_late_paste_reply_cannot_satisfy_replacement_selection() {
        let core = crate::tests::support::start_direct();
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        lock(&core.shared.paste_waiters).insert(
            (false, 123),
            PasteWaiter {
                nonce: 7,
                paths: vec!["new-selection.txt".into()],
                tx,
            },
        );
        let stale = crate::clipboard::PasteReadyOutcome::Ready(vec![([7; 32], 99)]);
        deliver_paste_ready(&core.shared, false, 123, 8, &stale);
        let received = rx.try_recv();
        assert!(
            received.is_err(),
            "old selection hash delivered into replacement waiter: {received:?}"
        );
        // The mismatched answer leaves the live waiter registered: its own
        // answer still completes it.
        deliver_paste_ready(&core.shared, false, 123, 7, &stale);
        let received = rx.try_recv().expect("matched answer dropped");
        assert_eq!(received.unwrap().files.len(), 1);
    }
}

#[cfg(test)]
mod paste_preparation_guard_tests {
    use super::*;
    use crate::tests::support::{pair_for_control, start_direct};

    struct Gate {
        entered: std::sync::mpsc::Sender<()>,
        release: std::sync::mpsc::Receiver<()>,
    }

    struct GatedSource {
        root: std::path::PathBuf,
        gate: std::sync::Mutex<Option<Gate>>,
    }

    impl crate::clipboard::HostClipboardFiles for GatedSource {
        fn resolve_source(&self, path: &str) -> Result<std::path::PathBuf, crate::Error> {
            if let Some(gate) = self.gate.lock().unwrap().take() {
                let _ = gate.entered.send(());
                let _ = gate.release.recv();
            }
            Ok(self.root.join(path))
        }
    }

    #[test]
    fn stale_worker_reply_cannot_satisfy_live_waiter() {
        let tag = format!(
            "vnilink-paste-stale-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let root = std::env::temp_dir().join(tag);
        std::fs::create_dir_all(root.join("files")).unwrap();
        std::fs::create_dir_all(root.join("blobs")).unwrap();
        std::fs::write(root.join("files/a.txt"), b"alpha").unwrap();
        std::fs::write(root.join("files/b.txt"), b"bravo-bravo").unwrap();
        let host = start_direct();
        let controller = start_direct();
        host.set_blob_directory(root.join("blobs")).unwrap();
        let (entered_tx, entered_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        host.set_clipboard_file_source(Some(std::sync::Arc::new(GatedSource {
            root: root.join("files"),
            gate: std::sync::Mutex::new(Some(Gate {
                entered: entered_tx,
                release: release_rx,
            })),
        })))
        .unwrap();
        pair_for_control(&host, &controller);
        for core in [&host, &controller] {
            core.set_desktop_sharing(crate::Grants {
                view: true,
                control: true,
                clipboard: true,
                transfer: true,
            })
            .unwrap();
        }
        let session = controller.connect(&host.endpoint_id()).unwrap();
        let hosted = host.snapshot().unwrap().incoming.unwrap();
        let offer = |path: &str, size: u64| crate::clipboard::FileSelection {
            entries: vec![crate::clipboard::ClipboardFileEntry {
                path: path.into(),
                size,
                dir: false,
            }],
        };
        host.publish_clipboard_files(hosted.id, &offer("a.txt", 5))
            .unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(8);
        loop {
            let revision = controller
                .clipboard_files()
                .unwrap()
                .map(|stored| stored.revision);
            if revision == Some(1) {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "controller never stored offer A"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let controller = std::sync::Arc::new(controller);
        let prepare_controller = std::sync::Arc::clone(&controller);
        let prepare_a = std::thread::spawn(move || {
            prepare_controller.prepare_outgoing_paste(session.id, session.generation, 1)
        });
        entered_rx
            .recv_timeout(std::time::Duration::from_secs(8))
            .expect("worker never reached the resolver");
        // Simulate A's timeout exactly: drop its waiter and answer Timeout
        // on its channel, the same state transition as the budget arm.
        let waiter_a = lock(&controller.shared.paste_waiters).remove(&(false, session.id));
        let waiter_a = waiter_a.expect("A registered no waiter");
        waiter_a
            .tx
            .send(Err(crate::Error::Timeout))
            .expect("A waiter channel closed");
        assert!(
            matches!(
                prepare_a.join().expect("prepare thread panicked"),
                Err(crate::Error::Timeout)
            ),
            "A did not time out"
        );
        // B registers under the same session key with equal file count
        // while A's worker is still parked.
        let (b_tx, b_rx) = std::sync::mpsc::sync_channel(1);
        lock(&controller.shared.paste_waiters).insert(
            (false, session.id),
            PasteWaiter {
                nonce: u64::MAX - 7,
                paths: vec!["b.txt".into()],
                tx: b_tx,
            },
        );
        let _ = release_tx.send(());
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(8);
        while !lock(&host.shared.paste_preparing).is_empty() {
            assert!(
                std::time::Instant::now() < deadline,
                "A worker never settled"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        // A's late answer arrives while B waits: ignored, and B's waiter
        // is still registered.
        assert!(
            b_rx.try_recv().is_err(),
            "stale A hashes completed live B waiter"
        );
        assert!(
            lock(&controller.shared.paste_waiters).contains_key(&(false, session.id)),
            "stale answer removed the live waiter"
        );
        lock(&controller.shared.paste_waiters).remove(&(false, session.id));
        // The live path still completes with B's own content.
        host.publish_clipboard_files(hosted.id, &offer("b.txt", 11))
            .unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(8);
        loop {
            let revision = controller
                .clipboard_files()
                .unwrap()
                .map(|stored| stored.revision);
            if revision == Some(2) {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "controller never stored offer B"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let prepared = controller
            .prepare_outgoing_paste(session.id, session.generation, 2)
            .unwrap();
        assert_eq!(prepared.files.len(), 1);
        assert_eq!(prepared.files[0].path, "b.txt");
        assert_eq!(prepared.files[0].size, 11);
        let dest = root.join("fetched-b.txt");
        host.blob_export(prepared.files[0].blob_hash(), &dest)
            .unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), b"bravo-bravo");
        assert!(lock(&controller.shared.paste_waiters).is_empty());
        assert!(lock(&host.shared.paste_preparing).is_empty());
        host.shutdown();
        controller.shutdown();
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn reoffer_mid_hash_refuses_stale_preparation() {
        let tag = format!(
            "vnilink-paste-reoffer-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let root = std::env::temp_dir().join(tag);
        std::fs::create_dir_all(root.join("files")).unwrap();
        std::fs::create_dir_all(root.join("blobs")).unwrap();
        std::fs::write(root.join("files/a.txt"), b"alpha").unwrap();
        std::fs::write(root.join("files/b.txt"), b"bravo").unwrap();
        let host = start_direct();
        let controller = start_direct();
        host.set_blob_directory(root.join("blobs")).unwrap();
        let (entered_tx, entered_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        host.set_clipboard_file_source(Some(std::sync::Arc::new(GatedSource {
            root: root.join("files"),
            gate: std::sync::Mutex::new(Some(Gate {
                entered: entered_tx,
                release: release_rx,
            })),
        })))
        .unwrap();
        pair_for_control(&host, &controller);
        for core in [&host, &controller] {
            core.set_desktop_sharing(crate::Grants {
                view: true,
                control: true,
                clipboard: true,
                transfer: true,
            })
            .unwrap();
        }
        let session = controller.connect(&host.endpoint_id()).unwrap();
        let hosted = host.snapshot().unwrap().incoming.unwrap();
        let offer = |path: &str, size: u64| crate::clipboard::FileSelection {
            entries: vec![crate::clipboard::ClipboardFileEntry {
                path: path.into(),
                size,
                dir: false,
            }],
        };
        host.publish_clipboard_files(hosted.id, &offer("a.txt", 5))
            .unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(8);
        loop {
            if controller
                .clipboard_files()
                .unwrap()
                .is_some_and(|stored| stored.revision == 1)
            {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "controller never stored offer A"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let controller = std::sync::Arc::new(controller);
        let prepare_controller = std::sync::Arc::clone(&controller);
        let prepare = std::thread::spawn(move || {
            prepare_controller.prepare_outgoing_paste(session.id, session.generation, 1)
        });
        entered_rx
            .recv_timeout(std::time::Duration::from_secs(8))
            .expect("worker never reached the resolver");
        // The source re-offers while A hashes: A is no longer covered.
        host.publish_clipboard_files(hosted.id, &offer("b.txt", 5))
            .unwrap();
        let _ = release_tx.send(());
        let outcome = prepare.join().expect("prepare thread panicked");
        assert!(
            matches!(outcome, Err(crate::Error::Unauthorized)),
            "stale hashes survived a re-offer: {outcome:?}"
        );
        assert!(lock(&controller.shared.paste_waiters).is_empty());
        host.shutdown();
        controller.shutdown();
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn concurrent_prepare_is_busy_and_dead_session_settles_without_reply() {
        let tag = format!(
            "vnilink-paste-guard-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let root = std::env::temp_dir().join(tag);
        std::fs::create_dir_all(root.join("files")).unwrap();
        std::fs::create_dir_all(root.join("blobs")).unwrap();
        std::fs::write(root.join("files/a.txt"), b"alpha").unwrap();
        let host = start_direct();
        let controller = start_direct();
        controller.set_blob_directory(root.join("blobs")).unwrap();
        let (entered_tx, entered_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        controller
            .set_clipboard_file_source(Some(std::sync::Arc::new(GatedSource {
                root: root.join("files"),
                gate: std::sync::Mutex::new(Some(Gate {
                    entered: entered_tx,
                    release: release_rx,
                })),
            })))
            .unwrap();
        pair_for_control(&host, &controller);
        for core in [&host, &controller] {
            core.set_desktop_sharing(crate::Grants {
                view: true,
                control: true,
                clipboard: true,
                transfer: true,
            })
            .unwrap();
        }
        let session = controller.connect(&host.endpoint_id()).unwrap();
        let hosted = host.snapshot().unwrap().incoming.unwrap();
        let offer = crate::clipboard::FileSelection {
            entries: vec![crate::clipboard::ClipboardFileEntry {
                path: "a.txt".into(),
                size: 5,
                dir: false,
            }],
        };
        controller
            .submit_clipboard_files(session.id, session.generation, &offer)
            .unwrap();
        let host = std::sync::Arc::new(host);
        let controller = std::sync::Arc::new(controller);
        let prepare_host = std::sync::Arc::clone(&host);
        let prepare = std::thread::spawn(move || prepare_host.prepare_incoming_paste(hosted.id, 1));
        entered_rx
            .recv_timeout(std::time::Duration::from_secs(8))
            .expect("worker never reached the resolver");
        // A second concurrent prepare for the same session side is Busy.
        // The direct call runs inside a runtime: the reservation check
        // must answer Busy, not fail on missing context.
        let peer_key = lock(&controller.shared.state)
            .outgoing
            .get(&session.id)
            .unwrap()
            .peer_key;
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let busy = runtime.block_on(async {
            spawn_paste_preparation(
                &controller.shared,
                false,
                session.id,
                peer_key,
                11,
                1,
                &offer,
            )
        });
        assert!(matches!(busy, Err(crate::Error::Busy)), "{busy:?}");
        // Close the parent: the parked worker settles without replying.
        controller.disconnect_session(session.id).unwrap();
        let _ = release_tx.send(());
        let outcome = prepare.join().expect("prepare thread panicked");
        assert!(
            matches!(outcome, Err(crate::Error::NoSession)),
            "expected session loss, got {outcome:?}"
        );
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(8);
        while !lock(&controller.shared.paste_preparing).is_empty() {
            assert!(
                std::time::Instant::now() < deadline,
                "preparation guard leaked after session loss"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        host.shutdown();
        controller.shutdown();
        std::fs::remove_dir_all(&root).unwrap();
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
            assert!(
                matches!(
                    spawn_paste_preparation(&core.shared, false, 7, [9; 32], 11, 1, &bad),
                    Err(crate::Error::Malformed)
                ),
                "{path} spawned"
            );
        }
        // Validation precedes reservation: nothing leaked into the guard.
        assert!(lock(&core.shared.paste_preparing).is_empty());
        core.shutdown();
    }
}
