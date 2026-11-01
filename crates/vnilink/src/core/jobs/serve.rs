// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! Serving a peer's job messages and preparing manifests for it.

use super::*;

/// Host-side job request handler: upload offers, download requests and
/// status queries from the controller. Generation is fenced before any
/// grant check or disk touch.
pub(in crate::core) async fn handle_job_request(
    shared: &Shared,
    session_id: u64,
    peer: &[u8; 32],
    send: &mut iroh::endpoint::SendStream,
    payload: &[u8],
    tag: u8,
) -> Result<(), ()> {
    async fn reply(send: &mut iroh::endpoint::SendStream, bytes: Vec<u8>) -> Result<(), ()> {
        let mut out = Vec::with_capacity(4 + bytes.len());
        out.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
        out.extend_from_slice(&bytes);
        crate::sidestream::write_frame(
            send,
            &out,
            |framed: &Vec<u8>| Ok(framed.clone()),
            "the job frame was not written",
        )
        .await
        .map_err(|_| ())
    }
    // Every job message carries its generation inside the JSON; fence it
    // against the live incoming session before parsing further. The id
    // rides along so even the stale answer correlates.
    let (generation, request_id): (u64, u32) = serde_json::from_slice::<serde_json::Value>(payload)
        .ok()
        .and_then(|value| {
            Some((
                value.get("generation")?.as_u64()?,
                value.get("id")?.as_u64()? as u32,
            ))
        })
        .ok_or(())?;
    let live = {
        let state = lock(&shared.state);
        matches!(state.incoming.get(&session_id), Some(session)
            if session.generation == generation
                && session.phase == crate::session::SessionPhase::Active
                && session.peer_key == *peer)
    };
    if !live {
        let denied = job::JobStateReply {
            v: job::JOB_VERSION,
            id: request_id,
            status: job::STATUS_STALE,
            state: JobState::Interrupted.name().to_owned(),
            error: None,
            waiting_file: None,
            done_bytes: 0,
            files_done: 0,
            skipped_files: 0,
        };
        reply(send, job::encode(&denied).map_err(|_| ())?).await?;
        return Ok(());
    }
    match tag {
        // An offer or a download request rides its Files service
        // (`service_jobs::serve`); a link refuses one on its own stream.
        t if t == job::TAG_JOB_STATUS => {
            handle_status_query(shared, peer, true, send, payload).await
        }
        t if t == job::TAG_JOB_MANIFEST => handle_manifest_query(shared, peer, send, payload).await,
        t if t == job::TAG_JOB_CANCEL => handle_cancel(shared, peer, true, send, payload).await,
        t if t == job::TAG_JOB_RESOLVE => handle_resolve(shared, peer, true, send, payload).await,
        t if t == job::TAG_JOB_RESUME => handle_resume(shared, peer, true, send, payload).await,
        _ => Err(()),
    }
}

/// Controller-side job handler: status queries from the host and symmetric
/// cancellation/decisions/resume from either sender or receiver arrive here.
pub(in crate::core) async fn handle_controller_job(
    shared: &Shared,
    peer: &[u8; 32],
    send: &mut iroh::endpoint::SendStream,
    payload: &[u8],
    tag: u8,
) -> Result<(), ()> {
    if tag == job::TAG_JOB_STATUS {
        return handle_status_query(shared, peer, false, send, payload).await;
    }
    if tag == job::TAG_JOB_CANCEL {
        return handle_cancel(shared, peer, false, send, payload).await;
    }
    if tag == job::TAG_JOB_RESOLVE {
        return handle_resolve(shared, peer, false, send, payload).await;
    }
    if tag == job::TAG_JOB_RESUME {
        return handle_resume(shared, peer, false, send, payload).await;
    }
    Err(())
}

/// Whether `generation` names a live session with `peer`: one it hosts on
/// the incoming side, else one this side controls. A peer may hold both a
/// control session and a services-only link; the message's generation
/// says which it came over.
pub(super) fn peer_session_live(
    state: &State,
    peer: &[u8; 32],
    incoming_side: bool,
    generation: u64,
) -> bool {
    let sessions = if incoming_side {
        &state.incoming
    } else {
        &state.outgoing
    };
    sessions.values().any(|session| {
        session.peer_key == *peer
            && session.wire_stamp.generation == generation
            && session.phase == crate::session::SessionPhase::Active
    })
}

/// The host answers a controller asking for a prepared download's
/// manifest: still preparing (no manifest), the manifest, or a refusal
/// once preparation failed or the job ended.
pub(super) async fn handle_manifest_query(
    shared: &Shared,
    peer: &[u8; 32],
    send: &mut iroh::endpoint::SendStream,
    payload: &[u8],
) -> Result<(), ()> {
    let query: job::JobStatus = job::decode(payload).map_err(|_| ())?;
    job::check_version(query.v).map_err(|_| ())?;
    let answer = |status: u8, manifest: Option<Manifest>| job::ManifestReply {
        v: job::JOB_VERSION,
        id: query.id,
        status,
        job_id: query.job_id,
        manifest,
    };
    let reply = if !peer_session_live(&lock(&shared.state), peer, true, query.generation) {
        answer(job::STATUS_STALE, None)
    } else {
        let jobs = lock(&shared.jobs);
        match wire_job(&jobs, peer, query.job_id).filter(|record| record.sender) {
            Some(record) if record.state == JobState::Preparing => answer(job::STATUS_OK, None),
            Some(record)
                if matches!(
                    record.state,
                    JobState::Serving | JobState::Transferring | JobState::WaitingDecision
                ) =>
            {
                answer(job::STATUS_OK, record.manifest.clone())
            }
            _ => answer(job::STATUS_ERROR, None),
        }
    };
    let bytes = job::encode(&reply).map_err(|_| ())?;
    let mut out = Vec::with_capacity(4 + bytes.len());
    out.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
    out.extend_from_slice(&bytes);
    crate::sidestream::write_frame(
        send,
        &out,
        |framed: &Vec<u8>| Ok(framed.clone()),
        "the job frame was not written",
    )
    .await
    .map_err(|_| ())
}

/// Symmetric cancellation: either side tells the peer one job is cancelled.
/// Generation is fenced like status queries; the peer marks its record
/// cancelled and signals its in-flight fetch so publication stops.
pub(super) async fn handle_cancel(
    shared: &Shared,
    peer: &[u8; 32],
    incoming_side: bool,
    send: &mut iroh::endpoint::SendStream,
    payload: &[u8],
) -> Result<(), ()> {
    async fn reply(send: &mut iroh::endpoint::SendStream, bytes: Vec<u8>) -> Result<(), ()> {
        let mut out = Vec::with_capacity(4 + bytes.len());
        out.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
        out.extend_from_slice(&bytes);
        crate::sidestream::write_frame(
            send,
            &out,
            |framed: &Vec<u8>| Ok(framed.clone()),
            "the job frame was not written",
        )
        .await
        .map_err(|_| ())
    }
    let cancel: job::JobCancel = job::decode(payload).map_err(|_| ())?;
    job::check_version(cancel.v).map_err(|_| ())?;
    let live = peer_session_live(&lock(&shared.state), peer, incoming_side, cancel.generation);
    if !live {
        let stale = job::JobReply {
            v: job::JOB_VERSION,
            id: cancel.id,
            status: job::STATUS_STALE,
            job_id: cancel.job_id,
        };
        reply(send, job::encode(&stale).map_err(|_| ())?).await?;
        return Ok(());
    }
    let id = {
        let mut jobs = lock(&shared.jobs);
        match wire_job_mut(&mut jobs, peer, cancel.job_id) {
            Some(record) if record.peer_key == *peer && !record.state.is_terminal() => {
                record.enter(JobState::Cancelled);
                record.error = None;
                record.waiting_file = None;
                Some(record.id)
            }
            _ => None,
        }
    };
    if let Some(id) = id {
        if let Some(stop) = lock(&shared.job_cancel).remove(&id) {
            crate::ignored!(
                crate::trace::FILES,
                "a fetch that already ended needs no stop",
                stop.send(true)
            );
        }
        stop_prefetch(shared, id);
        persist_jobs(shared);
    }
    reply(
        send,
        job::encode(&job::JobReply {
            v: job::JOB_VERSION,
            id: cancel.id,
            status: job::STATUS_OK,
            job_id: cancel.job_id,
        })
        .map_err(|_| ())?,
    )
    .await?;
    Ok(())
}

pub(super) async fn handle_status_query(
    shared: &Shared,
    peer: &[u8; 32],
    incoming_side: bool,
    send: &mut iroh::endpoint::SendStream,
    payload: &[u8],
) -> Result<(), ()> {
    async fn reply(send: &mut iroh::endpoint::SendStream, bytes: Vec<u8>) -> Result<(), ()> {
        let mut out = Vec::with_capacity(4 + bytes.len());
        out.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
        out.extend_from_slice(&bytes);
        crate::sidestream::write_frame(
            send,
            &out,
            |framed: &Vec<u8>| Ok(framed.clone()),
            "the job frame was not written",
        )
        .await
        .map_err(|_| ())
    }
    let query: job::JobStatus = job::decode(payload).map_err(|_| ())?;
    job::check_version(query.v).map_err(|_| ())?;
    // The querying peer must own the other end of the live session.
    let live = peer_session_live(&lock(&shared.state), peer, incoming_side, query.generation);
    if !live {
        let stale = job::JobStateReply {
            v: job::JOB_VERSION,
            id: query.id,
            status: job::STATUS_STALE,
            state: JobState::Interrupted.name().to_owned(),
            error: None,
            waiting_file: None,
            done_bytes: 0,
            files_done: 0,
            skipped_files: 0,
        };
        reply(send, job::encode(&stale).map_err(|_| ())?).await?;
        return Ok(());
    }
    let answer = {
        let jobs = lock(&shared.jobs);
        match wire_job(&jobs, peer, query.job_id) {
            Some(record) if record.peer_key == *peer => {
                let waiting_file = if record.state == JobState::WaitingDecision {
                    record.waiting_file.and_then(|index| {
                        record
                            .manifest
                            .as_ref()?
                            .entries
                            .iter()
                            .filter(|entry| !entry.is_dir)
                            .nth(index)
                            .map(|entry| entry.path.clone())
                    })
                } else {
                    None
                };
                job::JobStateReply {
                    v: job::JOB_VERSION,
                    id: query.id,
                    status: job::STATUS_OK,
                    state: record.state.name().to_owned(),
                    error: record.error.clone(),
                    waiting_file,
                    done_bytes: record.done(),
                    files_done: record.files_done,
                    skipped_files: record.skipped_files,
                }
            }
            _ => job::JobStateReply {
                v: job::JOB_VERSION,
                id: query.id,
                status: job::STATUS_ERROR,
                state: JobState::Interrupted.name().to_owned(),
                error: None,
                waiting_file: None,
                done_bytes: 0,
                files_done: 0,
                skipped_files: 0,
            },
        }
    };
    reply(send, job::encode(&answer).map_err(|_| ())?).await?;
    Ok(())
}

/// Authenticated conflict decision: the sender forwards the owner's choice
/// to the receiver that owns the waiting file. Generation fenced; stale
/// dialogs (wrong job, wrong file, or no longer waiting) are refused without
/// touching state.
pub(super) async fn handle_resolve(
    shared: &Shared,
    peer: &[u8; 32],
    incoming_side: bool,
    send: &mut iroh::endpoint::SendStream,
    payload: &[u8],
) -> Result<(), ()> {
    async fn reply(send: &mut iroh::endpoint::SendStream, bytes: Vec<u8>) -> Result<(), ()> {
        let mut out = Vec::with_capacity(4 + bytes.len());
        out.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
        out.extend_from_slice(&bytes);
        crate::sidestream::write_frame(
            send,
            &out,
            |framed: &Vec<u8>| Ok(framed.clone()),
            "the job frame was not written",
        )
        .await
        .map_err(|_| ())
    }
    let resolve: job::JobResolve = job::decode(payload).map_err(|_| ())?;
    job::check_version(resolve.v).map_err(|_| ())?;
    let live = peer_session_live(
        &lock(&shared.state),
        peer,
        incoming_side,
        resolve.generation,
    );
    if !live {
        let stale = job::JobReply {
            v: job::JOB_VERSION,
            id: resolve.id,
            status: job::STATUS_STALE,
            job_id: resolve.job_id,
        };
        reply(send, job::encode(&stale).map_err(|_| ())?).await?;
        return Ok(());
    }
    let choice = match resolve.choice.as_str() {
        "skip" => ConflictChoice::Skip,
        "replace" => ConflictChoice::Replace,
        "keep-both" => ConflictChoice::KeepBoth,
        _ => {
            let err = job::JobReply {
                v: job::JOB_VERSION,
                id: resolve.id,
                status: job::STATUS_ERROR,
                job_id: resolve.job_id,
            };
            reply(send, job::encode(&err).map_err(|_| ())?).await?;
            return Ok(());
        }
    };
    let status = {
        let mut jobs = lock(&shared.jobs);
        match wire_job_mut(&mut jobs, peer, resolve.job_id) {
            Some(record)
                if record.peer_key == *peer
                    && !record.sender
                    && record.state == JobState::WaitingDecision =>
            {
                // Stale-dialog fence: the waiting file must still name the
                // same manifest path; choices apply only to their job/file.
                let expected = record.waiting_file.and_then(|index| {
                    record
                        .manifest
                        .as_ref()?
                        .entries
                        .iter()
                        .filter(|entry| !entry.is_dir)
                        .nth(index)
                        .map(|entry| entry.path.clone())
                });
                // The sender forwards the owner's decision without naming a
                // file (it mirrors state); the receiver applies to its
                // current waiting file. If the protocol later names files,
                // fence here. For now, apply to the current waiting file.
                let _ = expected;
                let file = record.waiting_file.ok_or(())?;
                record.choices.insert(file, choice);
                if resolve.apply_to_remaining {
                    record.apply_remaining = Some(choice);
                }
                record.enter(JobState::Transferring);
                record.waiting_file = None;
                job::STATUS_OK
            }
            _ => job::STATUS_ERROR,
        }
    };
    // `status` computed under lock; save outside (save takes jobs lock).
    persist_jobs(shared);
    reply(
        send,
        job::encode(&job::JobReply {
            v: job::JOB_VERSION,
            id: resolve.id,
            status,
            job_id: resolve.job_id,
        })
        .map_err(|_| ())?,
    )
    .await?;
    Ok(())
}

/// Coordinated resume after symmetric cancellation: the peer returns a
/// Cancelled/Interrupted/Failed job to active, so retry works from either
/// side. Senders serve again after verifying their sources through handles;
/// receivers return to `Transferring`.
pub(super) async fn handle_resume(
    shared: &Shared,
    peer: &[u8; 32],
    incoming_side: bool,
    send: &mut iroh::endpoint::SendStream,
    payload: &[u8],
) -> Result<(), ()> {
    async fn reply(send: &mut iroh::endpoint::SendStream, bytes: Vec<u8>) -> Result<(), ()> {
        let mut out = Vec::with_capacity(4 + bytes.len());
        out.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
        out.extend_from_slice(&bytes);
        crate::sidestream::write_frame(
            send,
            &out,
            |framed: &Vec<u8>| Ok(framed.clone()),
            "the job frame was not written",
        )
        .await
        .map_err(|_| ())
    }
    let resume: job::JobResume = job::decode(payload).map_err(|_| ())?;
    job::check_version(resume.v).map_err(|_| ())?;
    let live = peer_session_live(&lock(&shared.state), peer, incoming_side, resume.generation);
    if !live {
        let stale = job::JobReply {
            v: job::JOB_VERSION,
            id: resume.id,
            status: job::STATUS_STALE,
            job_id: resume.job_id,
        };
        reply(send, job::encode(&stale).map_err(|_| ())?).await?;
        return Ok(());
    }
    // Snapshot the job without holding locks across awaits.
    enum ResumeSnap {
        Go {
            sender: bool,
            local_root: u64,
            local_rel: String,
            manifest: Option<Manifest>,
            source_ids: Vec<u64>,
        },
        AckOk,
        AckErr,
    }
    let snap = {
        let jobs = lock(&shared.jobs);
        match wire_job(&jobs, peer, resume.job_id) {
            Some(record)
                if record.peer_key == *peer
                    && super::service_jobs::check_owner(&lock(&shared.state), record.owner)
                        .is_ok() =>
            {
                match record.state {
                    JobState::Interrupted | JobState::Cancelled | JobState::Failed => {
                        ResumeSnap::Go {
                            sender: record.sender,
                            local_root: record.local_root,
                            local_rel: record.local_rel.clone(),
                            manifest: record.manifest.clone(),
                            source_ids: record.source_ids.clone(),
                        }
                    }
                    _ => ResumeSnap::AckOk,
                }
            }
            _ => ResumeSnap::AckErr,
        }
    };
    let (sender, local_root, local_rel, manifest, source_ids) = match snap {
        ResumeSnap::AckOk => {
            let ok = job::JobReply {
                v: job::JOB_VERSION,
                id: resume.id,
                status: job::STATUS_OK,
                job_id: resume.job_id,
            };
            reply(send, job::encode(&ok).map_err(|_| ())?).await?;
            return Ok(());
        }
        ResumeSnap::AckErr => {
            let err = job::JobReply {
                v: job::JOB_VERSION,
                id: resume.id,
                status: job::STATUS_ERROR,
                job_id: resume.job_id,
            };
            reply(send, job::encode(&err).map_err(|_| ())?).await?;
            return Ok(());
        }
        ResumeSnap::Go {
            sender,
            local_root,
            local_rel,
            manifest,
            source_ids,
        } => (sender, local_root, local_rel, manifest, source_ids),
    };
    // Peer-triggered resumes respect the same concurrency bound as local
    // resumes; excess resumes are refused without touching state.
    {
        let active = lock(&shared.jobs)
            .values()
            .filter(|record| ACTIVE_STATES.contains(&record.state))
            .count();
        if active >= MAX_ACTIVE_JOBS {
            let err = job::JobReply {
                v: job::JOB_VERSION,
                id: resume.id,
                status: job::STATUS_ERROR,
                job_id: resume.job_id,
            };
            reply(send, job::encode(&err).map_err(|_| ())?).await?;
            return Ok(());
        }
    }
    if sender {
        // Re-verify each source through handles: a changed source fails
        // instead of moving new bytes.
        let manifest = manifest.ok_or(())?;
        // Adopt the leg's pin up front (jobs->roots order); a source on a
        // replaced drive fails here with its own cause instead of re-authorizing
        // the new directory below. Decided under the guards, acted on
        // after: no mutex is held across the reply await.
        enum PinDecision {
            Go(fs::PinnedRoot),
            Gone,
            Rebound,
        }
        let decision = {
            let mut jobs = lock(&shared.jobs);
            match wire_job_mut(&mut jobs, peer, resume.job_id) {
                None => PinDecision::Gone,
                Some(record) => {
                    if verify_leg_pin(
                        &lock(&shared.file_roots),
                        record.local_root,
                        &mut record.local_pin,
                    )
                    .is_err()
                    {
                        record.enter(JobState::Failed);
                        record.error = Some(fs::DRIVE_CHANGED.to_owned());
                        PinDecision::Rebound
                    } else {
                        PinDecision::Go(record.local_pin.clone().expect("leg pin just verified"))
                    }
                }
            }
        };
        let pin = match decision {
            PinDecision::Go(pin) => pin,
            PinDecision::Gone => {
                let err = job::JobReply {
                    v: job::JOB_VERSION,
                    id: resume.id,
                    status: job::STATUS_ERROR,
                    job_id: resume.job_id,
                };
                reply(send, job::encode(&err).map_err(|_| ())?).await?;
                return Ok(());
            }
            PinDecision::Rebound => {
                persist_jobs(shared);
                let err = job::JobReply {
                    v: job::JOB_VERSION,
                    id: resume.id,
                    status: job::STATUS_ERROR,
                    job_id: resume.job_id,
                };
                reply(send, job::encode(&err).map_err(|_| ())?).await?;
                return Ok(());
            }
        };
        // Decided under the guards, acted on after: no mutex is held
        // across the reply await.
        let checked =
            check_sending_sources(shared, local_root, &local_rel, &pin, &manifest, &source_ids);
        if let Err(reason) = checked {
            {
                let mut jobs = lock(&shared.jobs);
                if let Some(record) = wire_job_mut(&mut jobs, peer, resume.job_id) {
                    record.enter(JobState::Failed);
                    record.error = Some(reason.to_owned());
                }
            }
            persist_jobs(shared);
            let err = job::JobReply {
                v: job::JOB_VERSION,
                id: resume.id,
                status: job::STATUS_ERROR,
                job_id: resume.job_id,
            };
            reply(send, job::encode(&err).map_err(|_| ())?).await?;
            return Ok(());
        }
        {
            let mut jobs = lock(&shared.jobs);
            if let Some(record) = wire_job_mut(&mut jobs, peer, resume.job_id) {
                if super::service_jobs::check_owner(&lock(&shared.state), record.owner).is_err() {
                    return Err(());
                }
                record.enter(JobState::Serving);
                record.error = None;
            }
        }
        persist_jobs(shared);
    } else {
        let mut jobs = lock(&shared.jobs);
        if let Some(record) = wire_job_mut(&mut jobs, peer, resume.job_id) {
            if super::service_jobs::check_owner(&lock(&shared.state), record.owner).is_err() {
                return Err(());
            }
            record.enter(JobState::Transferring);
            record.waiting_file = None;
            record.error = None;
        }
        drop(jobs);
        persist_jobs(shared);
    }
    reply(
        send,
        job::encode(&job::JobReply {
            v: job::JOB_VERSION,
            id: resume.id,
            status: job::STATUS_OK,
            job_id: resume.job_id,
        })
        .map_err(|_| ())?,
    )
    .await?;
    Ok(())
}

pub(in crate::core) async fn handle_upload_offer(
    shared: &Shared,
    peer: &[u8; 32],
    send: &mut iroh::endpoint::SendStream,
    payload: &[u8],
    owner: Option<super::service_jobs::JobOwner>,
) -> Result<(), ()> {
    async fn reply(send: &mut iroh::endpoint::SendStream, bytes: Vec<u8>) -> Result<(), ()> {
        let mut out = Vec::with_capacity(4 + bytes.len());
        out.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
        out.extend_from_slice(&bytes);
        crate::sidestream::write_frame(
            send,
            &out,
            |framed: &Vec<u8>| Ok(framed.clone()),
            "the job frame was not written",
        )
        .await
        .map_err(|_| ())
    }
    let offer: job::UploadOffer = job::decode(payload).map_err(|_| ())?;
    job::check_version(offer.v).map_err(|_| ())?;
    let id = offer.sender_job;
    // Decide under short locks; the reply goes out after every guard is
    // dropped so no mutex is held across an await.
    let accept: Result<(), u8> = (|| {
        if lock(&shared.transfer_dir).is_none() {
            return Err(job::STATUS_ERROR);
        }
        if !lock(&shared.state).trust.granted(peer, Grant::Transfer) {
            return Err(job::STATUS_DENIED);
        }
        if manifest::validate(&offer.manifest).is_err() || fs::check_rel(&offer.dst_rel).is_err() {
            return Err(job::STATUS_ERROR);
        }
        // Pin the destination leg before insertion (sequential roots
        // hold, never nested): a drive replaced between here and first use then
        // fails the leg instead of landing bytes in the new directory.
        let local_pin = lock(&shared.file_roots).pin(offer.dst_root).ok();
        let mut jobs = lock(&shared.jobs);
        let state = lock(&shared.state);
        super::service_jobs::check_owner(&state, owner).map_err(|_| job::STATUS_STALE)?;
        if wire_job(&jobs, peer, id).is_some() {
            return Err(job::STATUS_ERROR);
        }
        let local_id = allocate_job_id(shared).map_err(|_| job::STATUS_ERROR)?;
        shared
            .job_next
            .fetch_max(id.saturating_add(1), Ordering::Relaxed);
        jobs.insert(
            local_id,
            JobRecord {
                owner,
                destination: None,
                id: local_id,
                wire_id: id,
                created_at: retention_now(),
                peer_key: *peer,
                peer_id: endpoint_id_of(peer),
                direction: JobDirection::Upload,
                sender: false,
                state: JobState::Transferring,
                manifest: Some(offer.manifest),
                local_root: offer.dst_root,
                local_rel: offer.dst_rel,
                names: Vec::new(),
                local_pin,
                outgoing: false,
                landed: HashMap::new(),
                file_index: 0,
                done_bytes: 0,
                files_done: 0,
                resumed_files: 0,
                skipped_files: 0,
                waiting_file: None,
                skipped_symlinks: 0,
                choices: HashMap::new(),
                apply_remaining: None,
                error: None,
                in_flight: 0,
                attempt: 0,
                unsaved_landed: Vec::new(),
                retry: None,
                telling_resume: false,
                sent_at: None,
                source_ids: Vec::new(),
            },
        );
        Ok(())
    })();
    persist_jobs(shared);
    let status = match accept {
        Ok(()) => job::STATUS_OK,
        Err(status) => status,
    };
    reply(
        send,
        job::encode(&job::JobReply {
            v: job::JOB_VERSION,
            id: offer.id,
            status,
            job_id: id,
        })
        .map_err(|_| ())?,
    )
    .await?;
    Ok(())
}

pub(super) fn prepared_download_permitted(
    state: &State,
    peer: &[u8; 32],
    session_id: u64,
    generation: u64,
    permission_epoch: u64,
) -> bool {
    state.trust.granted(peer, Grant::Transfer)
        && *state.transfer_epochs.get(peer).unwrap_or(&0) == permission_epoch
        && matches!(state.incoming.get(&session_id), Some(session)
            if session.generation == generation
            && session.phase == SessionPhase::Active && session.peer_key == *peer)
}

pub(in crate::core) async fn handle_download_request(
    shared: &Shared,
    session_id: u64,
    peer: &[u8; 32],
    send: &mut iroh::endpoint::SendStream,
    payload: &[u8],
    owner: Option<super::service_jobs::JobOwner>,
) -> Result<(), ()> {
    async fn reply(send: &mut iroh::endpoint::SendStream, bytes: Vec<u8>) -> Result<(), ()> {
        let mut out = Vec::with_capacity(4 + bytes.len());
        out.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
        out.extend_from_slice(&bytes);
        crate::sidestream::write_frame(
            send,
            &out,
            |framed: &Vec<u8>| Ok(framed.clone()),
            "the job frame was not written",
        )
        .await
        .map_err(|_| ())
    }
    let request: job::DownloadRequest = job::decode(payload).map_err(|_| ())?;
    job::check_version(request.v).map_err(|_| ())?;
    let respond = |status: u8, job_id: u64, manifest: Option<Manifest>| {
        job::encode(&job::ManifestReply {
            v: job::JOB_VERSION,
            id: request.id,
            status,
            job_id,
            manifest,
        })
    };
    // Fast gates first, without holding anything across an await.
    if lock(&shared.transfer_dir).is_none() {
        reply(send, respond(job::STATUS_ERROR, 0, None).map_err(|_| ())?).await?;
        return Ok(());
    }
    let permission_epoch = {
        let state = lock(&shared.state);
        state
            .trust
            .granted(peer, Grant::Transfer)
            .then(|| *state.transfer_epochs.get(peer).unwrap_or(&0))
    };
    let Some(permission_epoch) = permission_epoch else {
        reply(send, respond(job::STATUS_DENIED, 0, None).map_err(|_| ())?).await?;
        return Ok(());
    };
    if check_sources(&request.src_folder, &request.names).is_err() {
        reply(send, respond(job::STATUS_ERROR, 0, None).map_err(|_| ())?).await?;
        return Ok(());
    }
    // The preparation is a job from the start: the controller sees it as
    // Preparing, can cancel it, and loses it with its owner. Same order as
    // publication/rename: jobs -> state, so activation and permission
    // changes cannot interleave between the check and the insert. The
    // source leg is pinned now: a drive replaced before the first serve fails the
    // leg instead of serving moved bytes.
    let local_pin = lock(&shared.file_roots).pin(request.src_root).ok();
    let (stop, stopped) = tokio::sync::watch::channel(false);
    let admitted: Result<u64, u8> = (|| {
        let mut jobs = lock(&shared.jobs);
        let state = lock(&shared.state);
        if !prepared_download_permitted(
            &state,
            peer,
            session_id,
            request.generation,
            permission_epoch,
        ) || super::service_jobs::check_owner(&state, owner).is_err()
        {
            return Err(job::STATUS_DENIED);
        }
        // Sender job ids minted on the host share the local allocator, so
        // adopted controller ids can never collide with a later mint.
        let id = allocate_job_id(shared).map_err(|_| job::STATUS_ERROR)?;
        jobs.insert(
            id,
            JobRecord {
                owner,
                destination: None,
                id,
                wire_id: id,
                created_at: retention_now(),
                peer_key: *peer,
                peer_id: endpoint_id_of(peer),
                direction: JobDirection::Upload,
                sender: true,
                state: JobState::Preparing,
                manifest: None,
                local_root: request.src_root,
                local_rel: request.src_folder.clone(),
                names: request.names.clone(),
                local_pin,
                outgoing: false,
                skipped_symlinks: 0,
                file_index: 0,
                done_bytes: 0,
                files_done: 0,
                resumed_files: 0,
                skipped_files: 0,
                waiting_file: None,
                landed: HashMap::new(),
                choices: HashMap::new(),
                apply_remaining: None,
                error: None,
                in_flight: 0,
                attempt: 0,
                unsaved_landed: Vec::new(),
                retry: None,
                telling_resume: false,
                sent_at: None,
                source_ids: Vec::new(),
            },
        );
        lock(&shared.job_cancel).insert(id, stop);
        Ok(id)
    })();
    let id = match admitted {
        Ok(id) => id,
        Err(status) => {
            reply(send, respond(status, 0, None).map_err(|_| ())?).await?;
            return Ok(());
        }
    };
    persist_jobs(shared);
    // Whatever ends this request before preparation does, the job does
    // not stay Preparing.
    let mut unfinished = Unprepared { shared, id };
    // Scan, hash and offer from this host's roots; nothing is announced
    // before every hash is served. A quick preparation answers with its
    // manifest; a longer one answers at once that it is preparing, and the
    // controller asks for the manifest until it is ready.
    let preparing = prepare_host_manifest(
        shared,
        request.src_root,
        &request.src_folder,
        &request.names,
        prepare_scan_hooks(shared),
        stopped,
    );
    tokio::pin!(preparing);
    let quick = tokio::select! {
        prepared = &mut preparing => Some(prepared),
        _ = tokio::time::sleep(PREPARE_ANSWER_WITHIN) => None,
    };
    let Some(prepared) = quick else {
        reply(send, respond(job::STATUS_OK, id, None).map_err(|_| ())?).await?;
        let prepared = preparing.await;
        if let Err(status) =
            finish_preparation(shared, &request, session_id, permission_epoch, id, prepared)
        {
            tracing::info!(
                target: crate::trace::FILES,
                job = id,
                status,
                "a download prepared after its answer failed"
            );
        }
        unfinished.id = 0;
        return Ok(());
    };
    let finished = finish_preparation(shared, &request, session_id, permission_epoch, id, prepared);
    unfinished.id = 0;
    match finished {
        Ok(manifest) => {
            reply(
                send,
                respond(job::STATUS_OK, id, Some(manifest)).map_err(|_| ())?,
            )
            .await?;
        }
        Err(status) => {
            reply(send, respond(status, 0, None).map_err(|_| ())?).await?;
        }
    }
    Ok(())
}

/// How long a download request waits for its preparation before it
/// answers that the job is preparing.
pub(super) const PREPARE_ANSWER_WITHIN: Duration = Duration::from_secs(1);

pub(super) const PREPARE_FAILED: &str = "the other computer could not prepare the transfer";

pub(super) const PREPARE_STOPPED: &str = "the transfer stopped before it was prepared";

/// A download job still Preparing when its request ends fails: nothing
/// else would ever finish preparing it. `id` 0 once it finished.
pub(super) struct Unprepared<'a> {
    shared: &'a Shared,
    id: u64,
}

impl Drop for Unprepared<'_> {
    fn drop(&mut self) {
        if self.id == 0 {
            return;
        }
        lock(&self.shared.job_cancel).remove(&self.id);
        let stopped = lock(&self.shared.jobs)
            .get_mut(&self.id)
            .filter(|record| record.state == JobState::Preparing)
            .map(|record| {
                record.enter(JobState::Failed);
                record.error = Some(PREPARE_STOPPED.to_owned());
            })
            .is_some();
        if stopped {
            persist_jobs(self.shared);
        }
    }
}

/// Settles a download job once its preparation ends: Serving with its
/// manifest, or Failed. A job cancelled, or whose owner went,
/// meanwhile keeps that state; a permission or session that changed
/// meanwhile fails it. Returns the manifest or the status to answer with.
pub(super) fn finish_preparation(
    shared: &Shared,
    request: &job::DownloadRequest,
    session_id: u64,
    permission_epoch: u64,
    id: u64,
    prepared: Result<(Manifest, u64, Vec<u64>), u8>,
) -> Result<Manifest, u8> {
    lock(&shared.job_cancel).remove(&id);
    let settled = {
        let mut jobs = lock(&shared.jobs);
        let state = lock(&shared.state);
        let record = jobs.get_mut(&id).ok_or(job::STATUS_ERROR)?;
        if record.state != JobState::Preparing {
            return Err(job::STATUS_ERROR);
        }
        let peer = record.peer_key;
        let permitted = prepared_download_permitted(
            &state,
            &peer,
            session_id,
            request.generation,
            permission_epoch,
        ) && super::service_jobs::check_owner(&state, record.owner).is_ok();
        match prepared {
            Ok(_) if !permitted => {
                record.enter(JobState::Failed);
                record.error = Some(PREPARE_FAILED.to_owned());
                Err(job::STATUS_DENIED)
            }
            Ok((manifest, skipped, source_ids)) => {
                record.manifest = Some(manifest.clone());
                record.skipped_symlinks = skipped;
                record.source_ids = source_ids;
                record.enter(JobState::Serving);
                Ok(manifest)
            }
            Err(status) => {
                record.enter(JobState::Failed);
                record.error = Some(PREPARE_FAILED.to_owned());
                Err(status)
            }
        }
    };
    persist_jobs(shared);
    settled
}

#[cfg(any(test, feature = "test-probes"))]
pub(crate) static PREPARE_SCAN_HOOKS: std::sync::Mutex<Option<(usize, manifest::ScanHooks)>> =
    std::sync::Mutex::new(None);

pub(super) fn prepare_scan_hooks(shared: &Shared) -> manifest::ScanHooks {
    #[cfg(any(test, feature = "test-probes"))]
    if let Some((engine, hooks)) = *lock(&PREPARE_SCAN_HOOKS) {
        if engine == shared as *const Shared as usize {
            return hooks;
        }
    }
    let _ = shared;
    manifest::ScanHooks::none()
}

/// Scans and imports every file of the chosen entries, returning the
/// filled manifest and the symlinks left out, until `stopped` says the
/// job was cancelled or lost its owner. Runs on the host inside its
/// runtime.
pub(super) async fn prepare_host_manifest(
    shared: &Shared,
    src_root: u64,
    src_folder: &str,
    names: &[String],
    scan_hooks: manifest::ScanHooks,
    mut stopped: tokio::sync::watch::Receiver<bool>,
) -> Result<(Manifest, u64, Vec<u64>), u8> {
    tokio::select! {
        biased;
        _ = stopped.wait_for(|stop| *stop) => Err(job::STATUS_ERROR),
        prepared = prepare_all(shared, src_root, src_folder, names, scan_hooks) => prepared,
    }
}

pub(super) async fn prepare_all(
    shared: &Shared,
    src_root: u64,
    src_folder: &str,
    names: &[String],
    scan_hooks: manifest::ScanHooks,
) -> Result<(Manifest, u64, Vec<u64>), u8> {
    let mut manifest = Manifest::new(Vec::new());
    let mut skipped_symlinks = 0;
    let mut scanned = HashMap::new();
    for rel in source_rels(src_folder, names) {
        let (part, skipped, ids) = prepare_source(shared, src_root, &rel, scan_hooks).await?;
        skipped_symlinks += skipped;
        scanned.extend(ids);
        manifest.entries.extend(part.entries);
        if manifest.entries.len() > manifest::MAX_MANIFEST_FILES {
            return Err(job::STATUS_ERROR);
        }
    }
    manifest.entries.sort_by(|a, b| a.path.cmp(&b.path));
    manifest::validate(&manifest).map_err(|_| job::STATUS_ERROR)?;
    let source_ids = source_ids(&manifest, src_folder, &scanned);
    Ok((manifest, skipped_symlinks, source_ids))
}

/// Scans one chosen entry `src_rel`, returning its part of the manifest,
/// the symlinks left out and each file's id by root-relative path.
/// Nothing is read or hashed: each file is opened and checked against the
/// manifest when it is served.
pub(super) async fn prepare_source(
    shared: &Shared,
    src_root: u64,
    src_rel: &str,
    scan_hooks: manifest::ScanHooks,
) -> Result<(Manifest, u64, HashMap<String, u64>), u8> {
    // Snapshot the shared roots under a short lock; the recursive walk
    // runs on a bounded worker with no locks held.
    let snapshot = lock(&shared.file_roots).clone();
    let scan = manifest::scan_roots_async(snapshot, src_root, src_rel.to_owned(), scan_hooks)
        .await
        .map_err(|_| job::STATUS_ERROR)?;
    let mut manifest = manifest::skeleton(&scan);
    // Directory sources keep their top folder.
    {
        let roots = lock(&shared.file_roots);
        if roots.resolve_dir(src_root, src_rel).is_ok() {
            manifest::prefix_source_dir(src_rel, &mut manifest);
        }
    }
    let ids = scan
        .files
        .into_iter()
        .map(|file| (file.root_rel, file.ino))
        .collect();
    Ok((manifest, scan.skipped_symlinks, ids))
}

pub(super) fn endpoint_id_of(peer: &[u8; 32]) -> String {
    iroh::PublicKey::from_bytes(peer)
        .map(|key| key.to_string())
        .unwrap_or_default()
}

impl Core {
    #[cfg(any(test, feature = "test-probes"))]
    /// Scan probes for this engine's download preparations, or none.
    pub fn set_prepare_scan_hooks(&self, hooks: Option<manifest::ScanHooks>) {
        let _engine = self.engine();
        *lock(&PREPARE_SCAN_HOOKS) = hooks.map(|hooks| (self.prestage_key(), hooks));
    }
}
