//! Owner-approved file roots and filesystem operations.
//!
//! The `Core` methods here execute against this installation's own disks
//! (the local pane). The `fs_list`/`fs_mkdir`/`fs_rename`/`fs_delete` methods reach the
//! controlled computer instead: each request opens its own short side
//! stream on the live control connection, carrying the versioned wire codec
//! with the session generation, and the host re-checks generation and the
//! transfer grant per request before touching its disks.
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
    /// Enables filesystem browsing within this process's OS permissions.
    /// Files service authorization remains independent and is checked on each
    /// operation. Existing folder-only access can still be explicitly selected.
    pub fn enable_filesystem_access(&self) -> Result<(), Error> {
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
    /// Replaces the owner-approved file roots. Nothing is browsable until
    /// this runs: pairing and transfer grants never widen into filesystem
    /// access on their own. Refused while a controlling session is active,
    /// like the transfer directory.
    pub fn set_file_roots(&self, paths: Vec<std::path::PathBuf>) -> Result<(), Error> {
        let state = self.alive_state()?;
        if state
            .incoming
            .values()
            .any(|session| !finished(&session.phase))
        {
            return Err(Error::Busy);
        }
        drop(state);
        let mut roots = lock(&self.shared.file_roots);
        roots.set(&paths)
    }

    /// Snapshots the approved roots as `(id, path)` pairs.
    pub fn file_roots(&self) -> Result<Vec<(u64, std::path::PathBuf)>, Error> {
        self.ensure_alive()?;
        Ok(lock(&self.shared.file_roots).snapshot())
    }

    /// Pins one approved root for a direct blob offer. Tests mint pins
    /// through this so direct offers share the job legs' contract.
    pub(crate) fn pin_file_root(&self, id: u64) -> Result<crate::fs::PinnedRoot, Error> {
        self.ensure_alive()?;
        lock(&self.shared.file_roots).pin(id)
    }

    /// This computer's roots, home folder and usual folders.
    pub fn places(&self) -> Result<fs::Places, Error> {
        self.ensure_alive()?;
        Ok(fs::places(&current_roots(&self.shared)))
    }

    /// Where `path` lies under the shared roots, as the panes name an
    /// item: its folder's root and path, and its own name. The folder is
    /// resolved and the item is not, so a link is copied as itself rather
    /// than as what it points to. `None` when it is outside every root.
    pub fn locate(&self, path: &std::path::Path) -> Result<Option<(u64, String, String)>, Error> {
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

    /// Lists one page of a directory under an approved root.
    pub fn list_dir(
        &self,
        root: u64,
        path: &str,
        offset: u64,
        limit: u16,
    ) -> Result<DirPage, Error> {
        self.ensure_alive()?;
        // Snapshot under a short lock; enumeration runs off every lock.
        let snapshot = lock(&self.shared.file_roots).clone();
        fs::list_page(&snapshot, root, path, offset, limit)
    }

    /// Creates one directory level under an approved root.
    pub fn create_dir(&self, root: u64, path: &str, name: &str) -> Result<(), Error> {
        self.ensure_alive()?;
        let roots = lock(&self.shared.file_roots);
        fs::create_dir(&roots, root, path, name)
    }

    /// Renames one entry inside its existing parent folder under an
    /// approved root. `expected` carries the listing token; a strong token
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
        self.ensure_alive()?;
        if super::jobs::rename_blocked(&self.shared, root, path, name) {
            return Err(Error::InvalidSetting(
                "the entry is used by an active transfer",
            ));
        }
        let roots = lock(&self.shared.file_roots);
        fs::rename_entry(&roots, root, path, name, expected, new_name)
    }

    /// Deletes one entry inside its existing parent folder under an
    /// approved root. `expected` carries the listing token; a strong token
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

    /// Stats one entry for test identity proofs without listing.
    #[cfg(test)]
    pub(crate) fn stat_entry(
        &self,
        root: u64,
        path: &str,
        name: &str,
    ) -> Result<crate::fs::FsEntry, Error> {
        self.ensure_alive()?;
        let roots = lock(&self.shared.file_roots);
        let (_parent, full) = roots.resolve_parent(root, path, name)?;
        Ok(fs::stat(&full, name.to_owned()))
    }

    /// Lists a directory on the controlled computer over the filesystem
    /// side stream. The session must be active and the generation current;
    /// the host re-checks both plus the transfer grant per request.
    pub fn fs_list(
        &self,
        session_id: u64,
        generation: u64,
        root: u64,
        path: &str,
        offset: u64,
        limit: u16,
    ) -> Result<DirPage, Error> {
        match self.fs_round_trip(
            session_id,
            generation,
            fs::Request::List {
                id: 1,
                generation,
                root,
                path: path.to_owned(),
                offset,
                limit,
            },
        )? {
            FsOutcome::Page(page) => Ok(page),
            FsOutcome::Done | FsOutcome::Places(_) => Err(Error::Malformed),
        }
    }

    /// Creates one directory level on the controlled computer.
    pub fn fs_mkdir(
        &self,
        session_id: u64,
        generation: u64,
        root: u64,
        path: &str,
        name: &str,
    ) -> Result<(), Error> {
        match self.fs_round_trip(
            session_id,
            generation,
            fs::Request::Mkdir {
                id: 1,
                generation,
                root,
                path: path.to_owned(),
                name: name.to_owned(),
            },
        )? {
            FsOutcome::Done => Ok(()),
            FsOutcome::Page(_) | FsOutcome::Places(_) => Err(Error::Malformed),
        }
    }

    /// Renames one entry on the controlled computer. `expected` carries the
    /// listing token from a remote listing; the host refuses a replaced
    /// source instead of renaming a different object.
    #[allow(clippy::too_many_arguments)]
    pub fn fs_rename(
        &self,
        session_id: u64,
        generation: u64,
        root: u64,
        path: &str,
        name: &str,
        expected: Option<crate::fs::EntryToken>,
        new_name: &str,
    ) -> Result<(), Error> {
        match self.fs_round_trip(
            session_id,
            generation,
            fs::Request::Rename {
                id: 1,
                generation,
                root,
                path: path.to_owned(),
                name: name.to_owned(),
                expected,
                new_name: new_name.to_owned(),
            },
        )? {
            FsOutcome::Done => Ok(()),
            FsOutcome::Page(_) | FsOutcome::Places(_) => Err(Error::Malformed),
        }
    }

    /// Deletes one entry on the controlled computer. `expected` carries the
    /// listing token from a remote listing; the host refuses a replaced
    /// source instead of deleting a different object.
    pub fn fs_delete(
        &self,
        session_id: u64,
        generation: u64,
        root: u64,
        path: &str,
        name: &str,
        expected: Option<crate::fs::EntryToken>,
    ) -> Result<(), Error> {
        match self.fs_round_trip(
            session_id,
            generation,
            fs::Request::Delete {
                id: 1,
                generation,
                root,
                path: path.to_owned(),
                name: name.to_owned(),
                expected,
                trash: false,
            },
        )? {
            FsOutcome::Done => Ok(()),
            FsOutcome::Page(_) | FsOutcome::Places(_) => Err(Error::Malformed),
        }
    }

    fn fs_round_trip(
        &self,
        session_id: u64,
        generation: u64,
        mut request: fs::Request,
    ) -> Result<FsOutcome, Error> {
        let connection = {
            let state = self.alive_state()?;
            let session = state.outgoing.get(&session_id).ok_or(Error::NoSession)?;
            if session.id != session_id || session.phase != crate::session::SessionPhase::Active {
                return Err(Error::NoSession);
            }
            if session.generation != generation {
                return Err(Error::StaleSession);
            }
            match &mut request {
                fs::Request::List { generation, .. }
                | fs::Request::Mkdir { generation, .. }
                | fs::Request::Rename { generation, .. }
                | fs::Request::Delete { generation, .. }
                | fs::Request::Watch { generation, .. }
                | fs::Request::Places { generation, .. } => {
                    *generation = session.wire_stamp.generation
                }
            }
            session.link.clone().ok_or(Error::NoSession)?
        };
        let payload = fs::encode_request(&request)?;
        let mut frame = Vec::with_capacity(1 + payload.len());
        frame.push(crate::job::TAG_FS);
        frame.extend_from_slice(&payload);
        self.runtime.block_on(async {
            let (mut send, mut recv) = connection.open_bi().await.map_err(Error::network_debug)?;
            crate::sidestream::write_frame(
                &mut send,
                &frame,
                |bytes: &Vec<u8>| encode_raw(bytes),
                FS_DID_NOT_WRITE,
            )
            .await?;
            let response_bytes: Vec<u8> = crate::sidestream::read_frame(
                &mut recv,
                |len| {
                    if len > MAX_FS_FRAME {
                        Err(Error::Malformed)
                    } else {
                        Ok(())
                    }
                },
                |bytes| Ok(bytes.to_vec()),
            )
            .await?;
            let response =
                fs::decode_response(&response_bytes).map_err(|error| error.to_error())?;
            if response.id != 1 {
                return Err(Error::Malformed);
            }
            status_error(response.status, &response.message)?;
            match request {
                fs::Request::List { .. } => {
                    let body: Vec<u8> = crate::sidestream::read_frame(
                        &mut recv,
                        |len| {
                            if len > MAX_FS_FRAME {
                                Err(Error::Malformed)
                            } else {
                                Ok(())
                            }
                        },
                        |bytes| Ok(bytes.to_vec()),
                    )
                    .await?;
                    let page = fs::decode_list_body(&body).map_err(|error| error.to_error())?;
                    Ok(FsOutcome::Page(page))
                }
                fs::Request::Places { .. } => {
                    let body: Vec<u8> = crate::sidestream::read_frame(
                        &mut recv,
                        |len| {
                            if len > MAX_FS_FRAME {
                                Err(Error::Malformed)
                            } else {
                                Ok(())
                            }
                        },
                        |bytes| Ok(bytes.to_vec()),
                    )
                    .await?;
                    let places = fs::decode_places_body(&body).map_err(|error| error.to_error())?;
                    Ok(FsOutcome::Places(places))
                }
                _ => Ok(FsOutcome::Done),
            }
        })
    }
}

/// Serves the auxiliary side streams for one hosted session: filesystem
/// requests and job-control requests share one accept loop, told apart by
/// the frame tag. Each request arrives on its own short stream; every
/// request re-checks the session generation and the transfer grant before
/// touching the disk.
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
    // JoinSet drop aborts every owned stream task; service guards settle records.
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
    let neutral = lock(&shared.state)
        .incoming
        .get(&session_id)
        .is_some_and(|s| s.services_only);
    if neutral
        && tag != super::service_stream::TAG_SERVICE
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
            } else if tag == crate::job::TAG_FS {
                crate::ignored!(
                    crate::trace::FILES,
                    "a failed file request ends only itself",
                    handle_fs_request(&shared, session_id, &peer, &mut send, body).await
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
        let neutral = lock(&shared.state)
            .outgoing_peer(&peer)
            .is_some_and(|s| s.services_only);
        if neutral && !super::jobs::neutral_job_control_allowed(&shared, &peer, tag, body) {
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
#[cfg(test)]
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
            #[cfg(test)]
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
/// missing, finished, reborn or foreign; denied when the transfer grant
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
    // Snapshot the approved roots under a short lock; enumeration runs
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
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
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
        let desktop_stop = tokio::sync::watch::channel(false).0;
        let stop = tokio::sync::watch::channel(false).0;
        let session = LiveSession {
            id: session_id,
            wire_stamp: crate::protocol::SessionStamp::new(session_id, generation),
            controller: None,
            generation,
            role: crate::session::SessionRole::Host,
            phase: crate::session::SessionPhase::Active,
            peer_id: String::new(),
            peer_key: STALE_PEER,
            quality: None,
            codec: 0,
            matrix: crate::YuvMatrix::Bt709,
            separate_cursor: false,
            files_only: true,
            services_only: false,
            desktop_stop,
            host_held: Arc::new(Mutex::new(crate::input::HeldInput::default())),
            incoming_slot: None,
            cursor_outbox: tokio::sync::watch::channel(None).0,
            input_epoch: Arc::new(AtomicU64::new(1)),
            desktop_owner: None,
            monitor_width: 0,
            monitor_height: 0,
            peer_topology: Vec::new(),
            selected_displays: Vec::new(),
            display_fault: None,
            stop,
            outbox: None,
            video: None,
            link: None,
            clipboard: None,
            clipboard_files: None,
            published_files: None,
            published_rev: AtomicU64::new(0),
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
        let files = crate::tests::filesystem_location::FilesystemLocation::new(
            host.file_roots().unwrap(),
            &root,
        );
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
            // Revoke the transfer grant while parked, then release: the
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
