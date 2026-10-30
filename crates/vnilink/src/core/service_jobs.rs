// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! Files-owned jobs reuse the transfer engine, with an explicit peer owner.
// trace: records every ignored failure
use super::*;
use crate::{job, CloseCause, JobSnapshot, JobState, ServiceKind, ServicePhase};
use serde::{Deserialize, Serialize};

// Scoped submissions carry their Files owner separately from unowned jobs.
pub(super) const TAG: u8 = 9;
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub(super) struct JobOwner {
    pub(super) id: u64,
    pub(super) generation: u64,
    hosted: bool,
}

pub(super) fn check_owner(state: &State, owner: Option<JobOwner>) -> Result<(), Error> {
    let Some(owner) = owner else {
        return Ok(());
    };
    let records = if owner.hosted {
        &state.hosted_services
    } else {
        &state.services
    };
    let record = records.get(&owner.id).ok_or(Error::NoSession)?;
    if record.info.generation != owner.generation {
        return Err(Error::StaleSession);
    }
    if record.info.kind != ServiceKind::Files
        || record.info.phase != ServicePhase::Active
        || *record.stop.borrow()
    {
        return Err(Error::NoSession);
    }
    let session = if owner.hosted {
        state.incoming.get(&record.info.connection_id)
    } else {
        state.outgoing.get(&record.info.connection_id)
    };
    if state.shut_down
        || !session
            .is_some_and(|s| s.generation == owner.generation && s.phase == SessionPhase::Active)
    {
        return Err(Error::NoSession);
    }
    Ok(())
}

/// The session job control with `peer` rides, the same one each time:
/// for a job a Files service owns, that service's own connection; for
/// any other, the peer's control session, since a services-only link
/// refuses jobs no service owns. `outgoing` is the side this computer
/// controls; a hosted owner lives on the other.
pub(super) fn job_session<'a>(
    state: &'a State,
    peer: &[u8; 32],
    outgoing: bool,
    owner: Option<JobOwner>,
) -> Option<&'a LiveSession> {
    let sessions = if outgoing {
        &state.outgoing
    } else {
        &state.incoming
    };
    let live = |session: &&LiveSession| session.peer_key == *peer && !finished(&session.phase);
    // A job rides the connection of the Files service that owns it.
    let owner = owner?;
    let records = if owner.hosted {
        &state.hosted_services
    } else {
        &state.services
    };
    let record = records.get(&owner.id)?;
    sessions.get(&record.info.connection_id).filter(live)
}

/// Why a job whose Files owner is gone stopped, as its owner's record
/// tells: the window here closed, this computer or the other one shut
/// down, the other computer ended its side or withdrew the permission, or
/// the connection between them ended. `sender` is whether the job sends,
/// to `peer`.
pub(super) fn orphan_reason(
    state: &State,
    owner: Option<JobOwner>,
    sender: bool,
    peer: &[u8; 32],
) -> &'static str {
    use super::jobs::{
        APP_CLOSED, CONNECTION_ENDED, OWNER_GONE, PERMISSION_OFF, RECEIVER_STOPPED,
        SENDER_SHUT_DOWN, SENDER_STOPPED,
    };
    let Some(owner) = owner else {
        return CONNECTION_ENDED;
    };
    if state.shut_down {
        return APP_CLOSED;
    }
    let (records, sessions) = if owner.hosted {
        (&state.hosted_services, &state.incoming)
    } else {
        (&state.services, &state.outgoing)
    };
    let ended = |sender| {
        if sender {
            RECEIVER_STOPPED
        } else {
            SENDER_STOPPED
        }
    };
    let Some(record) = records.get(&owner.id) else {
        if !owner.hosted {
            // A record forgotten since was closed here first.
            return OWNER_GONE;
        }
        // A host releases a service's record as it ends: its own sharing,
        // then what became of the link the service was on, says why. That
        // link is the peer's of the owner's generation, not a newer one the
        // peer dialed since.
        if !state.trust.service_granted(peer, ServiceKind::Files) {
            return PERMISSION_OFF;
        }
        let link = sessions
            .values()
            .find(|session| session.peer_key == *peer && session.generation == owner.generation);
        return match link {
            Some(link)
                if link
                    .link
                    .as_ref()
                    .is_some_and(crate::reconnect::peer_shut_down) =>
            {
                SENDER_SHUT_DOWN
            }
            Some(link) if link.connected() => ended(sender),
            _ => CONNECTION_ENDED,
        };
    };
    let peer_shut_down = sessions
        .get(&record.info.connection_id)
        .and_then(|session| session.link.as_ref())
        .is_some_and(crate::reconnect::peer_shut_down);
    match &record.info.phase {
        ServicePhase::Closed(CloseCause::OwnerClose) => OWNER_GONE,
        ServicePhase::Closed(CloseCause::Shutdown) => APP_CLOSED,
        ServicePhase::Closed(CloseCause::Revoked) => PERMISSION_OFF,
        _ if peer_shut_down => SENDER_SHUT_DOWN,
        ServicePhase::Closed(CloseCause::PeerClose) => ended(sender),
        _ => CONNECTION_ENDED,
    }
}

/// Stops the jobs whose Files owner is gone and leaves them Interrupted,
/// keeping what has arrived: an owner ends with its connection or pipe as
/// well as with its window, and a new owner adopts and resumes them
/// (`files_service_resume_orphans`). Jobs -> state is the publication lock
/// order. Never call under state.
pub(super) fn interrupt_orphaned(shared: &Shared) {
    let interrupted = {
        let mut jobs = lock(&shared.jobs);
        let state = lock(&shared.state);
        jobs.values_mut()
            .filter_map(|r| {
                // An interrupted job keeps the reason it was first given.
                if r.owner.is_some()
                    && !r.state.is_terminal()
                    && r.state != JobState::Interrupted
                    && check_owner(&state, r.owner).is_err()
                {
                    // A download still being prepared has nothing to resume.
                    if r.preparing_here() {
                        r.enter(JobState::Failed);
                        r.error = Some("the transfer stopped before it was prepared".to_owned());
                    } else {
                        r.enter(JobState::Interrupted);
                        r.error =
                            Some(orphan_reason(&state, r.owner, r.sender, &r.peer_key).to_owned());
                    }
                    r.waiting_file = None;
                    Some(r.id)
                } else {
                    None
                }
            })
            .collect::<Vec<_>>()
    };
    for id in &interrupted {
        if let Some(stop) = lock(&shared.job_cancel).remove(id) {
            stop.send_replace(true);
        }
        super::jobs::stop_prefetch(shared, *id);
    }
    if !interrupted.is_empty() {
        super::jobs::persist_jobs(shared);
    }
}

impl Core {
    fn files_job_owner(&self, id: u64, generation: u64) -> Result<(JobOwner, String), Error> {
        let lease = self.service_lease(id, generation)?;
        if lease.info().kind != ServiceKind::Files {
            return Err(Error::InvalidSetting("this is not a Files service"));
        }
        Ok((
            JobOwner {
                id,
                generation,
                hosted: false,
            },
            lease.info().peer_id.clone(),
        ))
    }
    pub(super) fn check_job_owner(&self, id: u64) -> Result<(), Error> {
        let jobs = lock(&self.shared.jobs);
        let record = jobs
            .get(&id)
            .ok_or(Error::InvalidSetting("the job is unknown"))?;
        check_owner(&lock(&self.shared.state), record.owner)
    }
    /// Copies the entries `names` of `src_folder` to `dst_rel` on the
    /// other computer as one job. Submit only after open_files_service has
    /// admitted the peer backend.
    #[allow(clippy::too_many_arguments)]
    pub fn files_service_upload(
        &self,
        id: u64,
        generation: u64,
        src_root: u64,
        src_folder: &str,
        names: &[String],
        dst_root: u64,
        dst_rel: &str,
    ) -> Result<u64, Error> {
        let _engine = self.engine();
        let (owner, peer) = self.files_job_owner(id, generation)?;
        self.submit_upload(&peer, src_root, src_folder, names, dst_root, dst_rel, owner)
    }
    /// Copies the other computer's entries `names` of `src_folder` to
    /// `dst_rel` here as one job.
    #[allow(clippy::too_many_arguments)]
    pub fn files_service_download(
        &self,
        id: u64,
        generation: u64,
        src_root: u64,
        src_folder: &str,
        names: &[String],
        dst_root: u64,
        dst_rel: &str,
    ) -> Result<u64, Error> {
        let _engine = self.engine();
        let (owner, peer) = self.files_job_owner(id, generation)?;
        self.submit_download(&peer, src_root, src_folder, names, dst_root, dst_rel, owner)
    }
    pub fn files_service_jobs(&self, id: u64, generation: u64) -> Result<Vec<JobSnapshot>, Error> {
        let _engine = self.engine();
        // Retained records remain inspectable after their owner closes.
        let service = self.controller_service(id)?;
        if service.generation != generation {
            return Err(Error::StaleSession);
        }
        if service.kind != ServiceKind::Files {
            return Err(Error::InvalidSetting("this is not a Files service"));
        }
        interrupt_orphaned(&self.shared);
        let owner = JobOwner {
            id,
            generation,
            hosted: false,
        };
        let mut jobs = lock(&self.shared.jobs)
            .values()
            .filter(|r| r.owner == Some(owner))
            .map(|r| r.snapshot())
            .collect::<Vec<_>>();
        jobs.sort_by_key(|r| r.id);
        Ok(jobs)
    }
    // Scoped actions and explicit adoption share the owner gate, so an old
    // handle cannot pass membership then act on a replacement owner.
    fn files_service_job_owner(&self, id: u64, generation: u64, job: u64) -> Result<(), Error> {
        self.ensure_alive()?;
        let jobs = lock(&self.shared.jobs);
        let state = lock(&self.shared.state);
        let service = state.services.get(&id).ok_or(Error::NoSession)?;
        if service.info.generation != generation {
            return Err(Error::StaleSession);
        }
        if service.info.kind != ServiceKind::Files {
            return Err(Error::InvalidSetting("this is not a Files service"));
        }
        let record = jobs
            .get(&job)
            .ok_or(Error::InvalidSetting("the job is unknown"))?;
        if record.owner
            != Some(JobOwner {
                id,
                generation,
                hosted: false,
            })
        {
            return Err(Error::InvalidSetting(
                "the job belongs to another Files service",
            ));
        }
        Ok(())
    }
    /// Retained terminal records remain available through their original owner.
    pub fn files_service_job_snapshot(
        &self,
        id: u64,
        generation: u64,
        job: u64,
    ) -> Result<JobSnapshot, Error> {
        let _engine = self.engine();
        let _owner_gate = lock(&self.shared.job_owner_gate);
        self.files_service_job_owner(id, generation, job)?;
        interrupt_orphaned(&self.shared);
        self.job_snapshot(job)
    }
    pub fn files_service_job_poll(
        &self,
        id: u64,
        generation: u64,
        job: u64,
    ) -> Result<JobSnapshot, Error> {
        let _engine = self.engine();
        let _owner_gate = lock(&self.shared.job_owner_gate);
        self.files_service_job_owner(id, generation, job)?;
        self.job_poll(job)
    }
    pub fn files_service_job_cancel(
        &self,
        id: u64,
        generation: u64,
        job: u64,
    ) -> Result<JobSnapshot, Error> {
        let _engine = self.engine();
        let _owner_gate = lock(&self.shared.job_owner_gate);
        self.files_service_job_owner(id, generation, job)?;
        self.job_cancel(job)
    }
    pub fn files_service_job_resume(
        &self,
        id: u64,
        generation: u64,
        job: u64,
    ) -> Result<JobSnapshot, Error> {
        let _engine = self.engine();
        let _owner_gate = lock(&self.shared.job_owner_gate);
        self.files_service_job_owner(id, generation, job)?;
        self.job_resume(job)
    }
    pub fn files_service_job_resolve(
        &self,
        id: u64,
        generation: u64,
        job: u64,
        choice: crate::ConflictChoice,
        apply_to_remaining: bool,
    ) -> Result<JobSnapshot, Error> {
        let _engine = self.engine();
        let _owner_gate = lock(&self.shared.job_owner_gate);
        self.files_service_job_owner(id, generation, job)?;
        self.job_resolve(job, choice, apply_to_remaining)
    }
    pub fn files_service_job_forget(
        &self,
        id: u64,
        generation: u64,
        job: u64,
    ) -> Result<(), Error> {
        let _engine = self.engine();
        let _owner_gate = lock(&self.shared.job_owner_gate);
        self.files_service_job_owner(id, generation, job)?;
        self.job_forget(job)
    }
    pub(super) fn service_job_round_trip(
        &self,
        owner: JobOwner,
        peer: &[u8; 32],
        tag: u8,
        payload: &[u8],
    ) -> Result<Vec<u8>, Error> {
        check_owner(&lock(&self.shared.state), Some(owner))?;
        let mut body = vec![1];
        body.extend_from_slice(&owner.id.to_be_bytes());
        body.push(tag);
        body.extend_from_slice(payload);
        let result = self.job_round_trip_side_owned(peer, true, TAG, &body, Some(owner), true);
        // An ambiguous submission may already have created a peer job. Closing
        // its owner settles that job even when we never received its wire id.
        if matches!(
            result,
            Err(Error::Network(_) | Error::Timeout | Error::Malformed)
        ) {
            crate::ignored!(
                crate::trace::FILES,
                "an owner already closed settles its job anyway",
                self.close_controller_service(owner.id, owner.generation)
            );
        }
        interrupt_orphaned(&self.shared);
        result
    }
}

pub(super) async fn serve(
    shared: &Shared,
    session_id: u64,
    peer: &[u8; 32],
    send: &mut iroh::endpoint::SendStream,
    body: &[u8],
) -> Result<(), ()> {
    if body.len() < 11 || body[0] != 1 {
        return Err(());
    }
    let remote_id = u64::from_be_bytes(body[1..9].try_into().map_err(|_| ())?);
    let payload = &body[10..];
    let generation = serde_json::from_slice::<serde_json::Value>(payload)
        .ok()
        .and_then(|v| v.get("generation")?.as_u64())
        .ok_or(())?;
    let peer_id = iroh::PublicKey::from_bytes(peer)
        .map_err(|_| ())?
        .to_string();
    let owner = {
        let state = lock(&shared.state);
        let record = state
            .hosted_services
            .values()
            .find(|r| {
                r.remote_id == Some(remote_id)
                    && r.info.connection_id == session_id
                    && r.info.peer_id == peer_id
            })
            .ok_or(())?;
        let owner = JobOwner {
            id: record.info.id,
            generation,
            hosted: true,
        };
        check_owner(&state, Some(owner)).map_err(|_| ())?;
        owner
    };
    match body[9] {
        job::TAG_JOB_UPLOAD => {
            super::jobs::handle_upload_offer(shared, peer, send, payload, Some(owner)).await
        }
        job::TAG_JOB_DOWNLOAD => {
            super::jobs::handle_download_request(
                shared,
                session_id,
                peer,
                send,
                payload,
                Some(owner),
            )
            .await
        }
        TAG_ADOPT => handle_adopt(shared, peer, send, payload, owner).await,
        _ => Err(()),
    }
}

const TAG_ADOPT: u8 = 12;
#[derive(Serialize, Deserialize)]
struct AdoptJob {
    generation: u64,
    job_id: u64,
}
struct AdoptionDrive<'a> {
    shared: &'a Shared,
    id: u64,
}
impl Drop for AdoptionDrive<'_> {
    fn drop(&mut self) {
        lock(&self.shared.job_driving).remove(&self.id);
    }
}

impl Core {
    /// Explicitly claims an inactive scoped job for a fresh Files owner. This
    /// does not resume it; both sides must durably bind before work restarts.
    pub fn files_service_adopt_job(
        &self,
        id: u64,
        generation: u64,
        job: u64,
    ) -> Result<JobSnapshot, Error> {
        let _engine = self.engine();
        self.ensure_alive()?;
        let _gate = lock(&self.shared.job_owner_gate);
        let (owner, peer_id) = self.files_job_owner(id, generation)?;
        let peer = parse_endpoint_id(&peer_id)?;
        interrupt_orphaned(&self.shared);
        if !lock(&self.shared.job_driving).insert(job) {
            return Err(Error::Busy);
        }
        let _driving = AdoptionDrive {
            shared: &self.shared,
            id: job,
        };
        let (previous, wire_id) = {
            let jobs = lock(&self.shared.jobs);
            let record = jobs
                .get(&job)
                .ok_or(Error::InvalidSetting("the job is unknown"))?;
            let state = lock(&self.shared.state);
            if record.peer_key != peer || record.owner.is_none_or(|o| o.hosted) {
                return Err(Error::InvalidSetting(
                    "the job is not a controller-owned Files job for this peer",
                ));
            }
            if record.owner == Some(owner) {
                return Ok(record.snapshot());
            }
            if check_owner(&state, record.owner).is_ok()
                || !matches!(
                    record.state,
                    JobState::Cancelled | JobState::Interrupted | JobState::Failed
                )
            {
                return Err(Error::Busy);
            }
            (record.owner, super::jobs::wire_job_id(record))
        };
        let payload = job::encode(&AdoptJob {
            generation,
            job_id: wire_id,
        })?;
        let bytes = self.service_job_round_trip(owner, &peer, TAG_ADOPT, &payload)?;
        let reply: job::JobReply = job::decode(&bytes)?;
        if reply.v != job::JOB_VERSION || reply.id != 0 || reply.job_id != wire_id {
            return Err(Error::Malformed);
        }
        if reply.status != job::STATUS_OK {
            return Err(job::error_for_status(reply.status));
        }
        {
            let mut jobs = lock(&self.shared.jobs);
            let state = lock(&self.shared.state);
            check_owner(&state, Some(owner))?;
            let record = jobs
                .get_mut(&job)
                .ok_or(Error::InvalidSetting("the job is unknown"))?;
            if record.owner != previous {
                return Err(Error::Busy);
            }
            record.owner = Some(owner);
            // Keep terminal/interrupted state until explicit Resume. This
            // also fences publication from any old generation.
        }
        if let Err(error) = super::jobs::save_jobs(&self.shared) {
            if let Some(record) = lock(&self.shared.jobs).get_mut(&job) {
                record.owner = previous;
            }
            return Err(error);
        }
        self.job_snapshot(job)
    }

    /// Claims this peer's Files jobs that were interrupted when their owner
    /// went, and resumes them, so a reconnected Files tab carries on where
    /// the lost one stopped. A job that cannot be claimed or resumed yet
    /// stays as it is; the claimed ones are returned.
    pub fn files_service_resume_orphans(
        &self,
        id: u64,
        generation: u64,
    ) -> Result<Vec<JobSnapshot>, Error> {
        let _engine = self.engine();
        let (owner, peer_id) = self.files_job_owner(id, generation)?;
        let peer = parse_endpoint_id(&peer_id)?;
        interrupt_orphaned(&self.shared);
        let mut orphans = {
            let jobs = lock(&self.shared.jobs);
            let state = lock(&self.shared.state);
            jobs.values()
                .filter(|record| {
                    record.peer_key == peer
                        && record.state == JobState::Interrupted
                        && record.owner.is_some_and(|o| !o.hosted && o != owner)
                        && check_owner(&state, record.owner).is_err()
                })
                .map(|record| record.id)
                .collect::<Vec<_>>()
        };
        orphans.sort_unstable();
        let mut claimed = Vec::new();
        for job in orphans {
            if self.files_service_adopt_job(id, generation, job).is_err() {
                continue;
            }
            let snapshot = self
                .files_service_job_resume(id, generation, job)
                .or_else(|_| self.job_snapshot(job))?;
            claimed.push(snapshot);
        }
        Ok(claimed)
    }
}

async fn handle_adopt(
    shared: &Shared,
    peer: &[u8; 32],
    send: &mut iroh::endpoint::SendStream,
    payload: &[u8],
    owner: JobOwner,
) -> Result<(), ()> {
    let request: AdoptJob = job::decode(payload).map_err(|_| ())?;
    let result = (|| {
        let mut jobs = lock(&shared.jobs);
        let state = lock(&shared.state);
        check_owner(&state, Some(owner))?;
        if !state.trust.granted(peer, crate::session::Grant::Transfer) {
            return Err(Error::TransferDenied);
        }
        let record = super::jobs::wire_job_mut(&mut jobs, peer, request.job_id)
            .ok_or(Error::InvalidSetting("the peer job is unknown"))?;
        if record.owner == Some(owner) {
            return Ok(());
        }
        if record.owner.is_none_or(|o| !o.hosted)
            || check_owner(&state, record.owner).is_ok()
            || !matches!(
                record.state,
                JobState::Cancelled | JobState::Interrupted | JobState::Failed
            )
        {
            return Err(Error::Busy);
        }
        if !lock(&shared.job_driving).insert(record.id) {
            return Err(Error::Busy);
        }
        let _driving = AdoptionDrive {
            shared,
            id: record.id,
        };
        let previous = record.owner;
        let local_id = record.id;
        record.owner = Some(owner);
        drop(state);
        drop(jobs);
        if let Err(error) = super::jobs::save_jobs(shared) {
            if let Some(record) = lock(&shared.jobs).get_mut(&local_id) {
                record.owner = previous;
            }
            return Err(error);
        }
        Ok(())
    })();
    let status = match result {
        Ok(()) => job::STATUS_OK,
        Err(Error::TransferDenied) => job::STATUS_DENIED,
        Err(Error::NoSession | Error::StaleSession) => job::STATUS_STALE,
        Err(Error::Busy) => job::STATUS_BUSY,
        _ => job::STATUS_ERROR,
    };
    let reply = job::JobReply {
        v: job::JOB_VERSION,
        id: 0,
        status,
        job_id: request.job_id,
    };
    let bytes = job::encode(&reply).map_err(|_| ())?;
    super::service_stream::write(send, bytes)
        .await
        .map_err(|_| ())
}
