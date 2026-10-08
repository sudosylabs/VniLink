//! Owner-approved file roots and filesystem operations.
//!
//! The `Core` methods here execute against this installation's own disks
//! (the local pane). The `fs_list`/`fs_mkdir`/`fs_rename` methods reach the
//! controlled computer instead: each request opens its own short side
//! stream on the live control connection, carrying the versioned wire codec
//! with the session generation, and the host re-checks generation and the
//! transfer grant per request before touching its disks.

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
    Done,
}

impl Core {
    /// Replaces the owner-approved file roots. Nothing is browsable until
    /// this runs: pairing and transfer grants never widen into filesystem
    /// access on their own. Refused while a controlling session is active,
    /// like the transfer directory.
    pub fn set_file_roots(&self, paths: Vec<std::path::PathBuf>) -> Result<(), Error> {
        let state = self.alive_state()?;
        if state
            .incoming
            .as_ref()
            .is_some_and(|session| !finished(&session.phase))
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

    /// Lists one page of a directory under an approved root.
    pub fn list_dir(
        &self,
        root: u64,
        path: &str,
        offset: u64,
        limit: u16,
    ) -> Result<DirPage, Error> {
        self.ensure_alive()?;
        let roots = lock(&self.shared.file_roots);
        fs::list_page(&roots, root, path, offset, limit)
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
            FsOutcome::Done => Err(Error::Malformed),
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
            FsOutcome::Page(_) => Err(Error::Malformed),
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
            FsOutcome::Page(_) => Err(Error::Malformed),
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
                | fs::Request::Rename { generation, .. } => {
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
            match response.status {
                0 => {}
                status if status == fs::STATUS_EXISTS => return Err(Error::FileExists),
                status if status == fs::STATUS_DENIED => return Err(Error::TransferDenied),
                status if status == fs::STATUS_STALE => return Err(Error::StaleSession),
                _ => {
                    return Err(Error::InvalidSetting(
                        "the remote filesystem refused the request",
                    ))
                }
            }
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
        .as_ref()
        .is_some_and(|s| s.id == session_id && s.services_only);
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
        let _ =
            super::desktop_stream::serve(shared, session_id, peer, send, recv, body, stop).await;
    } else if tag == super::desktop_clipboard::TAG {
        let _ = super::desktop_clipboard::serve(shared, session_id, peer, send, recv, body).await;
    } else if tag == super::service_stream::TAG_SERVICE {
        let _ =
            super::service_stream::serve(shared, session_id, peer, send, recv, body, stop).await;
    } else {
        let work = async {
            if tag == super::service_jobs::TAG {
                let _ =
                    super::service_jobs::serve(&shared, session_id, &peer, &mut send, body).await;
            } else if tag == crate::job::TAG_FS {
                let _ = handle_fs_request(&shared, session_id, &peer, &mut send, body).await;
            } else {
                let _ = super::jobs::handle_job_request(
                    &shared, session_id, &peer, &mut send, body, tag,
                )
                .await;
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
    let (id, generation) = match &request {
        fs::Request::List { id, generation, .. }
        | fs::Request::Mkdir { id, generation, .. }
        | fs::Request::Rename { id, generation, .. } => (*id, *generation),
    };
    // Fence generation + authorization with the mutation under one state
    // hold (jobs->state order for renames, consistent with publication):
    // a revoke completing before the mutation prevents it; holding across
    // the anchored syscalls (no awaits) blocks revoke until after.
    // Reply awaits happen after all locks drop, so futures stay Send.
    let (response, body): (fs::Response, Option<Vec<u8>>) = match request {
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
                let live = matches!(state_guard.incoming.as_ref(), Some(session)
                    if session.id == session_id
                        && session.generation == generation
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
            // List needs only state (no jobs). Hold state across the anchored
            // read; reply after drops. Roots nested state->roots; publication
            // drops roots before jobs->state, so no inversion.
            let state_guard = lock(&shared.state);
            let live = matches!(state_guard.incoming.as_ref(), Some(session)
                if session.id == session_id
                    && session.generation == generation
                    && session.phase == crate::session::SessionPhase::Active
                    && session.peer_key == *peer);
            if !live {
                (fs::Response::coded(id, fs::STATUS_STALE), None)
            } else if !state_guard.trust.granted(peer, Grant::Transfer) {
                (fs::Response::coded(id, fs::STATUS_DENIED), None)
            } else {
                let roots = lock(&shared.file_roots);
                match fs::list_page(&roots, root, &path, offset, limit) {
                    Ok(page) => match fs::encode_list_body(&page) {
                        Ok(body) => (fs::Response::ok(id), Some(body)),
                        Err(_) => (fs::Response::coded(id, fs::STATUS_ERROR), None),
                    },
                    Err(error) => (map_fs_error(id, &error), None),
                }
            }
        }
        fs::Request::Mkdir {
            root, path, name, ..
        } => {
            // Mkdir fenced like list: state held across the anchored commit.
            let state_guard = lock(&shared.state);
            let live = matches!(state_guard.incoming.as_ref(), Some(session)
                if session.id == session_id
                    && session.generation == generation
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
    reply(send, response, body).await?;
    Ok(())
}

fn map_fs_error(id: u32, error: &Error) -> fs::Response {
    match error {
        Error::FileExists => fs::Response::coded(id, fs::STATUS_EXISTS),
        Error::TransferDenied => fs::Response::coded(id, fs::STATUS_DENIED),
        Error::StaleSession => fs::Response::coded(id, fs::STATUS_STALE),
        _ => fs::Response::coded(id, fs::STATUS_ERROR),
    }
}
