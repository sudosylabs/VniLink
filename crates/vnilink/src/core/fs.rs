//! The shared disk and filesystem operations.
//!
//! The `Core` methods here execute against this installation's own disks
//! (the local pane). The other computer's disk is reached through a Files
//! service (`service_stream.rs`), whose requests this host serves here:
//! each carries the versioned wire codec with the link's generation, and
//! the host re-checks generation and Files sharing per request before
//! touching its disks.
// trace: records every ignored failure

use super::*;

use std::sync::Arc;

use crate::error::Error;
use crate::fs::{self, DirPage};
use crate::session::Grant;

/// Largest filesystem frame on the wire: requests stay tiny, list bodies
/// stay bounded by the page limit.
pub(crate) const MAX_FS_FRAME: usize = 256 * 1024;

const FS_DID_NOT_WRITE: &str = "the filesystem frame was not written";

/// Length-prefixes one raw frame. Unlike the clipboard/transfer codecs, the
/// filesystem wire messages carry no framing of their own.
fn encode_raw(bytes: &[u8]) -> Result<Vec<u8>, Error> {
    let mut out = Vec::with_capacity(4 + bytes.len());
    out.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
    out.extend_from_slice(bytes);
    Ok(out)
}

/// Outcome of one remote filesystem request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FsOutcome {
    Page(DirPage),
    Places(fs::Places),
    Done,
}

impl Core {
    /// Shares the whole disk, within this process's OS permissions: `/` on
    /// Unix and every drive on Windows. Nothing is browsable until this
    /// runs, and Files sharing with the peer (the `Transfer` grant) is still
    /// checked on each operation.
    /// Refused while a controlling session is active, like the transfer
    /// directory.
    pub fn enable_filesystem_access(&self) -> Result<(), Error> {
        let _engine = self.engine();
        let state = self.alive_state()?;
        if state
            .incoming
            .values()
            .any(|session| !finished(&session.phase))
        {
            return Err(Error::Busy);
        }
        drop(state);
        lock(&self.shared.file_roots).set_filesystem()
    }

    /// Test probe: shares `paths` as this computer's drives in place of
    /// the whole disk, as the drives are read again when one comes, goes or
    /// another volume takes a letter. A test takes a folder standing in for
    /// a drive out, or puts another at its path, and calls this again; a
    /// transfer pinned to the drive it began on then fails or resumes as
    /// it would on Windows. Unlike sharing the disk, it is not refused
    /// during a session: drives change whenever they change.
    #[cfg(feature = "test-probes")]
    pub fn share_as_drives(&self, paths: Vec<std::path::PathBuf>) -> Result<(), Error> {
        let _engine = self.engine();
        self.ensure_alive()?;
        lock(&self.shared.file_roots).share_as_drives(&paths)
    }

    /// Snapshots the shared roots as `(id, path)` pairs: `/`, or each drive.
    pub fn file_roots(&self) -> Result<Vec<(u64, std::path::PathBuf)>, Error> {
        let _engine = self.engine();
        self.ensure_alive()?;
        Ok(lock(&self.shared.file_roots).snapshot())
    }

    /// This computer's roots, home folder and usual folders.
    pub fn places(&self) -> Result<fs::Places, Error> {
        let _engine = self.engine();
        self.ensure_alive()?;
        Ok(fs::places(&current_roots(&self.shared)))
    }

    /// Where `path` lies under the shared roots, as the panes name an
    /// item: its folder's root and path, and its own name. The folder is
    /// resolved and the item is not, so a link is copied as itself rather
    /// than as what it points to. `None` when it is outside every root.
    pub fn locate(&self, path: &std::path::Path) -> Result<Option<(u64, String, String)>, Error> {
        let _engine = self.engine();
        self.ensure_alive()?;
        let (Some(folder), Some(name)) = (
            path.parent(),
            path.file_name().and_then(|name| name.to_str()),
        ) else {
            return Ok(None);
        };
        if fs::check_name(name).is_err() {
            return Ok(None);
        }
        let roots = lock(&self.shared.file_roots).clone();
        Ok(roots
            .locate(folder)
            .map(|(root, rel)| (root, rel, name.to_owned())))
    }

    /// Lists one page of a directory under a shared root.
    pub fn list_dir(
        &self,
        root: u64,
        path: &str,
        offset: u64,
        limit: u16,
    ) -> Result<DirPage, Error> {
        let _engine = self.engine();
        self.ensure_alive()?;
        // Snapshot under a short lock; enumeration runs off every lock.
        let snapshot = lock(&self.shared.file_roots).clone();
        fs::list_page(&snapshot, root, path, offset, limit)
    }

    /// Creates one directory level under a shared root.
    pub fn create_dir(&self, root: u64, path: &str, name: &str) -> Result<(), Error> {
        let _engine = self.engine();
        self.ensure_alive()?;
        let roots = lock(&self.shared.file_roots);
        fs::create_dir(&roots, root, path, name)
    }

    /// Renames one entry inside its existing parent folder under a
    /// shared root. `expected` carries the listing token; a strong token
    /// that no longer names the same object refuses the mutation instead of
    /// renaming a replacement. Never copies, moves across folders, or
    /// overwrites. Renaming an entry used by an active transfer is refused
    /// so publication cannot strand or recreate the old path.
    pub fn rename_entry(
        &self,
        root: u64,
        path: &str,
        name: &str,
        expected: Option<crate::fs::EntryToken>,
        new_name: &str,
    ) -> Result<(), Error> {
        let _engine = self.engine();
        self.ensure_alive()?;
        if super::jobs::rename_blocked(&self.shared, root, path, name) {
            return Err(Error::InvalidSetting(
                "the entry is used by an active transfer",
            ));
        }
        let roots = lock(&self.shared.file_roots);
        fs::rename_entry(&roots, root, path, name, expected, new_name)
    }

    /// Deletes one entry inside its existing parent folder under a
    /// shared root. `expected` carries the listing token; a strong token
    /// that no longer names the same object refuses the mutation. A file is
    /// unlinked in place. A folder is renamed aside and then removed without
    /// following links, after this method drops the roots lock. Deleting an
    /// entry used by an active transfer is refused.
    pub fn delete_entry(
        &self,
        root: u64,
        path: &str,
        name: &str,
        expected: Option<crate::fs::EntryToken>,
    ) -> Result<(), Error> {
        let _engine = self.engine();
        self.ensure_alive()?;
        if super::jobs::rename_blocked(&self.shared, root, path, name) {
            return Err(Error::InvalidSetting(
                "the entry is used by an active transfer",
            ));
        }
        let work = {
            let roots = lock(&self.shared.file_roots);
            fs::stage_delete(&roots, root, path, name, expected)?
        };
        fs::finish_delete(work)
    }

    /// Moves one entry to this computer's Trash, checked as `delete_entry`
    /// checks it. A place with no Trash refuses with `Unsupported`, so the
    /// caller can offer to delete it permanently instead.
    pub fn trash_entry(
        &self,
        root: u64,
        path: &str,
        name: &str,
        expected: Option<crate::fs::EntryToken>,
    ) -> Result<(), Error> {
        let _engine = self.engine();
        self.ensure_alive()?;
        if super::jobs::rename_blocked(&self.shared, root, path, name) {
            return Err(Error::InvalidSetting(
                "the entry is used by an active transfer",
            ));
        }
        let target = {
            let roots = lock(&self.shared.file_roots);
            fs::stage_trash(&roots, root, path, name, expected)?
        };
        fs::finish_trash(target)
    }
}

/// Serves the side streams of one hosted link: services, Desktop streams
/// and job control share one accept loop, told apart by the frame tag.
/// Each request arrives on its own stream; every request re-checks the
/// session generation and its service's permission before touching the
/// disk.
pub(super) async fn serve_aux_host(
    shared: Arc<Shared>,
    connection: iroh::endpoint::Connection,
    session_id: u64,
    peer: [u8; 32],
    mut stop_rx: tokio::sync::watch::Receiver<bool>,
) {
    let mut workers = tokio::task::JoinSet::new();
    loop {
        if *stop_rx.borrow() {
            break;
        }
        // One accept owner; service streams never compete with legacy streams.
        // A retained Files pipe must not block another request's first frame.
        tokio::select! {
            biased;
            _ = stop_rx.changed() => break,
            _ = workers.join_next(), if !workers.is_empty() => {},
            accepted = connection.accept_bi(), if workers.len() < 16 => {
                let Ok((send, recv)) = accepted else { break; };
                let shared = shared.clone();
                let stop = stop_rx.clone();
                workers.spawn(async move {
                    dispatch_aux_host(shared, session_id, peer, send, recv, stop).await;
                });
            }
        }
    }
    // Each stream ends on the stop it watches, telling its peer why; the
    // JoinSet's drop aborts what has not, and service guards settle records.
    crate::ignored!(
        crate::trace::SESSION,
        "streams still running are aborted with the set",
        tokio::time::timeout(super::host::STREAMS_ENDING, async {
            while workers.join_next().await.is_some() {}
        })
        .await
    );
}

async fn dispatch_aux_host(
    shared: Arc<Shared>,
    session_id: u64,
    peer: [u8; 32],
    mut send: iroh::endpoint::SendStream,
    mut recv: iroh::endpoint::RecvStream,
    mut stop: tokio::sync::watch::Receiver<bool>,
) {
    if *stop.borrow() {
        return;
    }
    let payload = tokio::select! {
        biased;
        _ = stop.changed() => return,
        result = tokio::time::timeout(std::time::Duration::from_secs(5), crate::sidestream::read_frame(
            &mut recv,
            |len| if len == 0 || len > MAX_FS_FRAME.max(crate::job::MAX_JOB_FRAME) { Err(Error::Malformed) } else { Ok(()) },
            |bytes| Ok(bytes.to_vec()),
        )) => match result { Ok(Ok(payload)) => payload, _ => return },
    };
    let Some((&tag, body)) = payload.split_first() else {
        return;
    };
    // A link carries services, Desktop streams and the control of jobs a
    // service owns; anything else is dropped unanswered.
    if tag != super::service_stream::TAG_SERVICE
        && tag != super::service_jobs::TAG
        && tag != super::desktop_stream::TAG
        && tag != super::desktop_clipboard::TAG
        && !super::jobs::neutral_job_control_allowed(&shared, &peer, tag, body)
    {
        return;
    }
    if tag == super::desktop_stream::TAG {
        crate::ignored!(
            crate::trace::STREAM,
            "a failed desktop stream ends only itself",
            super::desktop_stream::serve(shared, session_id, peer, send, recv, body, stop).await
        );
    } else if tag == super::desktop_clipboard::TAG {
        crate::ignored!(
            crate::trace::CLIPBOARD,
            "a failed clipboard pipe ends only itself",
            super::desktop_clipboard::serve(shared, session_id, peer, send, recv, body).await
        );
    } else if tag == super::service_stream::TAG_SERVICE {
        crate::ignored!(
            crate::trace::SERVICE,
            "a failed service stream ends only itself",
            super::service_stream::serve(shared, session_id, peer, send, recv, body, stop).await
        );
    } else {
        let work = async {
            if tag == super::service_jobs::TAG {
                crate::ignored!(
                    crate::trace::FILES,
                    "a failed Files service request ends only itself",
                    super::service_jobs::serve(&shared, session_id, &peer, &mut send, body).await
                );
            } else {
                crate::ignored!(
                    crate::trace::FILES,
                    "a failed transfer request ends only itself",
                    super::jobs::handle_job_request(
                        &shared, session_id, &peer, &mut send, body, tag,
                    )
                    .await
                );
            }
        };
        tokio::select! {
            biased;
            _ = stop.changed() => {},
            _ = work => {},
        }
    }
}

/// Serves host-initiated job streams on the controller: only status
/// queries arrive here, answered from this side's records.
pub(super) async fn serve_aux_controller(
    shared: Arc<Shared>,
    connection: iroh::endpoint::Connection,
    peer: [u8; 32],
    mut stop_rx: tokio::sync::watch::Receiver<bool>,
) {
    loop {
        if *stop_rx.borrow() {
            break;
        }
        let accepted = tokio::select! {
            result = connection.accept_bi() => result,
            _ = stop_rx.changed() => break,
        };
        let Ok((mut send, mut recv)) = accepted else {
            break;
        };
        let payload: Vec<u8> = match crate::sidestream::read_frame(
            &mut recv,
            |len| {
                if len > crate::job::MAX_JOB_FRAME {
                    Err(Error::Malformed)
                } else {
                    Ok(())
                }
            },
            |bytes| Ok(bytes.to_vec()),
        )
        .await
        {
            Ok(payload) => payload,
            Err(_) => continue,
        };
        let (tag, body) = match payload.split_first() {
            Some((tag, body)) => (*tag, body),
            None => continue,
        };
        if !super::jobs::neutral_job_control_allowed(&shared, &peer, tag, body) {
            continue;
        }
        if super::jobs::handle_controller_job(&shared, &peer, &mut send, body, tag)
            .await
            .is_err()
        {
            continue;
        }
    }
}

/// Holds back each listing served to one controller, in milliseconds,
/// so a test can play a slow host without slowing the others.
#[cfg(any(test, feature = "test-probes"))]
impl Core {
    /// Holds back each listing this host serves `peer` by `millis`, or
    /// none for `None`, so a test plays a slow host to one controller.
    pub fn slow_listings_to(&self, peer: &str, millis: Option<u64>) -> Result<(), Error> {
        let _engine = self.engine();
        let peer = super::parse_endpoint_id(peer)?;
        *lock(&LIST_DELAY) = millis.map(|millis| (peer, millis));
        Ok(())
    }
}

#[cfg(any(test, feature = "test-probes"))]
pub(crate) static LIST_DELAY: Mutex<Option<([u8; 32], u64)>> = Mutex::new(None);

pub(super) async fn handle_fs_request(
    shared: &Shared,
    session_id: u64,
    peer: &[u8; 32],
    send: &mut iroh::endpoint::SendStream,
    payload: &[u8],
) -> Result<(), ()> {
    async fn reply(
        send: &mut iroh::endpoint::SendStream,
        response: fs::Response,
        body: Option<Vec<u8>>,
    ) -> Result<(), ()> {
        let header = fs::encode_response(&response);
        crate::sidestream::write_frame(
            send,
            &header,
            |bytes: &Vec<u8>| encode_raw(bytes),
            FS_DID_NOT_WRITE,
        )
        .await
        .map_err(|_| ())?;
        if let Some(body) = body {
            crate::sidestream::write_frame(
                send,
                &body,
                |bytes: &Vec<u8>| encode_raw(bytes),
                FS_DID_NOT_WRITE,
            )
            .await
            .map_err(|_| ())?;
        }
        Ok(())
    }
    let payload: Vec<u8> = payload.to_vec();
    let request = fs::decode_request(&payload).map_err(|_| ())?;
    let started = std::time::Instant::now();
    let operation = match &request {
        fs::Request::List { .. } => "list",
        fs::Request::Mkdir { .. } => "create folder",
        fs::Request::Rename { .. } => "rename",
        fs::Request::Delete { .. } => "delete",
        fs::Request::Watch { .. } => "watch",
        fs::Request::Places { .. } => "places",
    };
    let (id, generation) = match &request {
        fs::Request::List { id, generation, .. }
        | fs::Request::Mkdir { id, generation, .. }
        | fs::Request::Rename { id, generation, .. }
        | fs::Request::Delete { id, generation, .. }
        | fs::Request::Watch { id, generation, .. }
        | fs::Request::Places { id, generation } => (*id, *generation),
    };
    // Fence generation + authorization with the mutation under one state
    // hold (jobs->state order for renames, consistent with publication):
    // a revoke completing before the mutation prevents it; holding across
    // the anchored syscalls (no awaits) blocks revoke until after.
    // Listings instead snapshot under short locks and recheck after the
    // worker await, so slow storage never parks the handler's locks.
    // Reply awaits happen after all locks drop, so futures stay Send.
    let (response, body): (fs::Response, Option<Vec<u8>>) = match request {
        fs::Request::Delete {
            root,
            path,
            name,
            expected,
            trash,
            ..
        } => {
            // Same admission as rename, but the tree walk happens after the
            // guards drop. A folder is only renamed aside while jobs, state
            // and roots are held; unlinking its children must not park them.
            let staged = {
                let jobs_guard = lock(&shared.jobs);
                let state_guard = lock(&shared.state);
                let live = matches!(state_guard.incoming.get(&session_id), Some(session)
                    if session.generation == generation
                        && session.phase == crate::session::SessionPhase::Active
                        && session.peer_key == *peer);
                if !live {
                    Err(fs::Response::coded(id, fs::STATUS_STALE))
                } else if !state_guard.trust.granted(peer, Grant::Transfer) {
                    Err(fs::Response::coded(id, fs::STATUS_DENIED))
                } else if super::jobs::rename_blocked_with(&jobs_guard, root, &path, &name) {
                    Err(fs::Response::coded(id, fs::STATUS_ERROR))
                } else {
                    let roots = lock(&shared.file_roots);
                    let outcome = if trash {
                        fs::stage_trash(&roots, root, &path, &name, expected).map(Staged::Trash)
                    } else {
                        fs::stage_delete(&roots, root, &path, &name, expected).map(Staged::Delete)
                    };
                    drop(roots);
                    outcome.map_err(|error| map_fs_error(id, &error))
                }
            };
            match staged {
                Err(response) => (response, None),
                Ok(work) => {
                    match tokio::task::spawn_blocking(move || match work {
                        Staged::Delete(work) => fs::finish_delete(work),
                        Staged::Trash(path) => fs::finish_trash(path),
                    })
                    .await
                    {
                        Ok(Ok(())) => (fs::Response::ok(id), None),
                        Ok(Err(error)) => (map_fs_error(id, &error), None),
                        Err(_) => (fs::Response::coded(id, fs::STATUS_ERROR), None),
                    }
                }
            }
        }
        fs::Request::Rename {
            root,
            path,
            name,
            expected,
            new_name,
            ..
        } => {
            // Jobs->state order (consistent with publication): hold both across
            // generation+grant+active-job checks and the anchored mutation.
            // No pre-check via helper (which would lock, drop, then re-lock with
            // a race); scan under the held jobs guard instead (no nesting).
            {
                let jobs_guard = lock(&shared.jobs);
                let state_guard = lock(&shared.state);
                let live = matches!(state_guard.incoming.get(&session_id), Some(session)
                    if session.generation == generation
                        && session.phase == crate::session::SessionPhase::Active
                        && session.peer_key == *peer);
                if !live {
                    (fs::Response::coded(id, fs::STATUS_STALE), None)
                } else if !state_guard.trust.granted(peer, Grant::Transfer) {
                    (fs::Response::coded(id, fs::STATUS_DENIED), None)
                } else if super::jobs::rename_blocked_with(&jobs_guard, root, &path, &name) {
                    (fs::Response::coded(id, fs::STATUS_ERROR), None)
                } else {
                    // Anchored mutation while holding jobs+state; roots acquired
                    // nested jobs->state->roots (publication drops roots before
                    // jobs->state, so no inversion). No awaits while holding.
                    let roots = lock(&shared.file_roots);
                    let outcome = fs::rename_entry(&roots, root, &path, &name, expected, &new_name);
                    drop(roots);
                    match outcome {
                        Ok(()) => (fs::Response::ok(id), None),
                        Err(error) => (map_fs_error(id, &error), None),
                    }
                }
            }
        }
        fs::Request::List {
            root,
            path,
            offset,
            limit,
            ..
        } => {
            #[cfg(any(test, feature = "test-probes"))]
            {
                let delay = *lock(&LIST_DELAY);
                if let Some((_, millis)) = delay.filter(|(slow, _)| slow == peer) {
                    tokio::time::sleep(std::time::Duration::from_millis(millis)).await;
                }
            }
            serve_list_page(
                shared,
                session_id,
                peer,
                generation,
                id,
                root,
                &path,
                offset,
                limit,
                fs::ListHooks::none(),
            )
            .await
        }
        // A wait belongs to an open Files service: see `service_stream`.
        fs::Request::Watch { .. } => (fs::Response::coded(id, fs::STATUS_ERROR), None),
        fs::Request::Places { .. } => {
            // Admitted like a listing: it names folders, never their contents.
            let gate = list_gate(&lock(&shared.state), session_id, peer, generation);
            match gate {
                Some(status) => (fs::Response::coded(id, status), None),
                None => {
                    let snapshot = current_roots(shared);
                    match tokio::task::spawn_blocking(move || fs::places(&snapshot)).await {
                        Ok(places) => match fs::encode_places_body(&places) {
                            Ok(body) => (fs::Response::ok(id), Some(body)),
                            Err(_) => (fs::Response::coded(id, fs::STATUS_ERROR), None),
                        },
                        Err(_) => (fs::Response::coded(id, fs::STATUS_ERROR), None),
                    }
                }
            }
        }
        fs::Request::Mkdir {
            root, path, name, ..
        } => {
            // Mkdir fenced like list: state held across the anchored commit.
            let state_guard = lock(&shared.state);
            let live = matches!(state_guard.incoming.get(&session_id), Some(session)
                if session.generation == generation
                    && session.phase == crate::session::SessionPhase::Active
                    && session.peer_key == *peer);
            if !live {
                (fs::Response::coded(id, fs::STATUS_STALE), None)
            } else if !state_guard.trust.granted(peer, Grant::Transfer) {
                (fs::Response::coded(id, fs::STATUS_DENIED), None)
            } else {
                let roots = lock(&shared.file_roots);
                match fs::create_dir(&roots, root, &path, &name) {
                    Ok(()) => (fs::Response::ok(id), None),
                    Err(error) => (map_fs_error(id, &error), None),
                }
            }
        }
    };
    // The operation and how it went, never the names it touched.
    tracing::debug!(
        target: crate::trace::FILES,
        operation,
        status = response.status,
        body_bytes = body.as_ref().map_or(0, Vec::len),
        took_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
        "file request answered"
    );
    reply(send, response, body).await?;
    Ok(())
}

/// Admission gate for one remote listing: stale when the session is
/// missing, finished, reborn or foreign; denied when Files sharing
/// is missing. `None` permits the listing.
fn list_gate(
    state: &super::State,
    session_id: u64,
    peer: &[u8; 32],
    generation: u64,
) -> Option<u8> {
    let live = matches!(state.incoming.get(&session_id), Some(session)
        if session.generation == generation
            && session.phase == crate::session::SessionPhase::Active
            && session.peer_key == *peer);
    if !live {
        Some(fs::STATUS_STALE)
    } else if !state.trust.granted(peer, Grant::Transfer) {
        Some(fs::STATUS_DENIED)
    } else {
        None
    }
}

/// Serves one remote listing: admission under short locks, enumeration
/// on a bounded worker, then an exact liveness/grant recheck before the
/// page is encoded. A stale completion never publishes its page.
async fn serve_list_page(
    shared: &Shared,
    session_id: u64,
    peer: &[u8; 32],
    generation: u64,
    id: u32,
    root: u64,
    path: &str,
    offset: u64,
    limit: u16,
    hooks: fs::ListHooks,
) -> (fs::Response, Option<Vec<u8>>) {
    // Fast gates first, without holding anything across an await.
    if let Some(status) = list_gate(&lock(&shared.state), session_id, peer, generation) {
        return (fs::Response::coded(id, status), None);
    }
    // Snapshot the shared roots under a short lock; enumeration runs
    // on a bounded worker with no locks held.
    let snapshot = lock(&shared.file_roots).clone();
    let page =
        match fs::list_page_async(snapshot, root, path.to_owned(), offset, limit, hooks).await {
            Ok(page) => page,
            Err(error) => return (map_fs_error(id, &error), None),
        };
    // Exact recheck before publishing: a completion that outlived its
    // session, generation or grant never publishes its page.
    if let Some(status) = list_gate(&lock(&shared.state), session_id, peer, generation) {
        return (fs::Response::coded(id, status), None);
    }
    match fs::encode_list_body(&page) {
        Ok(body) => (fs::Response::ok(id), Some(body)),
        Err(_) => (fs::Response::coded(id, fs::STATUS_ERROR), None),
    }
}

pub(super) fn status_error(status: u8, message: &str) -> Result<(), Error> {
    match status {
        0 => Ok(()),
        status if status == fs::STATUS_EXISTS => Err(Error::FileExists),
        status if status == fs::STATUS_DENIED => Err(Error::TransferDenied),
        status if status == fs::STATUS_STALE => Err(Error::StaleSession),
        status if status == fs::STATUS_NO_TRASH => Err(Error::Unsupported(fs::NO_TRASH)),
        status if status == fs::STATUS_UNPROVEN => Err(Error::UnprovenEntry),
        _ => Err(Error::InvalidSetting(fs::failure_message(message))),
    }
}

/// The shared roots as they are now: the drives are read again first, so
/// Places offers one plugged in since VniLink started.
fn current_roots(shared: &Shared) -> fs::RootSet {
    let mut roots = lock(&shared.file_roots);
    crate::ignored!(
        crate::trace::FILES,
        "places keeps the drives it already knew",
        roots.refresh_drives()
    );
    roots.clone()
}

/// A delete ready to run once the handler's locks are dropped.
enum Staged {
    Delete(fs::DeleteWork),
    Trash(std::path::PathBuf),
}

fn map_fs_error(id: u32, error: &Error) -> fs::Response {
    match error {
        Error::Unsupported(message) if *message == fs::NO_TRASH => {
            fs::Response::coded(id, fs::STATUS_NO_TRASH)
        }
        Error::FileExists => fs::Response::coded(id, fs::STATUS_EXISTS),
        Error::UnprovenEntry => fs::Response::coded(id, fs::STATUS_UNPROVEN),
        Error::TransferDenied => fs::Response::coded(id, fs::STATUS_DENIED),
        Error::StaleSession => fs::Response::coded(id, fs::STATUS_STALE),
        Error::InvalidSetting(message)
            if *message == fs::DELETE_FAILED || *message == fs::DELETE_INCOMPLETE =>
        {
            fs::Response {
                id,
                status: fs::STATUS_ERROR,
                message: (*message).to_owned(),
            }
        }
        _ => fs::Response::coded(id, fs::STATUS_ERROR),
    }
}

#[cfg(test)]
mod listing_tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;

    use crate::core::{Core, CoreConfig, Listen};

    static STALE_PARKED: AtomicBool = AtomicBool::new(false);
    static STALE_RELEASED: AtomicBool = AtomicBool::new(false);

    const STALE_PEER: [u8; 32] = [
        0x58, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66,
        0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66,
        0x66, 0x66,
    ];

    fn stale_park_probe(_unit: u64, cancelled: &AtomicBool) {
        if STALE_PARKED.swap(true, Ordering::SeqCst) {
            return;
        }
        while !STALE_RELEASED.load(Ordering::Acquire) && !cancelled.load(Ordering::Acquire) {
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// Forges a live incoming transfer session plus its paired grant, so
    /// the real serve path (not the wire) is exercised without a peer.
    fn forge_transfer_session(host: &Core, session_id: u64, generation: u64) {
        let stop = tokio::sync::watch::channel(false).0;
        let session = LiveSession {
            id: session_id,
            wire_stamp: crate::protocol::SessionStamp::new(session_id, generation),
            generation,
            role: crate::session::SessionRole::Host,
            phase: crate::session::SessionPhase::Active,
            peer_id: String::new(),
            peer_key: STALE_PEER,
            peer_topology: Vec::new(),
            stop,
            link: None,
            remote_availability: None,
        };
        let mut state = lock(&host.shared.state);
        state.trust.sharing.files = true;
        state.incoming.insert(session_id, session);
        state
            .trust
            .pair(crate::trust::Device {
                endpoint_id: STALE_PEER,
                display_id: String::new(),
                relays: Vec::new(),
                direct: Vec::new(),
            })
            .unwrap();
    }

    /// STD08: a listing that outlives its grant never publishes its page,
    /// and a sibling listing completes while the first is parked.
    #[test]
    fn stale_list_completion_does_not_publish() {
        let tag = format!("vnilink-list-stale-{}", std::process::id());
        let root = std::env::temp_dir().join(&tag).join("root");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("alpha.txt"), b"a").unwrap();
        std::fs::write(root.join("beta.txt"), b"beta-bytes").unwrap();

        let host = Core::boot(
            CoreConfig {
                listen: Listen::Loopback,
                ..CoreConfig::default()
            },
            iroh::RelayMode::Disabled,
        )
        .expect("loopback core");
        host.enable_filesystem_access().unwrap();
        let files = crate::test_support::FilesystemLocation::on(&host, &root);
        forge_transfer_session(&host, 41, 7);

        let parked_rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let sibling_rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let hooks = fs::ListHooks {
            entry_probe: Some(stale_park_probe),
            finish_probe: None,
        };
        let (result_tx, result_rx) = std::sync::mpsc::channel();
        struct ReleaseOnDrop;
        impl Drop for ReleaseOnDrop {
            fn drop(&mut self) {
                STALE_RELEASED.store(true, Ordering::Release);
            }
        }
        std::thread::scope(|scope| {
            let _release = ReleaseOnDrop;
            scope.spawn(|| {
                parked_rt.block_on(async {
                    let served = serve_list_page(
                        &host.shared,
                        41,
                        &STALE_PEER,
                        7,
                        9,
                        files.root,
                        &files.path(""),
                        0,
                        50,
                        hooks,
                    )
                    .await;
                    let _ = result_tx.send(served);
                });
            });
            // Wait for the park: the worker reached the pre-access probe.
            let deadline = std::time::Instant::now() + Duration::from_secs(8);
            while !STALE_PARKED.load(Ordering::SeqCst) {
                assert!(
                    std::time::Instant::now() < deadline,
                    "list worker never parked"
                );
                std::thread::sleep(Duration::from_millis(10));
            }
            // Sibling progress while parked: the async path is free and no
            // shared lock is held, so a full second listing completes.
            let (sibling, sibling_body) = sibling_rt.block_on(serve_list_page(
                &host.shared,
                41,
                &STALE_PEER,
                7,
                10,
                files.root,
                &files.path(""),
                0,
                50,
                fs::ListHooks::none(),
            ));
            assert!(sibling.is_ok(), "sibling list refused while parked");
            let page =
                fs::decode_list_body(&sibling_body.expect("sibling body")).expect("sibling decode");
            assert_eq!(page.total, 2, "sibling list changed while parked");
            // Revoke Files sharing while parked, then release: the
            // stale completion must not publish its page.
            host.set_desktop_sharing(crate::session::Grants::default())
                .unwrap();
            STALE_RELEASED.store(true, Ordering::SeqCst);
            let (response, body) = result_rx
                .recv_timeout(Duration::from_secs(5))
                .expect("stale list never completed");
            assert_eq!(response.id, 9);
            assert_eq!(
                response.status,
                fs::STATUS_DENIED,
                "stale list was not refused"
            );
            assert!(body.is_none(), "stale list published its page");
        });
        host.shutdown();
        std::fs::remove_dir_all(std::env::temp_dir().join(&tag)).unwrap();
    }
}
