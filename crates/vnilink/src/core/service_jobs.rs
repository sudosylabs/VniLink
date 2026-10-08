//! Files-owned jobs reuse the transfer engine, with an explicit peer owner.
use super::*;
use crate::{job, JobSnapshot, JobState, ServiceKind, ServicePhase};
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
        state.incoming.as_ref()
    } else {
        state.outgoing.get(&record.info.connection_id)
    };
    if state.shut_down
        || !session.is_some_and(|s| {
            s.id == record.info.connection_id
                && s.generation == owner.generation
                && s.phase == SessionPhase::Active
        })
    {
        return Err(Error::NoSession);
    }
    Ok(())
}

/// Jobs -> state is the publication lock order. Never call under state.
pub(super) fn cancel_orphaned(shared: &Shared) {
    let cancelled = {
        let mut jobs = lock(&shared.jobs);
        let state = lock(&shared.state);
        jobs.values_mut()
            .filter_map(|r| {
                if r.owner.is_some()
                    && !r.state.is_terminal()
                    && check_owner(&state, r.owner).is_err()
                {
                    r.state = JobState::Cancelled;
                    r.waiting_file = None;
                    r.error = None;
                    Some((r.id, r.peer_key, r.sender))
                } else {
                    None
                }
            })
            .collect::<Vec<_>>()
    };
    for (id, peer, sender) in &cancelled {
        if let Some(stop) = lock(&shared.job_cancel).remove(id) {
            stop.send_replace(true);
        }
        if *sender {
            Core::withdraw_job_offers(shared, peer, *id);
        }
    }
    if !cancelled.is_empty() {
        let _ = super::jobs::save_jobs(shared);
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
    /// Submit only after open_files_service has admitted the peer backend.
    pub fn files_service_upload(
        &self,
        id: u64,
        generation: u64,
        src_root: u64,
        src_rel: &str,
        dst_root: u64,
        dst_rel: &str,
    ) -> Result<u64, Error> {
        let (owner, peer) = self.files_job_owner(id, generation)?;
        self.submit_upload(&peer, src_root, src_rel, dst_root, dst_rel, Some(owner))
    }
    pub fn files_service_download(
        &self,
        id: u64,
        generation: u64,
        src_root: u64,
        src_rel: &str,
        dst_root: u64,
        dst_rel: &str,
    ) -> Result<u64, Error> {
        let (owner, peer) = self.files_job_owner(id, generation)?;
        self.submit_download(&peer, src_root, src_rel, dst_root, dst_rel, Some(owner))
    }
    pub fn files_service_jobs(&self, id: u64, generation: u64) -> Result<Vec<JobSnapshot>, Error> {
        // Retained records remain inspectable after their owner closes.
        let service = self.controller_service(id)?;
        if service.generation != generation {
            return Err(Error::StaleSession);
        }
        if service.kind != ServiceKind::Files {
            return Err(Error::InvalidSetting("this is not a Files service"));
        }
        cancel_orphaned(&self.shared);
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
    // Job owners and IDs are immutable. Checking membership before delegating
    // cannot redirect a concurrent operation to a different owner. The engine
    // rechecks active ownership at transfer publication and resume boundaries.
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
        self.files_service_job_owner(id, generation, job)?;
        cancel_orphaned(&self.shared);
        self.job_snapshot(job)
    }
    pub fn files_service_job_poll(
        &self,
        id: u64,
        generation: u64,
        job: u64,
    ) -> Result<JobSnapshot, Error> {
        self.files_service_job_owner(id, generation, job)?;
        self.job_poll(job)
    }
    pub fn files_service_job_cancel(
        &self,
        id: u64,
        generation: u64,
        job: u64,
    ) -> Result<JobSnapshot, Error> {
        self.files_service_job_owner(id, generation, job)?;
        self.job_cancel(job)
    }
    pub fn files_service_job_resume(
        &self,
        id: u64,
        generation: u64,
        job: u64,
    ) -> Result<JobSnapshot, Error> {
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
        self.files_service_job_owner(id, generation, job)?;
        self.job_resolve(job, choice, apply_to_remaining)
    }
    pub fn files_service_job_forget(
        &self,
        id: u64,
        generation: u64,
        job: u64,
    ) -> Result<(), Error> {
        self.files_service_job_owner(id, generation, job)?;
        self.job_forget(job)
    }
    pub(super) fn service_job_round_trip(
        &self,
        owner: Option<JobOwner>,
        peer: &[u8; 32],
        tag: u8,
        payload: &[u8],
    ) -> Result<Vec<u8>, Error> {
        let Some(owner) = owner else {
            return self.job_round_trip_side(peer, true, tag, payload);
        };
        check_owner(&lock(&self.shared.state), Some(owner))?;
        let mut body = vec![1];
        body.extend_from_slice(&owner.id.to_be_bytes());
        body.push(tag);
        body.extend_from_slice(payload);
        let result = self.job_round_trip_side_owned(peer, true, TAG, &body, Some(owner));
        // An ambiguous submission may already have created a peer job. Closing
        // its owner settles that job even when we never received its wire id.
        if matches!(
            result,
            Err(Error::Network(_) | Error::Timeout | Error::Malformed)
        ) {
            let _ = self.close_controller_service(owner.id, owner.generation);
        }
        cancel_orphaned(&self.shared);
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
        _ => Err(()),
    }
}

/// A rejected or cancelled preparation must not leave temporary offers behind.
pub(super) struct PreparedOffers {
    shared: Arc<Shared>,
    peer: [u8; 32],
    pub(super) hashes: Vec<[u8; 32]>,
    pub(super) committed: bool,
}
impl PreparedOffers {
    pub(super) fn new(shared: Arc<Shared>, peer: [u8; 32], scoped: bool) -> Self {
        Self {
            shared,
            peer,
            hashes: Vec::new(),
            committed: !scoped,
        }
    }
}
impl Drop for PreparedOffers {
    fn drop(&mut self) {
        if self.committed {
            return;
        }
        super::jobs::release_prepared_offers(&self.shared, &self.peer, &self.hashes);
    }
}
