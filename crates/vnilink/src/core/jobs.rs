//! Transfer job manager (Stage C).
//!
//! Jobs move whole manifests with staging, conflict choices,
//! cancellation and restartable progress. Each side polls its own
//! records; senders learn completion by asking the receiver over the
//! auxiliary stream, so both directions complete honestly. Records
//! persist as JSON beside the blob store; validated blob ranges persist
//! in the store itself.
//!
//! Threading follows the core rule: call from outside the runtime,
//! preferably off the UI thread. One `job_poll` advances at most one
//! file; progress is exact between polls.

use super::*;

use std::collections::HashMap;
use std::sync::atomic::Ordering;

use serde::{Deserialize, Serialize};

use crate::error::Error;
use crate::fs;
use crate::job::{self, ConflictChoice, JobDirection, JobSnapshot, JobState};
use crate::manifest::{self, Manifest, ManifestEntry};
use crate::session::Grant;
use iroh_blobs::api::blobs::AddPathOptions;
use iroh_blobs::api::proto::ImportMode;
use iroh_blobs::BlobFormat;

const JOBS_FILE: &str = "jobs.json";
const STAGING_DIR: &str = "staging";

const NO_JOB: &str = "the job is unknown";
const JOB_STORE_FAILED: &str = "the job record could not be saved";
const SOURCE_CHANGED: &str = "the source changed since preparation";
const VERIFY_FAILED: &str = "the destination could not be verified";
const PUBLISH_FAILED: &str = "the file could not be published";
const STAGE_FAILED: &str = "the file could not be staged";

/// Durable job record. Counters and choices survive restart; the blob
/// store keeps the verified bytes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct JobRecord {
    #[serde(default)]
    pub(super) owner: Option<super::service_jobs::JobOwner>,
    pub(super) id: u64,
    /// Peer protocol job id; zero migrates older records whose ids matched.
    #[serde(default)]
    wire_id: u64,
    #[serde(default = "retention_now")]
    created_at: u64,
    pub(super) peer_key: [u8; 32],
    peer_id: String,
    direction: JobDirection,
    pub(super) sender: bool,
    pub(super) state: JobState,
    manifest: Option<Manifest>,
    local_root: u64,
    local_rel: String,
    /// True when status queries ride the outgoing session (controller
    /// senders); false for host senders polling over the incoming one.
    outgoing: bool,
    file_index: usize,
    done_bytes: u64,
    files_done: u64,
    resumed_files: u64,
    skipped_files: u64,
    pub(super) waiting_file: Option<usize>,
    skipped_symlinks: u64,
    landed: HashMap<usize, String>,
    choices: HashMap<usize, ConflictChoice>,
    apply_remaining: Option<ConflictChoice>,
    pub(super) error: Option<String>,
}

impl JobRecord {
    fn files(&self) -> Vec<&ManifestEntry> {
        self.manifest
            .as_ref()
            .map(|manifest| {
                manifest
                    .entries
                    .iter()
                    .filter(|entry| !entry.is_dir)
                    .collect()
            })
            .unwrap_or_default()
    }

    pub(super) fn snapshot(&self) -> JobSnapshot {
        let (total_bytes, files_total, label) = self
            .manifest
            .as_ref()
            .map(|manifest| {
                let files: Vec<&str> = manifest
                    .entries
                    .iter()
                    .filter(|entry| !entry.is_dir)
                    .map(|entry| entry.path.as_str())
                    .collect();
                let label = match files.as_slice() {
                    [] => "Transfer".to_owned(),
                    [first] => (*first).to_owned(),
                    [first, rest @ ..] => format!("{first} +{} more", rest.len()),
                };
                (manifest.total_bytes(), manifest.file_count() as u64, label)
            })
            .unwrap_or((0, 0, "Transfer".to_owned()));
        JobSnapshot {
            id: self.id,
            peer: self.peer_id.clone(),
            label,
            direction: self.direction,
            sender: self.sender,
            state: self.state,
            total_bytes,
            done_bytes: self.done_bytes,
            files_total,
            files_done: self.files_done,
            resumed_files: self.resumed_files,
            skipped_files: self.skipped_files,
            skipped_symlinks: self.skipped_symlinks,
            waiting_file: self
                .waiting_file
                .and_then(|index| self.files().get(index).map(|entry| entry.path.clone())),
            error: self.error.clone(),
        }
    }
}

impl Core {
    /// Uploads a local tree to the paired peer: scans, hashes, offers and
    /// sends the manifest. The returned id is the sender record; the peer
    /// adopts the same id for its receiver record.
    pub fn job_submit_upload(
        &self,
        peer_endpoint_id: &str,
        src_root: u64,
        src_rel: &str,
        dst_root: u64,
        dst_rel: &str,
    ) -> Result<u64, Error> {
        self.submit_upload(peer_endpoint_id, src_root, src_rel, dst_root, dst_rel, None)
    }

    pub(super) fn submit_upload(
        &self,
        peer_endpoint_id: &str,
        src_root: u64,
        src_rel: &str,
        dst_root: u64,
        dst_rel: &str,
        owner: Option<super::service_jobs::JobOwner>,
    ) -> Result<u64, Error> {
        let peer = parse_endpoint_id(peer_endpoint_id)?;
        self.ensure_alive()?;
        Self::check_job_capacity(&self.shared)?;
        {
            let state = lock(&self.shared.state);
            state.trust.get(&peer).cloned().ok_or(Error::NotPaired)?;
        }
        fs::check_rel(dst_rel)?;
        // The live session first: no hashing work happens without one.
        // The host re-fences the generation on arrival.
        let generation = self.outgoing_generation_for(&peer)?;
        super::service_jobs::check_owner(&lock(&self.shared.state), owner)?;
        let scan = {
            let roots = lock(&self.shared.file_roots);
            manifest::scan(&roots, src_root, src_rel)?
        };
        let mut prepared =
            super::service_jobs::PreparedOffers::new(self.shared.clone(), peer, owner.is_some());
        let mut manifest = manifest::skeleton(&scan);
        // Import every file through anchored handles and authorize the peer
        // before announcing. Hashes fill by scan path, so directory
        // prefixing lands after.
        for file in &scan.files {
            super::service_jobs::check_owner(&lock(&self.shared.state), owner)?;
            let (hash, size) = self.blob_offer_file_at(
                peer_endpoint_id,
                src_root,
                file,
                manifest::MAX_MANIFEST_FILE_BYTES,
            )?;
            prepared.hashes.push(*hash.as_bytes());
            if size != file.size {
                return Err(Error::InvalidSetting(SOURCE_CHANGED));
            }
            manifest
                .entries
                .iter_mut()
                .find(|entry| entry.path == file.rel)
                .map(|entry| {
                    entry.hash = *hash.as_bytes();
                });
        }
        {
            // A directory source keeps its top folder; files stay flat.
            let roots = lock(&self.shared.file_roots);
            if roots.resolve_dir(src_root, src_rel).is_ok() {
                manifest::prefix_source_dir(src_rel, &mut manifest);
            }
        }
        if manifest
            .entries
            .iter()
            .filter(|entry| !entry.is_dir)
            .any(|entry| entry.hash == [0; 32])
        {
            return Err(Error::InvalidSetting("the transfer could not be prepared"));
        }
        manifest::validate(&manifest)?;
        let id = allocate_job_id(&self.shared)?;
        let record = JobRecord {
            owner,
            id,
            wire_id: id,
            created_at: retention_now(),
            peer_key: peer,
            peer_id: peer_endpoint_id.to_owned(),
            direction: JobDirection::Upload,
            sender: true,
            state: JobState::Preparing,
            manifest: Some(manifest.clone()),
            local_root: src_root,
            local_rel: src_rel.to_owned(),
            outgoing: true,
            skipped_symlinks: scan.skipped_symlinks,
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
        };
        // Announce over the auxiliary stream on the live control session.
        let offer = job::UploadOffer {
            v: job::JOB_VERSION,
            id: 1,
            generation,
            sender_job: id,
            manifest,
            dst_root,
            dst_rel: dst_rel.to_owned(),
        };
        job::check_version(offer.v)?;
        let reply_bytes =
            self.service_job_round_trip(owner, &peer, job::TAG_JOB_UPLOAD, &job::encode(&offer)?)?;
        let reply: job::JobReply = job::decode(&reply_bytes)?;
        if reply.id != offer.id {
            return Err(Error::Malformed);
        }
        match reply.status {
            status if status == job::STATUS_OK => {}
            status => return Err(job::error_for_status(status)),
        }
        {
            let mut jobs = lock(&self.shared.jobs);
            let state = lock(&self.shared.state);
            super::service_jobs::check_owner(&state, owner)?;
            let mut record = record;
            record.state = JobState::Serving;
            for hash in record_hashes(&record) {
                lock(&self.shared.blob_job_offers).insert((peer, hash));
            }
            jobs.insert(id, record);
            prepared.committed = true;
        }
        save_jobs(&self.shared)?;
        Ok(id)
    }

    /// Asks the paired peer to prepare a download from its roots. The host
    /// scans, hashes and offers; this side adopts the host's job id for
    /// its receiver record.
    pub fn job_submit_download(
        &self,
        peer_endpoint_id: &str,
        src_root: u64,
        src_rel: &str,
        dst_root: u64,
        dst_rel: &str,
    ) -> Result<u64, Error> {
        self.submit_download(peer_endpoint_id, src_root, src_rel, dst_root, dst_rel, None)
    }

    pub(super) fn submit_download(
        &self,
        peer_endpoint_id: &str,
        src_root: u64,
        src_rel: &str,
        dst_root: u64,
        dst_rel: &str,
        owner: Option<super::service_jobs::JobOwner>,
    ) -> Result<u64, Error> {
        let peer = parse_endpoint_id(peer_endpoint_id)?;
        self.ensure_alive()?;
        Self::check_job_capacity(&self.shared)?;
        {
            let state = lock(&self.shared.state);
            state.trust.get(&peer).cloned().ok_or(Error::NotPaired)?;
        }
        fs::check_rel(dst_rel)?;
        {
            let roots = lock(&self.shared.file_roots);
            if roots.snapshot().iter().all(|(id, _)| *id != dst_root) {
                return Err(Error::InvalidSetting("the destination root is unknown"));
            }
        }
        // The live session first, as in uploads.
        let generation = self.outgoing_generation_for(&peer)?;
        let request = job::DownloadRequest {
            v: job::JOB_VERSION,
            id: 1,
            generation,
            src_root,
            src_rel: src_rel.to_owned(),
        };
        let reply_bytes = self.service_job_round_trip(
            owner,
            &peer,
            job::TAG_JOB_DOWNLOAD,
            &job::encode(&request)?,
        )?;
        let reply: job::ManifestReply = job::decode(&reply_bytes)?;
        if reply.id != request.id {
            return Err(Error::Malformed);
        }
        match reply.status {
            status if status == job::STATUS_OK => {}
            status => return Err(job::error_for_status(status)),
        }
        let manifest = reply.manifest.ok_or(Error::Malformed)?;
        manifest::validate(&manifest)?;
        let id;
        {
            let mut jobs = lock(&self.shared.jobs);
            let state = lock(&self.shared.state);
            super::service_jobs::check_owner(&state, owner)?;
            if jobs
                .values()
                .any(|record| record.peer_key == peer && wire_job_id(record) == reply.job_id)
            {
                return Err(Error::InvalidSetting("the job id is already in use"));
            }
            id = allocate_job_id(&self.shared)?;
            self.shared
                .job_next
                .fetch_max(reply.job_id.saturating_add(1), Ordering::Relaxed);
            jobs.insert(
                id,
                JobRecord {
                    owner,
                    id,
                    wire_id: reply.job_id,
                    created_at: retention_now(),
                    peer_key: peer,
                    peer_id: peer_endpoint_id.to_owned(),
                    direction: JobDirection::Download,
                    sender: false,
                    state: JobState::Transferring,
                    manifest: Some(manifest),
                    local_root: dst_root,
                    local_rel: dst_rel.to_owned(),
                    outgoing: true,
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
                },
            );
        }
        save_jobs(&self.shared)?;
        Ok(id)
    }

    /// Snapshots one job record.
    pub fn job_snapshot(&self, id: u64) -> Result<JobSnapshot, Error> {
        self.ensure_alive()?;
        lock(&self.shared.jobs)
            .get(&id)
            .map(JobRecord::snapshot)
            .ok_or(Error::InvalidSetting(NO_JOB))
    }

    /// Snapshots all job records by id.
    pub fn job_list(&self) -> Result<Vec<JobSnapshot>, Error> {
        self.ensure_alive()?;
        let jobs = lock(&self.shared.jobs);
        let mut ids: Vec<u64> = jobs.keys().copied().collect();
        ids.sort();
        Ok(ids
            .iter()
            .filter_map(|id| jobs.get(id).map(JobRecord::snapshot))
            .collect())
    }

    /// Advances one job by at most one file: receivers fetch, stage and
    /// publish; senders ask the receiver for its state. Exact progress is
    /// visible between polls. Concurrent polls for the same job (background
    /// driver plus window ticks) serialize through a per-job driving guard:
    /// the second caller observes the current snapshot without advancing.
    pub fn job_poll(&self, id: u64) -> Result<JobSnapshot, Error> {
        self.ensure_alive()?;
        super::service_jobs::cancel_orphaned(&self.shared);
        let sender = {
            let jobs = lock(&self.shared.jobs);
            let record = jobs.get(&id).ok_or(Error::InvalidSetting(NO_JOB))?;
            if record.state.is_terminal()
                || record.state == JobState::Interrupted
                || (record.state == JobState::WaitingDecision && !record.sender)
            {
                return Ok(record.snapshot());
            }
            record.sender
        };
        // Per-job driving guard: background plus window ticks must not
        // advance the same job concurrently.
        if !lock(&self.shared.job_driving).insert(id) {
            return self.job_snapshot(id);
        }
        let outcome = if sender {
            self.poll_sender(id)
        } else {
            self.poll_receiver(id)
        };
        lock(&self.shared.job_driving).remove(&id);
        outcome
    }

    /// Advances every active job by one step (receivers fetch/publish,
    /// senders query receiver state). Used by the background driver and by
    /// native windows; receivers waiting for a decision are skipped.
    /// Senders keep polling so receiver-side decisions cannot strand them.
    /// Runs on the caller's thread and blocks like `job_poll`.
    pub fn drive_jobs(&self) -> Result<Vec<JobSnapshot>, Error> {
        self.ensure_alive()?;
        let ids: Vec<u64> = {
            let jobs = lock(&self.shared.jobs);
            jobs.values()
                .filter(|record| {
                    !record.state.is_terminal()
                        && (record.sender || record.state != JobState::WaitingDecision)
                })
                .map(|record| record.id)
                .collect()
        };
        let mut out = Vec::with_capacity(ids.len());
        for id in ids {
            out.push(self.job_poll(id)?);
        }
        Ok(out)
    }

    /// Starts the background transfer driver: advances every active job by
    /// one step per second on its own OS thread, independent of any Files
    /// window. Closing all windows pauses window ticks but not this driver;
    /// jobs keep their state and resume. Call once after `set_blob_directory`
    /// from the owning `Arc<Core>`; each Core gets its own driver thread
    /// that exits when the Core shuts down or drops.
    pub fn start_job_driver(self: &std::sync::Arc<Self>) {
        let weak = std::sync::Arc::downgrade(self);
        std::thread::spawn(move || loop {
            std::thread::sleep(std::time::Duration::from_secs(1));
            let Some(core) = weak.upgrade() else {
                break;
            };
            if lock(&core.shared.state).shut_down {
                break;
            }
            let _ = core.drive_jobs();
        });
    }

    fn check_job_capacity(shared: &Shared) -> Result<(), Error> {
        let active = lock(&shared.jobs)
            .values()
            .filter(|record| ACTIVE_STATES.contains(&record.state))
            .count();
        if active >= MAX_ACTIVE_JOBS {
            return Err(Error::Busy);
        }
        Ok(())
    }

    /// Cancels one job. Staging and verified store bytes stay for an
    /// explicit resume; the destination never holds a partial file.
    /// Cancellation is symmetric: the peer is notified best-effort so its
    /// record stops as well, and sender offers for this job alone are
    /// withdrawn (other jobs sharing a hash keep theirs). The receiver
    /// re-checks cancellation before every publication, so a sender cancel
    /// still prevents delivery even when bytes already arrived.
    pub fn job_cancel(&self, id: u64) -> Result<JobSnapshot, Error> {
        self.ensure_alive()?;
        let (peer_key, outgoing, was_sender) = {
            let mut jobs = lock(&self.shared.jobs);
            let record = jobs.get_mut(&id).ok_or(Error::InvalidSetting(NO_JOB))?;
            if !record.state.is_terminal() {
                record.state = JobState::Cancelled;
                record.error = None;
            }
            (record.peer_key, record.outgoing, record.sender)
        };
        if let Some(stop) = lock(&self.shared.job_cancel).remove(&id) {
            let _ = stop.send(true);
        }
        // Withdraw only this job's orphaned offers: hashes still needed by
        // another active sender job for the same peer stay served.
        if was_sender {
            Self::withdraw_job_offers(&self.shared, &peer_key, id);
        }
        // Best-effort peer notification: without a live session the local
        // record is still cancelled and the sender offers are gone, so a
        // later status poll or resume observes the terminal state.
        let _ = self.notify_peer_cancel(&peer_key, outgoing, id);
        save_jobs(&self.shared)?;
        self.job_snapshot(id)
    }

    pub(super) fn withdraw_job_offers(shared: &Shared, peer: &[u8; 32], cancelled: u64) {
        let cancelled_hashes: Vec<[u8; 32]> = {
            let jobs = lock(&shared.jobs);
            jobs.get(&cancelled)
                .and_then(|record| record.manifest.clone())
                .map(|manifest| {
                    manifest
                        .entries
                        .iter()
                        .filter(|entry| !entry.is_dir)
                        .map(|entry| entry.hash)
                        .collect()
                })
                .unwrap_or_default()
        };
        if cancelled_hashes.is_empty() {
            return;
        }
        let mut still_needed = std::collections::HashSet::new();
        {
            let jobs = lock(&shared.jobs);
            for record in jobs.values() {
                if record.id == cancelled || record.peer_key != *peer || !record.sender {
                    continue;
                }
                if record.state.is_terminal() {
                    continue;
                }
                if let Some(manifest) = &record.manifest {
                    for entry in &manifest.entries {
                        if !entry.is_dir {
                            still_needed.insert(entry.hash);
                        }
                    }
                }
            }
        }
        let mut offers = lock(&shared.blob_job_offers);
        for hash in cancelled_hashes {
            if !still_needed.contains(&hash) {
                offers.remove(&(*peer, hash));
            }
        }
    }

    fn notify_peer_cancel(
        &self,
        peer: &[u8; 32],
        outgoing: bool,
        job_id: u64,
    ) -> Result<(), Error> {
        let job_id = wire_job_id(
            lock(&self.shared.jobs)
                .get(&job_id)
                .ok_or(Error::InvalidSetting(NO_JOB))?,
        );
        let generation = self.generation_for(peer, outgoing)?;
        let cancel = job::JobCancel {
            v: job::JOB_VERSION,
            id: 1,
            generation,
            job_id,
        };
        let payload = job::encode(&cancel)?;
        // The reply is an ack; any failure (no session, stale generation,
        // denied) still leaves the local cancellation in place.
        let _ = self.job_round_trip_side(peer, outgoing, job::TAG_JOB_CANCEL, &payload);
        Ok(())
    }

    /// Resumes an interrupted, cancelled or failed job, coordinating across
    /// endpoints after symmetric cancellation: the local record resumes (sender
    /// re-offers through handles, receiver returns to `Transferring`) and the
    /// peer is notified best-effort to resume and reauthorize its offers, so
    /// retry works from either side. Source changes fail instead of moving
    /// different bytes.
    pub fn job_resume(&self, id: u64) -> Result<JobSnapshot, Error> {
        self.ensure_alive()?;
        self.check_job_owner(id)?;
        let (sender, peer_key, outgoing) = {
            let jobs = lock(&self.shared.jobs);
            let record = jobs.get(&id).ok_or(Error::InvalidSetting(NO_JOB))?;
            match record.state {
                JobState::Interrupted | JobState::Cancelled | JobState::Failed => {}
                _ => return Ok(record.snapshot()),
            }
            (record.sender, record.peer_key, record.outgoing)
        };
        // Resuming reactivates bulk work: respect the concurrency bound.
        Self::check_job_capacity(&self.shared)?;
        let snapshot = if sender {
            self.resume_sender(id)?
        } else {
            let mut jobs = lock(&self.shared.jobs);
            let record = jobs.get_mut(&id).ok_or(Error::InvalidSetting(NO_JOB))?;
            let state = lock(&self.shared.state);
            super::service_jobs::check_owner(&state, record.owner)?;
            record.state = JobState::Transferring;
            record.waiting_file = None;
            record.error = None;
            drop(state);
            drop(jobs);
            save_jobs(&self.shared)?;
            self.job_snapshot(id)?
        };
        // Best-effort peer coordination: without a live session the local
        // resume still stands and a later status poll reconciles.
        let _ = self.notify_peer_resume(&peer_key, outgoing, id);
        Ok(snapshot)
    }

    fn notify_peer_resume(
        &self,
        peer: &[u8; 32],
        outgoing: bool,
        job_id: u64,
    ) -> Result<(), Error> {
        let job_id = wire_job_id(
            lock(&self.shared.jobs)
                .get(&job_id)
                .ok_or(Error::InvalidSetting(NO_JOB))?,
        );
        let generation = self.generation_for(peer, outgoing)?;
        let resume = job::JobResume {
            v: job::JOB_VERSION,
            id: 1,
            generation,
            job_id,
        };
        let payload = job::encode(&resume)?;
        let _ = self.job_round_trip_side(peer, outgoing, job::TAG_JOB_RESUME, &payload);
        Ok(())
    }

    /// Answers the waiting conflict for one job. Receiver-owned conflicts
    /// resolve locally; sender-mirrored conflicts (upload collisions seen by
    /// the controller) forward the authenticated decision to the receiver
    /// that owns the file, so both directions share the same conflict
    /// behavior without requiring the receiver's Files window.
    pub fn job_resolve(
        &self,
        id: u64,
        choice: ConflictChoice,
        apply_to_remaining: bool,
    ) -> Result<JobSnapshot, Error> {
        self.ensure_alive()?;
        let (sender, peer_key, outgoing) = {
            let mut jobs = lock(&self.shared.jobs);
            let record = jobs.get_mut(&id).ok_or(Error::InvalidSetting(NO_JOB))?;
            super::service_jobs::check_owner(&lock(&self.shared.state), record.owner)?;
            if record.state != JobState::WaitingDecision {
                return Err(Error::InvalidSetting(
                    "the job is not waiting for a decision",
                ));
            }
            let file = record.waiting_file.ok_or(Error::InvalidSetting(NO_JOB))?;
            record.choices.insert(file, choice);
            if apply_to_remaining {
                record.apply_remaining = Some(choice);
            }
            // Receivers own their conflicts; senders mirror and forward.
            // Optimistically advance local state; the next status poll
            // reconciles if forwarding failed.
            record.state = JobState::Transferring;
            record.waiting_file = None;
            (record.sender, record.peer_key, record.outgoing)
        };
        save_jobs(&self.shared)?;
        if sender {
            let _ = self.notify_peer_resolve(&peer_key, outgoing, id, choice, apply_to_remaining);
        }
        self.job_snapshot(id)
    }

    /// Forgets one terminal job (Complete, Cancelled or Failed), releasing its
    /// staging tree and orphaned offers. Active, WaitingDecision and
    /// Interrupted jobs are refused (resume or cancel first). Interrupted
    /// retains staging/store for byte-reusing resume; forgetting it would
    /// force full re-download, so explicit cancel-then-forget is required
    /// (cancel retains staging for resume; forget releases it).
    pub fn job_forget(&self, id: u64) -> Result<(), Error> {
        self.ensure_alive()?;
        let (peer_key, hashes) = {
            let mut jobs = lock(&self.shared.jobs);
            let record = jobs.get(&id).ok_or(Error::InvalidSetting(NO_JOB))?;
            match record.state {
                JobState::Complete | JobState::Cancelled | JobState::Failed => {}
                _ => {
                    return Err(Error::InvalidSetting(
                        "only finished jobs can be forgotten; cancel or complete first",
                    ));
                }
            }
            let hashes: Vec<[u8; 32]> = record
                .manifest
                .as_ref()
                .map(|manifest| {
                    manifest
                        .entries
                        .iter()
                        .filter(|entry| !entry.is_dir)
                        .map(|entry| entry.hash)
                        .collect()
                })
                .unwrap_or_default();
            (record.peer_key, hashes)
        };
        // Release staging (Complete already removed via finalize; Cancelled/
        // Failed may retain for resume – forget releases) and orphaned offers.
        if let Some(dir) = lock(&self.shared.blob_dir).clone() {
            let _ = std::fs::remove_dir_all(dir.join(STAGING_DIR).join(format!("job-{id}")));
        }
        {
            let mut jobs = lock(&self.shared.jobs);
            jobs.remove(&id);
        }
        Self::drop_orphaned_offers(&self.shared, &peer_key, &hashes, id);
        save_jobs(&self.shared)?;
        Ok(())
    }

    fn notify_peer_resolve(
        &self,
        peer: &[u8; 32],
        outgoing: bool,
        job_id: u64,
        choice: ConflictChoice,
        apply_to_remaining: bool,
    ) -> Result<(), Error> {
        let job_id = wire_job_id(
            lock(&self.shared.jobs)
                .get(&job_id)
                .ok_or(Error::InvalidSetting(NO_JOB))?,
        );
        let generation = self.generation_for(peer, outgoing)?;
        let choice_name = match choice {
            ConflictChoice::Skip => "skip",
            ConflictChoice::Replace => "replace",
            ConflictChoice::KeepBoth => "keep-both",
        }
        .to_owned();
        let resolve = job::JobResolve {
            v: job::JOB_VERSION,
            id: 1,
            generation,
            job_id,
            choice: choice_name,
            apply_to_remaining,
        };
        let payload = job::encode(&resolve)?;
        let _ = self.job_round_trip_side(peer, outgoing, job::TAG_JOB_RESOLVE, &payload);
        Ok(())
    }

    fn poll_sender(&self, id: u64) -> Result<JobSnapshot, Error> {
        let (peer_key, outgoing) = {
            let jobs = lock(&self.shared.jobs);
            let record = jobs.get(&id).ok_or(Error::InvalidSetting(NO_JOB))?;
            (record.peer_key, record.outgoing)
        };
        let generation = match self.generation_for(&peer_key, outgoing) {
            Ok(generation) => generation,
            Err(Error::NoSession | Error::StaleSession) => return self.mark_interrupted(id),
            Err(error) => return Err(error),
        };
        let query = job::JobStatus {
            v: job::JOB_VERSION,
            id: 1,
            generation,
            job_id: wire_job_id(
                lock(&self.shared.jobs)
                    .get(&id)
                    .ok_or(Error::InvalidSetting(NO_JOB))?,
            ),
        };
        let reply_bytes = match self.job_round_trip_side(
            &peer_key,
            outgoing,
            job::TAG_JOB_STATUS,
            &job::encode(&query)?,
        ) {
            Ok(bytes) => bytes,
            Err(_) => return self.job_snapshot(id),
        };
        let reply: job::JobStateReply = job::decode(&reply_bytes).map_err(|_| Error::Malformed)?;
        if reply.id != query.id {
            return Err(Error::Malformed);
        }
        if reply.status != job::STATUS_OK {
            return self.job_snapshot(id);
        }
        let state = job::JobState::from_name(&reply.state).ok_or(Error::Malformed)?;
        {
            let mut jobs = lock(&self.shared.jobs);
            let record = jobs.get_mut(&id).ok_or(Error::InvalidSetting(NO_JOB))?;
            // Do not overwrite a locally cancelled record with a stale
            // receiver state: cancellation wins.
            if record.state == JobState::Cancelled {
                drop(jobs);
            } else {
                match state {
                    JobState::Complete => {
                        record.state = JobState::Complete;
                        record.error = None;
                        record.waiting_file = None;
                    }
                    JobState::Failed | JobState::Cancelled | JobState::Interrupted => {
                        record.state = state;
                        record.error = reply.error.map(|text| truncate_error(&text));
                        record.waiting_file = None;
                    }
                    JobState::WaitingDecision => {
                        record.state = JobState::WaitingDecision;
                        record.error = None;
                        // Map the receiver's waiting path back to this
                        // record's file index (manifests correlate by id).
                        record.waiting_file = reply.waiting_file.as_deref().and_then(|path| {
                            record
                                .manifest
                                .as_ref()?
                                .entries
                                .iter()
                                .filter(|entry| !entry.is_dir)
                                .position(|entry| entry.path == path)
                        });
                    }
                    JobState::Transferring if record.state == JobState::WaitingDecision => {
                        super::service_jobs::check_owner(&lock(&self.shared.state), record.owner)?;
                        record.state = JobState::Serving;
                        record.waiting_file = None;
                    }
                    _ => {}
                }
                drop(jobs);
            }
        }
        save_jobs(&self.shared)?;
        self.job_snapshot(id)
    }

    fn poll_receiver(&self, id: u64) -> Result<JobSnapshot, Error> {
        // Cancellation or connection interruption stops before any fetch.
        {
            let jobs = lock(&self.shared.jobs);
            let record = jobs.get(&id).ok_or(Error::InvalidSetting(NO_JOB))?;
            if matches!(record.state, JobState::Cancelled | JobState::Interrupted) {
                return Ok(record.snapshot());
            }
        }
        let (peer_id, peer_bytes, file_index, file_count) = {
            let jobs = lock(&self.shared.jobs);
            let record = jobs.get(&id).ok_or(Error::InvalidSetting(NO_JOB))?;
            (
                record.peer_id.clone(),
                record.peer_key,
                record.file_index,
                record.files().len(),
            )
        };
        if file_index >= file_count {
            // Zero-file manifests still carry directory entries: build the
            // skeleton before claiming completion so empty folders land.
            // Revocation stops here: no new directories after the grant is gone.
            if !Self::receiver_granted(&self.shared, &peer_bytes) {
                return self.mark_interrupted(id);
            }
            self.ensure_receiver_dirs(id)?;
            return self.finalize_receiver(id);
        }
        // First poll ensures the directory skeleton exactly once per job;
        // creation is idempotent, so repeats stay safe. Revocation stops
        // directory creation as well as file publication.
        if file_index == 0 {
            if !Self::receiver_granted(&self.shared, &peer_bytes) {
                return self.mark_interrupted(id);
            }
            self.ensure_receiver_dirs(id)?;
        }
        let entry = {
            let jobs = lock(&self.shared.jobs);
            let record = jobs.get(&id).ok_or(Error::InvalidSetting(NO_JOB))?;
            record.files().get(file_index).cloned().cloned()
        }
        .ok_or(Error::InvalidSetting(NO_JOB))?;
        // Conflict check before any fetch: the destination wins ties until
        // the owner decides.
        let choice = self.conflict_choice(id, file_index, &entry)?;
        let Some(choice) = choice else {
            let mut jobs = lock(&self.shared.jobs);
            let record = jobs.get_mut(&id).ok_or(Error::InvalidSetting(NO_JOB))?;
            if matches!(record.state, JobState::Cancelled | JobState::Interrupted) {
                return Ok(record.snapshot());
            }
            record.state = JobState::WaitingDecision;
            record.waiting_file = Some(file_index);
            drop(jobs);
            save_jobs(&self.shared)?;
            return self.job_snapshot(id);
        };
        if choice == ConflictChoice::Skip {
            let mut jobs = lock(&self.shared.jobs);
            let record = jobs.get_mut(&id).ok_or(Error::InvalidSetting(NO_JOB))?;
            // A cancelled poll between check and commit still stops here.
            if matches!(record.state, JobState::Cancelled | JobState::Interrupted) {
                drop(jobs);
                save_jobs(&self.shared)?;
                return self.job_snapshot(id);
            }
            record.skipped_files += 1;
            record.done_bytes = record.done_bytes.saturating_add(entry.size);
            record.file_index += 1;
            drop(jobs);
            save_jobs(&self.shared)?;
            return self.job_snapshot(id);
        }
        // Fetch under the receiver's own grant: revocation stops new
        // reads here even while the sender still serves.
        if !Self::receiver_granted(&self.shared, &peer_bytes) {
            return self.mark_interrupted(id);
        }
        // Fetch into the persistent store with a per-poll cancel watch.
        let (stop_tx, stop_rx) = tokio::sync::watch::channel(false);
        {
            let jobs = lock(&self.shared.jobs);
            let record = jobs.get(&id).ok_or(Error::InvalidSetting(NO_JOB))?;
            let state = lock(&self.shared.state);
            if matches!(record.state, JobState::Cancelled | JobState::Interrupted) {
                return Ok(record.snapshot());
            }
            super::service_jobs::check_owner(&state, record.owner)?;
            lock(&self.shared.job_cancel).insert(id, stop_tx);
        }
        let hash = iroh_blobs::Hash::from_bytes(entry.hash);
        let fetch = self.blob_fetch_streamed(&peer_id, hash, None, Some(stop_rx));
        lock(&self.shared.job_cancel).remove(&id);
        // A cancel or interruption during fetch preserves its state even if bytes arrived.
        {
            let jobs = lock(&self.shared.jobs);
            let record = jobs.get(&id).ok_or(Error::InvalidSetting(NO_JOB))?;
            if matches!(record.state, JobState::Cancelled | JobState::Interrupted) {
                drop(jobs);
                save_jobs(&self.shared)?;
                return self.job_snapshot(id);
            }
        }
        // Revocation between fetch and publication must not create the
        // destination file: re-check authorization before any staging or
        // commit. The publication itself re-checks as well.
        if !Self::receiver_granted(&self.shared, &peer_bytes) {
            return self.mark_interrupted(id);
        }
        let downloaded = match fetch {
            Ok(crate::core::blob::FetchResult::Done { downloaded }) => downloaded,
            Err(_) => {
                return self.mark_interrupted(id);
            }
            Ok(crate::core::blob::FetchResult::Cancelled) => {
                let mut jobs = lock(&self.shared.jobs);
                let record = jobs.get_mut(&id).ok_or(Error::InvalidSetting(NO_JOB))?;
                record.state = JobState::Cancelled;
                drop(jobs);
                save_jobs(&self.shared)?;
                return self.job_snapshot(id);
            }
        };
        // Stage through a temp sibling, then publish per the decision.
        let staged = self.stage_path(id, &entry.path)?;
        if let Some(parent) = staged.parent() {
            std::fs::create_dir_all(parent).map_err(|_| Error::InvalidSetting(STAGE_FAILED))?;
        }
        if self.blob_export(hash, &staged).is_err() {
            return self.mark_failed(id, STAGE_FAILED);
        }
        let staged_size = std::fs::metadata(&staged)
            .map(|meta| meta.len())
            .unwrap_or(u64::MAX);
        if staged_size != entry.size {
            let _ = std::fs::remove_file(&staged);
            return self.mark_failed(id, VERIFY_FAILED);
        }
        let landed = match self.publish_staged(id, &entry, &staged, choice)? {
            Some(landed) => landed,
            None => return self.job_snapshot(id),
        };
        // Skip leaves the destination untouched; anything published lands
        // under its final name for verification.
        let _ = landed;
        {
            let mut jobs = lock(&self.shared.jobs);
            let record = jobs.get_mut(&id).ok_or(Error::InvalidSetting(NO_JOB))?;
            let at = record.file_index;
            if choice != ConflictChoice::Skip {
                record.landed.insert(at, landed);
            }
        }
        {
            let mut jobs = lock(&self.shared.jobs);
            let record = jobs.get_mut(&id).ok_or(Error::InvalidSetting(NO_JOB))?;
            if matches!(record.state, JobState::Cancelled | JobState::Interrupted) {
                drop(jobs);
                save_jobs(&self.shared)?;
                return self.job_snapshot(id);
            }
            if downloaded < entry.size {
                record.resumed_files += 1;
            }
            record.files_done += 1;
            record.done_bytes = record.done_bytes.saturating_add(entry.size);
            record.file_index += 1;
            drop(jobs);
        }
        save_jobs(&self.shared)?;
        self.job_snapshot(id)
    }

    /// Returns the stored choice, the job-wide choice, or `None` when the
    /// destination is free. A waiting state is entered by the caller.
    fn conflict_choice(
        &self,
        id: u64,
        file_index: usize,
        entry: &ManifestEntry,
    ) -> Result<Option<ConflictChoice>, Error> {
        let (local_root, local_rel) = {
            let jobs = lock(&self.shared.jobs);
            let record = jobs.get(&id).ok_or(Error::InvalidSetting(NO_JOB))?;
            (record.local_root, record.local_rel.clone())
        };
        let dest = self.receiver_dest(id, local_root, &local_rel, &entry.path)?;
        if std::fs::symlink_metadata(&dest).is_err() {
            return Ok(Some(ConflictChoice::Replace));
        }
        let jobs = lock(&self.shared.jobs);
        let record = jobs.get(&id).ok_or(Error::InvalidSetting(NO_JOB))?;
        if let Some(choice) = record.choices.get(&file_index).copied() {
            return Ok(Some(choice));
        }
        if let Some(choice) = record.apply_remaining {
            return Ok(Some(choice));
        }
        Ok(None)
    }

    fn receiver_dest(
        &self,
        id: u64,
        root: u64,
        rel: &str,
        path: &str,
    ) -> Result<std::path::PathBuf, Error> {
        let jobs = lock(&self.shared.jobs);
        let record = jobs.get(&id).ok_or(Error::InvalidSetting(NO_JOB))?;
        let state = lock(&self.shared.state);
        super::service_jobs::check_owner(&state, record.owner)?;
        let roots = lock(&self.shared.file_roots);
        let (parent_rel, name) = match path.rfind('/') {
            Some(slash) => (&path[..slash], &path[slash + 1..]),
            None => ("", path),
        };
        // The manifest jail already validated every component; resolve the
        // parent through it (creating missing levels idempotently) and
        // append the single validated file name.
        let full_parent = if rel.is_empty() {
            parent_rel.to_owned()
        } else if parent_rel.is_empty() {
            rel.to_owned()
        } else {
            format!("{rel}/{parent_rel}")
        };
        if roots.resolve_dir(root, &full_parent).is_err() {
            manifest::create_levels(&roots, root, rel, parent_rel)?;
        }
        let parent = roots.resolve_dir(root, &full_parent)?;
        Ok(parent.join(name))
    }

    fn stage_path(&self, id: u64, rel_path: &str) -> Result<std::path::PathBuf, Error> {
        let blob_dir = lock(&self.shared.blob_dir)
            .clone()
            .ok_or(Error::Unsupported("no blob directory is installed"))?;
        Ok(blob_dir
            .join(STAGING_DIR)
            .join(format!("job-{id}"))
            .join(rel_path))
    }

    fn ensure_receiver_dirs(&self, id: u64) -> Result<(), Error> {
        let jobs = lock(&self.shared.jobs);
        let state = lock(&self.shared.state);
        let (local_root, local_rel, manifest) = {
            let record = jobs.get(&id).ok_or(Error::InvalidSetting(NO_JOB))?;
            super::service_jobs::check_owner(&state, record.owner)?;
            (
                record.local_root,
                record.local_rel.clone(),
                record
                    .manifest
                    .clone()
                    .ok_or(Error::InvalidSetting(NO_JOB))?,
            )
        };
        let roots = lock(&self.shared.file_roots);
        // The destination base itself may be fresh: build it first so the
        // per-entry levels below always start from an existing folder.
        if !local_rel.is_empty() && roots.resolve_dir(local_root, &local_rel).is_err() {
            manifest::create_levels(&roots, local_root, "", &local_rel)?;
        }
        manifest::ensure_manifest_dirs(&roots, local_root, &local_rel, &manifest)
    }

    /// Publishes one staged file, returning the landed file name.
    /// Returns `None` when the poll must stop with the stored snapshot
    /// (a lost KeepBoth race re-enters the conflict state).
    /// Enforces the transfer grant at publication: revocation between fetch
    /// and commit refuses the write instead of creating the destination.
    fn publish_staged(
        &self,
        id: u64,
        entry: &ManifestEntry,
        staged: &std::path::Path,
        choice: ConflictChoice,
    ) -> Result<Option<String>, Error> {
        let (local_root, local_rel, peer_key) = {
            let jobs = lock(&self.shared.jobs);
            let record = jobs.get(&id).ok_or(Error::InvalidSetting(NO_JOB))?;
            (record.local_root, record.local_rel.clone(), record.peer_key)
        };
        if !Self::receiver_granted(&self.shared, &peer_key) {
            let _ = std::fs::remove_file(staged);
            self.mark_interrupted(id)?;
            return Ok(None);
        }
        let policy = match choice {
            ConflictChoice::Replace => manifest::PublishPolicy::Replace,
            ConflictChoice::KeepBoth => manifest::PublishPolicy::KeepBoth,
            ConflictChoice::Skip => return Ok(None),
        };
        #[cfg(unix)]
        {
            self.publish_staged_anchored(id, &entry.path, staged, policy, local_root, &local_rel)
        }
        #[cfg(not(unix))]
        {
            let dest = self.receiver_dest(id, local_root, &local_rel, &entry.path)?;
            let dest_dir = dest
                .parent()
                .ok_or(Error::InvalidSetting(PUBLISH_FAILED))?
                .to_path_buf();
            let file_name = dest
                .file_name()
                .and_then(|name| name.to_str())
                .ok_or(Error::InvalidSetting(PUBLISH_FAILED))?
                .to_owned();
            match manifest::publish_file(&dest_dir, &file_name, staged, policy) {
                Ok(landed) => Ok(Some(landed)),
                Err(Error::FileExists) => {
                    // Lost a KeepBoth race after choosing: surface the
                    // conflict again instead of failing the job.
                    let mut jobs = lock(&self.shared.jobs);
                    let record = jobs.get_mut(&id).ok_or(Error::InvalidSetting(NO_JOB))?;
                    record.choices.remove(&record.file_index);
                    record.state = JobState::WaitingDecision;
                    record.waiting_file = Some(record.file_index);
                    drop(jobs);
                    save_jobs(&self.shared)?;
                    Ok(None)
                }
                Err(_) => {
                    let _ = std::fs::remove_file(staged);
                    self.mark_failed(id, PUBLISH_FAILED)?;
                    Ok(None)
                }
            }
        }
    }

    /// Anchored publication: the destination parent is opened as a directory
    /// handle and the staged file is committed with `renameat`, so a parent
    /// replaced by a symlink cannot redirect the write outside the root.
    /// The final authorization/state decision is synchronized with the commit:
    /// both the job and grant locks are held across the re-check and the
    /// `renameat`, so receiver cancellation (`Cancelled`) or revocation
    /// (`Interrupted` via grant loss) completing before the rename prevents
    /// destination creation.
    #[cfg(unix)]
    fn publish_staged_anchored(
        &self,
        id: u64,
        manifest_path: &str,
        staged: &std::path::Path,
        policy: manifest::PublishPolicy,
        local_root: u64,
        local_rel: &str,
    ) -> Result<Option<String>, Error> {
        use std::os::unix::io::AsFd;
        let (parent_rel, file_name) = match manifest_path.rfind('/') {
            Some(slash) => (&manifest_path[..slash], &manifest_path[slash + 1..]),
            None => ("", manifest_path),
        };
        let full_parent = if local_rel.is_empty() {
            parent_rel.to_owned()
        } else if parent_rel.is_empty() {
            local_rel.to_owned()
        } else {
            format!("{local_rel}/{parent_rel}")
        };
        // Ensure the parent chain exists through anchored mkdirs, then open it.
        // No job/state locks held across mkdirs (they may block); the commit
        // below re-locks and re-checks.
        {
            let jobs = lock(&self.shared.jobs);
            let record = jobs.get(&id).ok_or(Error::InvalidSetting(NO_JOB))?;
            let state = lock(&self.shared.state);
            super::service_jobs::check_owner(&state, record.owner)?;
            let roots = lock(&self.shared.file_roots);
            if roots.open_dir_fd(local_root, &full_parent).is_err() {
                manifest::create_levels(&roots, local_root, local_rel, parent_rel)?;
            }
        }
        let roots = lock(&self.shared.file_roots);
        let dir_fd = roots.open_dir_fd(local_root, &full_parent)?;
        drop(roots);
        // For KeepBoth, choose the free name through the anchored listing
        // before locking; the no-replace commit below decides atomically and
        // re-enters conflict on a lost race.
        let free_name: Option<String> = match policy {
            manifest::PublishPolicy::Replace => None,
            manifest::PublishPolicy::KeepBoth => {
                let existing: std::collections::HashSet<String> =
                    vnilink_rename::anchored::list_names(dir_fd.as_fd())
                        .map_err(|_| Error::InvalidSetting(PUBLISH_FAILED))?
                        .into_iter()
                        .filter_map(|os| os.into_string().ok())
                        .collect();
                Some(manifest::keep_both_name(file_name, |candidate| {
                    existing.contains(candidate)
                }))
            }
        };
        // Commit under both locks: re-check cancellation and grant after any
        // barrier, then `renameat` without releasing. Revocation (grant update
        // needs `state`, interruption needs `jobs`) and cancellation (needs
        // `jobs`) both block here, so completing before the rename prevents it.
        let commit: Result<Option<String>, Error> = (|| {
            let mut jobs = lock(&self.shared.jobs);
            let record = jobs.get_mut(&id).ok_or(Error::InvalidSetting(NO_JOB))?;
            if matches!(record.state, JobState::Cancelled | JobState::Interrupted) {
                return Ok(None);
            }
            let peer_key = record.peer_key;
            // Hold `state` from grant check through `renameat`: revoke's grant
            // update needs `state`, so it blocks until after the commit.
            // Jobs->state order; no other path nests state->jobs.
            let state_guard = lock(&self.shared.state);
            let granted = match state_guard.trust.get(&peer_key) {
                Some(device) => state_guard
                    .trust
                    .granted(&device.endpoint_id, Grant::Transfer),
                None => false,
            };
            if !granted || super::service_jobs::check_owner(&state_guard, record.owner).is_err() {
                if record.state != JobState::Cancelled {
                    record.state = JobState::Interrupted;
                    record.waiting_file = None;
                }
                drop(state_guard);
                drop(jobs);
                let _ = std::fs::remove_file(staged);
                save_jobs(&self.shared)?;
                return Ok(None);
            }
            // Still active with both locks held: commit without releasing.
            // `state_guard` fences revoke's grant update; `jobs` fences
            // cancellation/interruption until after `renameat`.
            match policy {
                manifest::PublishPolicy::Replace => {
                    // Recheck conflict immediately before commit (handoff): an
                    // absent destination uses no-replace so a newly appearing
                    // file returns to conflict instead of silent overwrite;
                    // explicit Replace (stored choice or apply-remaining) may
                    // overwrite an existing destination.
                    let dest_exists =
                        vnilink_rename::anchored::stat_at(dir_fd.as_fd(), file_name).is_ok();
                    let at = record.file_index;
                    let explicit_replace = record.choices.get(&at).copied()
                        == Some(ConflictChoice::Replace)
                        || record.apply_remaining == Some(ConflictChoice::Replace);
                    if dest_exists && !explicit_replace {
                        record.choices.remove(&at);
                        record.state = JobState::WaitingDecision;
                        record.waiting_file = Some(at);
                        drop(state_guard);
                        drop(jobs);
                        save_jobs(&self.shared)?;
                        return Ok(None);
                    }
                    // Absent (auto or explicit with deleted dest) -> no-replace
                    // (detects race via Exists); present + explicit -> overwrite.
                    let allow_overwrite = dest_exists && explicit_replace;
                    match vnilink_rename::anchored::rename_from_staged(
                        staged,
                        dir_fd.as_fd(),
                        file_name,
                        allow_overwrite,
                    ) {
                        Ok(()) => Ok(Some(file_name.to_owned())),
                        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                            // Lost auto-Replace race (dest appeared): conflict.
                            record.choices.remove(&record.file_index);
                            record.state = JobState::WaitingDecision;
                            record.waiting_file = Some(record.file_index);
                            drop(state_guard);
                            drop(jobs);
                            save_jobs(&self.shared)?;
                            return Ok(None);
                        }
                        Err(_) => {
                            drop(state_guard);
                            drop(jobs);
                            let _ = std::fs::remove_file(staged);
                            self.mark_failed(id, PUBLISH_FAILED)?;
                            return Ok(None);
                        }
                    }
                }
                manifest::PublishPolicy::KeepBoth => {
                    let free = free_name
                        .clone()
                        .ok_or(Error::InvalidSetting(PUBLISH_FAILED))?;
                    if free == file_name {
                        match vnilink_rename::anchored::rename_from_staged(
                            staged,
                            dir_fd.as_fd(),
                            file_name,
                            false,
                        ) {
                            Ok(()) => Ok(Some(file_name.to_owned())),
                            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                                // Lost race: re-enter conflict.
                                record.choices.remove(&record.file_index);
                                record.state = JobState::WaitingDecision;
                                record.waiting_file = Some(record.file_index);
                                drop(state_guard);
                                drop(jobs);
                                save_jobs(&self.shared)?;
                                return Ok(None);
                            }
                            Err(_) => {
                                drop(state_guard);
                                drop(jobs);
                                let _ = std::fs::remove_file(staged);
                                self.mark_failed(id, PUBLISH_FAILED)?;
                                return Ok(None);
                            }
                        }
                    } else {
                        match vnilink_rename::anchored::rename_from_staged(
                            staged,
                            dir_fd.as_fd(),
                            &free,
                            false,
                        ) {
                            Ok(()) => Ok(Some(free)),
                            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                                record.choices.remove(&record.file_index);
                                record.state = JobState::WaitingDecision;
                                record.waiting_file = Some(record.file_index);
                                drop(state_guard);
                                drop(jobs);
                                save_jobs(&self.shared)?;
                                return Ok(None);
                            }
                            Err(_) => {
                                drop(state_guard);
                                drop(jobs);
                                let _ = std::fs::remove_file(staged);
                                self.mark_failed(id, PUBLISH_FAILED)?;
                                return Ok(None);
                            }
                        }
                    }
                }
            }
        })();
        // `commit` only returns `Ok` (all io errors handled above as failed/
        // conflict); any crate `Err` (unknown job, etc.) propagates.
        commit
    }

    fn finalize_receiver(&self, id: u64) -> Result<JobSnapshot, Error> {
        // Verify every manifest path landed before claiming completion.
        // Skipped files intentionally keep the destination as it was.
        let (local_root, local_rel, manifest, choices) = {
            let jobs = lock(&self.shared.jobs);
            let record = jobs.get(&id).ok_or(Error::InvalidSetting(NO_JOB))?;
            (
                record.local_root,
                record.local_rel.clone(),
                record
                    .manifest
                    .clone()
                    .ok_or(Error::InvalidSetting(NO_JOB))?,
                record.choices.clone(),
            )
        };
        let landed = {
            let jobs = lock(&self.shared.jobs);
            let record = jobs.get(&id).ok_or(Error::InvalidSetting(NO_JOB))?;
            record.landed.clone()
        };
        for (index, entry) in manifest
            .entries
            .iter()
            .filter(|entry| !entry.is_dir)
            .enumerate()
        {
            if choices.get(&index).copied() == Some(ConflictChoice::Skip) {
                continue;
            }
            let name = landed.get(&index).ok_or(Error::InvalidSetting(NO_JOB))?;
            let dest = self.receiver_dest(id, local_root, &local_rel, &entry.path)?;
            let dest = dest
                .parent()
                .ok_or(Error::InvalidSetting(VERIFY_FAILED))?
                .join(name);
            let size = std::fs::metadata(&dest)
                .map(|meta| meta.len())
                .unwrap_or(u64::MAX);
            if size != entry.size {
                return self.mark_failed(id, VERIFY_FAILED);
            }
        }
        // Release staging; the collector releases payloads once no job needs them.
        if let Some(dir) = lock(&self.shared.blob_dir).clone() {
            let _ = std::fs::remove_dir_all(dir.join(STAGING_DIR).join(format!("job-{id}")));
        }
        // Terminal cleanup: completed jobs never authorize again (see
        // `is_active_serve`); drop orphaned offers so authorization does not
        // outlive the job in memory. Completed jobs do not pin payloads;
        // garbage collection keeps the delivered destination as the sole copy.
        {
            let (peer_key, hashes) = {
                let jobs = lock(&self.shared.jobs);
                let record = jobs.get(&id).ok_or(Error::InvalidSetting(NO_JOB))?;
                let hashes: Vec<[u8; 32]> = record
                    .manifest
                    .as_ref()
                    .map(|manifest| {
                        manifest
                            .entries
                            .iter()
                            .filter(|entry| !entry.is_dir)
                            .map(|entry| entry.hash)
                            .collect()
                    })
                    .unwrap_or_default();
                (record.peer_key, hashes)
            };
            Self::drop_orphaned_offers(&self.shared, &peer_key, &hashes, id);
        }
        {
            let mut jobs = lock(&self.shared.jobs);
            let record = jobs.get_mut(&id).ok_or(Error::InvalidSetting(NO_JOB))?;
            if matches!(record.state, JobState::Cancelled | JobState::Interrupted) {
                return Ok(record.snapshot());
            }
            record.state = JobState::Complete;
            record.error = None;
            drop(jobs);
        }
        save_jobs(&self.shared)?;
        let (peer, hashes) = {
            let jobs = lock(&self.shared.jobs);
            let record = jobs.get(&id).ok_or(Error::InvalidSetting(NO_JOB))?;
            (record.peer_key, record_hashes(record))
        };
        Self::drop_orphaned_offers(&self.shared, &peer, &hashes, id);
        self.job_snapshot(id)
    }

    fn mark_interrupted(&self, id: u64) -> Result<JobSnapshot, Error> {
        {
            let mut jobs = lock(&self.shared.jobs);
            let record = jobs.get_mut(&id).ok_or(Error::InvalidSetting(NO_JOB))?;
            if record.state == JobState::Cancelled {
                drop(jobs);
            } else {
                record.state = JobState::Interrupted;
                drop(jobs);
            }
        }
        save_jobs(&self.shared)?;
        self.job_snapshot(id)
    }

    /// Receiver-side authorization at publication time: the peer must still
    /// be paired with the transfer grant. Revocation stops publication even
    /// when bytes already arrived.
    fn receiver_granted(shared: &Shared, peer: &[u8; 32]) -> bool {
        let state = lock(&shared.state);
        match state.trust.get(peer) {
            Some(device) => state.trust.granted(&device.endpoint_id, Grant::Transfer),
            None => false,
        }
    }

    fn mark_failed(&self, id: u64, reason: &'static str) -> Result<JobSnapshot, Error> {
        let (peer_key, hashes) = {
            let mut jobs = lock(&self.shared.jobs);
            let record = jobs.get_mut(&id).ok_or(Error::InvalidSetting(NO_JOB))?;
            record.state = JobState::Failed;
            record.error = Some(reason.to_owned());
            let hashes: Vec<[u8; 32]> = record
                .manifest
                .as_ref()
                .map(|manifest| {
                    manifest
                        .entries
                        .iter()
                        .filter(|entry| !entry.is_dir)
                        .map(|entry| entry.hash)
                        .collect()
                })
                .unwrap_or_default();
            (record.peer_key, hashes)
        };
        // Failed jobs do not resume without re-import: drop staging to free
        // disk (resume re-fetches), and drop orphaned offers (memory).
        if let Some(dir) = lock(&self.shared.blob_dir).clone() {
            let _ = std::fs::remove_dir_all(dir.join(STAGING_DIR).join(format!("job-{id}")));
        }
        Self::drop_orphaned_offers(&self.shared, &peer_key, &hashes, id);
        save_jobs(&self.shared)?;
        self.job_snapshot(id)
    }

    /// Drops `(peer, hash)` offers not needed by any non-terminal job
    /// (active + resumable Interrupted/Cancelled). Terminal Complete/Failed
    /// jobs never authorize (see `is_active_serve`), but their offers linger
    /// in memory without this cleanup.
    fn drop_orphaned_offers(shared: &Shared, peer: &[u8; 32], hashes: &[[u8; 32]], completed: u64) {
        if hashes.is_empty() {
            return;
        }
        let mut needed = std::collections::HashSet::new();
        {
            let jobs = lock(&shared.jobs);
            for record in jobs.values() {
                if record.id == completed || record.peer_key != *peer {
                    continue;
                }
                match record.state {
                    JobState::Complete | JobState::Failed => continue,
                    _ => {}
                }
                if let Some(manifest) = &record.manifest {
                    for entry in &manifest.entries {
                        if !entry.is_dir {
                            needed.insert(entry.hash);
                        }
                    }
                }
            }
        }
        let live = retained_hashes(&lock(&shared.jobs));
        let mut pins = lock(&shared.blob_import_pins);
        for hash in hashes {
            if !live.contains(&iroh_blobs::Hash::from_bytes(*hash)) {
                pins.remove(hash);
            }
        }
        drop(pins);
        let mut offers = lock(&shared.blob_job_offers);
        for hash in hashes {
            if !needed.contains(hash) {
                offers.remove(&(*peer, *hash));
            }
        }
    }

    fn resume_sender(&self, id: u64) -> Result<JobSnapshot, Error> {
        // Re-verify sources and re-authorize hashes after a restart. A
        // changed source fails the job instead of moving new bytes.
        let (peer_id, peer_key, local_root, local_rel, manifest) = {
            let jobs = lock(&self.shared.jobs);
            let record = jobs.get(&id).ok_or(Error::InvalidSetting(NO_JOB))?;
            (
                record.peer_id.clone(),
                record.peer_key,
                record.local_root,
                record.local_rel.clone(),
                record
                    .manifest
                    .clone()
                    .ok_or(Error::InvalidSetting(NO_JOB))?,
            )
        };
        for entry in manifest.entries.iter().filter(|entry| !entry.is_dir) {
            // Root-relative source path: the manifest entry already carries
            // the top folder for directory sources, so prepend only the
            // source parent (not the full `local_rel`, which would double
            // the top folder for nested sources like `docs/pack`).
            let root_rel = if local_rel.is_empty() {
                entry.path.clone()
            } else {
                match local_rel.rfind('/') {
                    Some(slash) => format!("{}/{}", &local_rel[..slash], entry.path),
                    None => entry.path.clone(),
                }
            };
            // Anchored verification: re-open through handles, never the
            // retained absolute pathname. A swapped parent fails here.
            #[cfg(unix)]
            {
                use std::os::unix::io::AsFd;
                let (parent_rel, name) = match root_rel.rfind('/') {
                    Some(slash) => (&root_rel[..slash], &root_rel[slash + 1..]),
                    None => ("", root_rel.as_str()),
                };
                // Release roots before recording failure (roots->jobs would
                // invert publication/rename jobs->state->roots order).
                let verified: Result<(), Error> = (|| {
                    let roots = lock(&self.shared.file_roots);
                    let parent_fd = roots
                        .open_dir_fd(local_root, parent_rel)
                        .map_err(|_| Error::InvalidSetting(SOURCE_CHANGED))?;
                    let st = vnilink_rename::anchored::stat_at(parent_fd.as_fd(), name)
                        .map_err(|_| Error::InvalidSetting(SOURCE_CHANGED))?;
                    if !st.is_file || st.is_symlink || st.size != entry.size {
                        return Err(Error::InvalidSetting(SOURCE_CHANGED));
                    }
                    Ok(())
                })();
                if verified.is_err() {
                    return self.mark_failed(id, SOURCE_CHANGED);
                }
            }
            #[cfg(not(unix))]
            {
                // Fallback pathname check; residual race documented.
                let (parent_rel, name) = match root_rel.rfind('/') {
                    Some(slash) => (
                        root_rel[..slash].to_owned(),
                        root_rel[slash + 1..].to_owned(),
                    ),
                    None => (String::new(), root_rel.clone()),
                };
                let roots = lock(&self.shared.file_roots);
                let (_parent, full) = roots
                    .resolve_parent(local_root, &parent_rel, &name)
                    .map_err(|_| Error::InvalidSetting(SOURCE_CHANGED))?;
                drop(roots);
                let meta =
                    std::fs::metadata(&full).map_err(|_| Error::InvalidSetting(SOURCE_CHANGED))?;
                if meta.len() != entry.size {
                    return self.mark_failed(id, SOURCE_CHANGED);
                }
            }
            let hash = iroh_blobs::Hash::from_bytes(entry.hash);
            let present = self
                .blob_has(hash)
                .map_err(|_| Error::InvalidSetting(SOURCE_CHANGED))?;
            if !present {
                // Re-offer with old hash/size/mtime authentication (no ScannedFile
                // dev/ino; hash compare detects replacement even with preserved
                // mtime/size).
                let old_mtime = manifest
                    .entries
                    .iter()
                    .filter(|e| !e.is_dir)
                    .find(|e| e.hash == entry.hash)
                    .map(|e| e.source_mtime_ms)
                    .unwrap_or(0);
                let (fresh, size) = self
                    .blob_reoffer_file_at(
                        &peer_id,
                        local_root,
                        &root_rel,
                        entry.hash,
                        entry.size,
                        old_mtime,
                        manifest::MAX_MANIFEST_FILE_BYTES,
                    )
                    .map_err(|_| Error::InvalidSetting(SOURCE_CHANGED))?;
                if fresh != hash || size != entry.size {
                    return self.mark_failed(id, SOURCE_CHANGED);
                }
            } else {
                lock(&self.shared.blob_job_offers).insert((peer_key, entry.hash));
            }
        }
        {
            let mut jobs = lock(&self.shared.jobs);
            let record = jobs.get_mut(&id).ok_or(Error::InvalidSetting(NO_JOB))?;
            let state = lock(&self.shared.state);
            super::service_jobs::check_owner(&state, record.owner)?;
            record.state = JobState::Serving;
            record.error = None;
            drop(state);
            drop(jobs);
        }
        save_jobs(&self.shared)?;
        self.job_snapshot(id)
    }

    fn outgoing_generation_for(&self, peer: &[u8; 32]) -> Result<u64, Error> {
        let state = self.alive_state()?;
        let session = state.outgoing_peer(peer).ok_or(Error::NoSession)?;
        if session.phase != crate::session::SessionPhase::Active || session.peer_key != *peer {
            return Err(Error::NoSession);
        }
        Ok(session.wire_stamp.generation)
    }

    fn generation_for(&self, peer: &[u8; 32], outgoing: bool) -> Result<u64, Error> {
        let state = self.alive_state()?;
        let session = if outgoing {
            state.outgoing_peer(peer)
        } else {
            state.incoming.as_ref()
        }
        .ok_or(Error::NoSession)?;
        if session.phase != crate::session::SessionPhase::Active || session.peer_key != *peer {
            return Err(Error::NoSession);
        }
        Ok(session.wire_stamp.generation)
    }

    pub(super) fn job_round_trip_side(
        &self,
        peer: &[u8; 32],
        outgoing: bool,
        tag: u8,
        payload: &[u8],
    ) -> Result<Vec<u8>, Error> {
        self.job_round_trip_side_owned(peer, outgoing, tag, payload, None)
    }

    pub(super) fn job_round_trip_side_owned(
        &self,
        peer: &[u8; 32],
        outgoing: bool,
        tag: u8,
        payload: &[u8],
        owner: Option<super::service_jobs::JobOwner>,
    ) -> Result<Vec<u8>, Error> {
        let mut lease = owner
            .map(|owner| self.service_lease(owner.id, owner.generation))
            .transpose()?;
        // Senders poll over whichever session faces the peer: outgoing
        // for controller senders, incoming for host senders. Both ride
        // the live control connection.
        let connection = {
            let state = self.alive_state()?;
            let session = if outgoing {
                state.outgoing_peer(peer)
            } else {
                state.incoming.as_ref()
            }
            .ok_or(Error::NoSession)?;
            if session.phase != crate::session::SessionPhase::Active {
                return Err(Error::NoSession);
            }
            if session.peer_key != *peer {
                return Err(Error::NoSession);
            }
            session.link.clone().ok_or(Error::NoSession)?
        };
        let mut frame = Vec::with_capacity(1 + payload.len());
        frame.push(tag);
        frame.extend_from_slice(payload);
        self.runtime.block_on(async {
            let work = async {
                let (mut send, mut recv) =
                    connection.open_bi().await.map_err(Error::network_debug)?;
                crate::sidestream::write_frame(
                    &mut send,
                    &frame,
                    |bytes: &Vec<u8>| {
                        let mut out = Vec::with_capacity(4 + bytes.len());
                        out.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
                        out.extend_from_slice(bytes);
                        Ok(out)
                    },
                    "the job frame was not written",
                )
                .await?;
                let reply: Vec<u8> = crate::sidestream::read_frame(
                    &mut recv,
                    |len| {
                        if len > job::MAX_JOB_FRAME {
                            Err(Error::Malformed)
                        } else {
                            Ok(())
                        }
                    },
                    |bytes| Ok(bytes.to_vec()),
                )
                .await?;
                Ok(reply)
            };
            if let Some(lease) = lease.as_mut() {
                lease
                    .run(async {
                        tokio::time::timeout(Duration::from_secs(60), work)
                            .await
                            .map_err(|_| Error::Timeout)?
                    })
                    .await
            } else {
                work.await
            }
        })
    }
}

fn truncate_error(text: &str) -> String {
    const MAX: usize = 256;
    if text.len() <= MAX {
        return text.to_owned();
    }
    text[..MAX].to_owned()
}

/// Largest concurrent bulk jobs (Serving/Transferring/WaitingDecision).
/// Bounds staging disk (manifest-capped per job), memory and bulk traffic
/// alongside desktop video/input. Paused (Interrupted) and terminal jobs do
/// not count; explicit resume re-checks capacity.
pub(crate) const MAX_ACTIVE_JOBS: usize = 8;

pub(super) const ACTIVE_STATES: &[JobState] = &[
    JobState::Preparing,
    JobState::Serving,
    JobState::Transferring,
    JobState::WaitingDecision,
];

/// Retention: staging lives under the blob dir per job (`staging/job-{id}`),
/// removed on Complete (verified publish) and Failed (no resume without
/// re-import); retained on Cancelled/Interrupted for byte-reusing resume.
/// Blob store bytes persist for resume efficiency; offers are memory-only and
/// dropped when no active or resumable job needs the hash (terminal Complete
/// or Failed drops orphaned offers; Cancelled/Interrupted retain for resume).
/// Manifests bound each job (5000 files, 1 GiB total, 256 MiB/file) and
/// `MAX_ACTIVE_JOBS` bounds concurrency, so disk stays proportional to
/// in-flight plus resumable work, not unbounded history.

/// Resumable payload cache: seven days, at most 4 GiB across inactive jobs.
/// Active work remains protected by the manifest and concurrency limits.
const RESUME_CACHE_BYTES: u64 = 4 * 1024 * 1024 * 1024;
const RESUME_CACHE_AGE: u64 = 7 * 24 * 60 * 60;
fn retention_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn allocate_job_id(shared: &Shared) -> Result<u64, Error> {
    shared
        .job_next
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |id| id.checked_add(1))
        .map_err(|_| Error::Busy)
}

fn wire_job_id(record: &JobRecord) -> u64 {
    if record.wire_id == 0 {
        record.id
    } else {
        record.wire_id
    }
}
fn wire_job<'a>(
    jobs: &'a HashMap<u64, JobRecord>,
    peer: &[u8; 32],
    id: u64,
) -> Option<&'a JobRecord> {
    jobs.values()
        .find(|record| record.peer_key == *peer && wire_job_id(record) == id)
}
fn wire_job_mut<'a>(
    jobs: &'a mut HashMap<u64, JobRecord>,
    peer: &[u8; 32],
    id: u64,
) -> Option<&'a mut JobRecord> {
    jobs.values_mut()
        .find(|record| record.peer_key == *peer && wire_job_id(record) == id)
}

/// Neutral links accept lifecycle messages only for live service-owned jobs.
/// All four control requests share the status request's identity fields.
pub(super) fn neutral_job_control_allowed(
    shared: &Shared,
    peer: &[u8; 32],
    tag: u8,
    payload: &[u8],
) -> bool {
    if !matches!(
        tag,
        job::TAG_JOB_STATUS | job::TAG_JOB_CANCEL | job::TAG_JOB_RESOLVE | job::TAG_JOB_RESUME
    ) {
        return false;
    }
    let Ok(query) = job::decode::<job::JobStatus>(payload) else {
        return false;
    };
    let jobs = lock(&shared.jobs);
    wire_job(&jobs, peer, query.job_id).is_some_and(|record| {
        record.owner.is_some()
            && super::service_jobs::check_owner(&lock(&shared.state), record.owner).is_ok()
    })
}

pub(super) fn record_hashes(record: &JobRecord) -> Vec<[u8; 32]> {
    record.files().into_iter().map(|entry| entry.hash).collect()
}

pub(super) fn record_reclaimable(
    record: &JobRecord,
    retained_jobs: &std::collections::HashSet<u64>,
) -> bool {
    !retained_jobs.contains(&record.id)
}

pub(super) fn retained_hashes(
    jobs: &HashMap<u64, JobRecord>,
) -> std::collections::HashSet<iroh_blobs::Hash> {
    retained_job_ids(jobs)
        .into_iter()
        .flat_map(|id| record_hashes(&jobs[&id]))
        .map(iroh_blobs::Hash::from_bytes)
        .collect()
}

pub(super) fn retained_job_ids(jobs: &HashMap<u64, JobRecord>) -> std::collections::HashSet<u64> {
    let mut live = std::collections::HashSet::new();
    let mut records: Vec<_> = jobs.values().collect();
    records.sort_by_key(|record| std::cmp::Reverse((record.created_at, record.id)));
    let mut budget = RESUME_CACHE_BYTES;
    for record in records {
        let keep = match record.state {
            JobState::Complete | JobState::Failed => false,
            JobState::Interrupted | JobState::Cancelled => {
                let size = record
                    .manifest
                    .as_ref()
                    .map(Manifest::total_bytes)
                    .unwrap_or(0);
                if retention_now().saturating_sub(record.created_at) > RESUME_CACHE_AGE
                    || size > budget
                {
                    false
                } else {
                    budget -= size;
                    true
                }
            }
            _ => true,
        };
        if keep {
            live.insert(record.id);
        }
    }
    live
}

/// True when `peer` has an active sender job needing `hash`. Blob serving
/// requires this in addition to pairing + offer set membership, so completed
/// or cancelled jobs never authorize even when their offers linger.
pub(super) fn is_active_serve(shared: &Shared, peer: &[u8; 32], hash: &[u8; 32]) -> bool {
    let jobs = lock(&shared.jobs);
    let state = lock(&shared.state);
    jobs.values().any(|record| {
        super::service_jobs::check_owner(&state, record.owner).is_ok()
            && record.peer_key == *peer
            && record.sender
            && matches!(
                record.state,
                JobState::Serving | JobState::Transferring | JobState::WaitingDecision
            )
            && record.manifest.as_ref().is_some_and(|manifest| {
                manifest
                    .entries
                    .iter()
                    .filter(|entry| !entry.is_dir)
                    .any(|entry| entry.hash == *hash)
            })
    })
}

/// Serve-time source re-verification for job blob requests (see blob.rs):
/// finds the active sender entry for `hash`, reconstructs its root-relative
/// source (parent of `local_rel` + manifest path, handling nested `docs/pack`
/// correctly) and stats it through handles (NOFOLLOW, regular, size matches).
/// Denies swapped parents/replaced files so TryReference absolute paths can
/// never disclose outside bytes.
#[cfg(unix)]
pub(super) fn verify_serve_source(
    shared: &Shared,
    peer: &[u8; 32],
    hash: &[u8; 32],
) -> Result<std::os::unix::io::OwnedFd, Error> {
    let (local_root, local_rel, entry_size, entry_path) = {
        let jobs = lock(&shared.jobs);
        let mut found = None;
        for record in jobs.values() {
            if record.peer_key != *peer || !record.sender {
                continue;
            }
            match record.state {
                JobState::Serving | JobState::Transferring | JobState::WaitingDecision => {}
                _ => continue,
            }
            if let Some(manifest) = &record.manifest {
                if let Some(entry) = manifest
                    .entries
                    .iter()
                    .filter(|e| !e.is_dir)
                    .find(|e| e.hash == *hash)
                {
                    found = Some((
                        record.local_root,
                        record.local_rel.clone(),
                        entry.size,
                        entry.path.clone(),
                    ));
                    break;
                }
            }
        }
        found.ok_or(Error::TransferDenied)?
    };
    let root_rel = if local_rel.is_empty() {
        entry_path.clone()
    } else {
        match local_rel.rfind('/') {
            Some(slash) => format!("{}/{}", &local_rel[..slash], entry_path),
            None => entry_path.clone(),
        }
    };
    let (parent_rel, name) = match root_rel.rfind('/') {
        Some(slash) => (&root_rel[..slash], &root_rel[slash + 1..]),
        None => ("", root_rel.as_str()),
    };
    #[cfg(unix)]
    {
        use std::os::unix::io::AsFd;
        let roots = lock(&shared.file_roots);
        let parent_fd = roots
            .open_dir_fd(local_root, parent_rel)
            .map_err(|_| Error::TransferDenied)?;
        let st = vnilink_rename::anchored::stat_at(parent_fd.as_fd(), name)
            .map_err(|_| Error::TransferDenied)?;
        if !st.is_file || st.is_symlink || st.size != entry_size {
            return Err(Error::TransferDenied);
        }
        let fd =
            vnilink_rename::anchored::open_file_verified(parent_fd.as_fd(), name, st.dev, st.ino)
                .map_err(|_| Error::TransferDenied)?;
        drop(parent_fd);
        drop(roots);
        let duplicate = fd.try_clone().map_err(|_| Error::TransferDenied)?;
        if Core::hash_fd(duplicate)? != *hash {
            return Err(Error::TransferDenied);
        }
        Ok(fd)
    }
    #[cfg(not(unix))]
    {
        let _ = (parent_rel, name, entry_size);
        // Fallback pathname check; residual race documented with rename fallback.
        Ok(())
    }
}

/// True when renaming `rel/name` under `root` would disturb an active
/// transfer: the entry is a source of a sender job or a destination (base
/// or manifest path) of a receiver job. Terminal jobs never block; anything
/// resumable does, so publication cannot strand or recreate the old path.
pub(super) fn rename_blocked(shared: &Shared, root: u64, rel: &str, name: &str) -> bool {
    let jobs = lock(&shared.jobs);
    rename_blocked_with(&jobs, root, rel, name)
}

pub(super) fn rename_blocked_with(
    jobs: &std::collections::HashMap<u64, JobRecord>,
    root: u64,
    rel: &str,
    name: &str,
) -> bool {
    let target = if rel.is_empty() {
        name.to_owned()
    } else {
        format!("{rel}/{name}")
    };
    for record in jobs.values() {
        if record.local_root != root {
            continue;
        }
        match record.state {
            JobState::Complete | JobState::Cancelled | JobState::Failed => continue,
            _ => {}
        }
        if record.sender {
            // Sender source: `local_rel` names the selected file, folder or
            // whole root (empty). Any overlap blocks.
            if record.local_rel.is_empty() {
                return true;
            }
            if paths_overlap(&record.local_rel, &target) {
                return true;
            }
        } else {
            // Receiver destination: base plus every manifest path. An empty
            // base with no manifest yet still blocks renames of the base
            // itself (checked via base overlap).
            if paths_overlap(&record.local_rel, &target) {
                return true;
            }
            if let Some(manifest) = &record.manifest {
                for entry in &manifest.entries {
                    let full = if record.local_rel.is_empty() {
                        entry.path.clone()
                    } else {
                        format!("{}/{}", record.local_rel, entry.path)
                    };
                    // `entry.path` for dirs is the dir itself; files give
                    // their full path. Either overlapping the rename target
                    // (the entry itself, a child, or an ancestor) blocks.
                    if paths_overlap(&full, &target) {
                        return true;
                    }
                }
            }
        }
    }
    false
}

fn paths_overlap(a: &str, b: &str) -> bool {
    if a.is_empty() || b.is_empty() {
        // Empty names the root itself: any entry under that root overlaps.
        return true;
    }
    a == b || a.starts_with(&format!("{b}/")) || b.starts_with(&format!("{a}/"))
}

/// Marks a peer's active jobs interrupted on revoke, unpair and grant
/// removal. Verified bytes stay in the store; resume continues them.
/// In-flight fetches are signalled through their cancel watches so active
/// work stops instead of publishing after revocation.
pub(super) fn interrupt_peer_jobs(shared: &Shared, peer: &[u8; 32]) {
    interrupt_jobs(shared, peer, false);
}
pub(super) fn interrupt_controller_jobs(shared: &Shared, peer: &[u8; 32]) {
    interrupt_jobs(shared, peer, true);
}
fn interrupt_jobs(shared: &Shared, peer: &[u8; 32], controller_only: bool) {
    let ids: Vec<u64> = {
        let mut jobs = lock(&shared.jobs);
        let mut ids = Vec::new();
        for record in jobs.values_mut() {
            if record.peer_key == *peer && (!controller_only || record.outgoing) {
                match record.state {
                    JobState::Serving | JobState::Transferring | JobState::WaitingDecision => {
                        record.state = JobState::Interrupted;
                        record.waiting_file = None;
                        ids.push(record.id);
                    }
                    _ => {}
                }
            }
        }
        ids
    };
    for id in ids {
        if let Some(stop) = lock(&shared.job_cancel).remove(&id) {
            let _ = stop.send(true);
        }
    }
    let _ = save_jobs(shared);
}

pub(super) fn save_jobs(shared: &Shared) -> Result<(), Error> {
    let _writer = lock(&shared.job_save);
    let Some(dir) = lock(&shared.blob_dir).clone() else {
        return Ok(());
    };
    let jobs = lock(&shared.jobs);
    let mut records: Vec<&JobRecord> = jobs.values().collect();
    records.sort_by_key(|record| record.id);
    let owned: Vec<JobRecord> = records.into_iter().cloned().collect();
    drop(jobs);
    let text =
        serde_json::to_string(&owned).map_err(|_| Error::InvalidSetting(JOB_STORE_FAILED))?;
    let tmp = dir.join(format!("{}.tmp", JOBS_FILE));
    std::fs::write(&tmp, text).map_err(|_| Error::InvalidSetting(JOB_STORE_FAILED))?;
    std::fs::rename(&tmp, dir.join(JOBS_FILE))
        .map_err(|_| Error::InvalidSetting(JOB_STORE_FAILED))?;
    Ok(())
}

pub(super) fn load_jobs(shared: &Shared) {
    let Some(dir) = lock(&shared.blob_dir).clone() else {
        return;
    };
    let text = match std::fs::read_to_string(dir.join(JOBS_FILE)) {
        Ok(text) => text,
        Err(_) => return,
    };
    let mut records: Vec<JobRecord> = match serde_json::from_str(&text) {
        Ok(records) => records,
        Err(_) => return,
    };
    let mut max = 0u64;
    {
        let mut jobs = lock(&shared.jobs);
        for mut record in records.drain(..) {
            if let Some(owner) = record.owner.as_mut() {
                owner.id = 0; // service ids are process-local; never rebind on restart
            }
            // Older controller download records used the wrong side flag.
            if !record.sender && record.direction == JobDirection::Download {
                record.outgoing = true;
            }
            // Anything in flight died with the process.
            match record.state {
                JobState::Preparing | JobState::Serving | JobState::Transferring => {
                    record.state = JobState::Interrupted;
                }
                _ => {}
            }
            max = max.max(record.id).max(wire_job_id(&record));
            jobs.insert(record.id, record);
        }
    }
    let next = shared.job_next.load(Ordering::Relaxed);
    if max.saturating_add(1) > next {
        shared
            .job_next
            .store(max.saturating_add(1), Ordering::Relaxed);
    }
}

/// Host-side job request handler: upload offers, download requests and
/// status queries from the controller. Generation is fenced before any
/// grant check or disk touch.
pub(super) async fn handle_job_request(
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
        matches!(state.incoming.as_ref(), Some(session)
            if session.id == session_id
                && session.generation == generation
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
        };
        reply(send, job::encode(&denied).map_err(|_| ())?).await?;
        return Ok(());
    }
    match tag {
        t if t == job::TAG_JOB_UPLOAD => {
            handle_upload_offer(shared, peer, send, payload, None).await
        }
        t if t == job::TAG_JOB_DOWNLOAD => {
            handle_download_request(shared, session_id, peer, send, payload, None).await
        }
        t if t == job::TAG_JOB_STATUS => {
            handle_status_query(shared, peer, true, send, payload).await
        }
        t if t == job::TAG_JOB_CANCEL => handle_cancel(shared, peer, true, send, payload).await,
        t if t == job::TAG_JOB_RESOLVE => handle_resolve(shared, peer, true, send, payload).await,
        t if t == job::TAG_JOB_RESUME => handle_resume(shared, peer, true, send, payload).await,
        _ => Err(()),
    }
}

/// Controller-side job handler: status queries from the host and symmetric
/// cancellation/decisions/resume from either sender or receiver arrive here.
pub(super) async fn handle_controller_job(
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

/// Symmetric cancellation: either side tells the peer one job is cancelled.
/// Generation is fenced like status queries; the peer marks its record
/// cancelled and signals its in-flight fetch so publication stops.
async fn handle_cancel(
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
    let live = {
        let state = lock(&shared.state);
        let session = if incoming_side {
            state.incoming.as_ref()
        } else {
            state.outgoing_peer(peer)
        };
        matches!(session, Some(session)
            if session.wire_stamp.generation == cancel.generation
                && session.phase == crate::session::SessionPhase::Active
                && session.peer_key == *peer)
    };
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
                record.state = JobState::Cancelled;
                record.error = None;
                record.waiting_file = None;
                Some(record.id)
            }
            _ => None,
        }
    };
    if let Some(id) = id {
        if let Some(stop) = lock(&shared.job_cancel).remove(&id) {
            let _ = stop.send(true);
        }
        let _ = save_jobs(shared);
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

async fn handle_status_query(
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
    let live = {
        let state = lock(&shared.state);
        let session = if incoming_side {
            state.incoming.as_ref()
        } else {
            state.outgoing_peer(peer)
        };
        matches!(session, Some(session)
            if session.wire_stamp.generation == query.generation
                && session.phase == crate::session::SessionPhase::Active
                && session.peer_key == *peer)
    };
    if !live {
        let stale = job::JobStateReply {
            v: job::JOB_VERSION,
            id: query.id,
            status: job::STATUS_STALE,
            state: JobState::Interrupted.name().to_owned(),
            error: None,
            waiting_file: None,
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
                }
            }
            _ => job::JobStateReply {
                v: job::JOB_VERSION,
                id: query.id,
                status: job::STATUS_ERROR,
                state: JobState::Interrupted.name().to_owned(),
                error: None,
                waiting_file: None,
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
async fn handle_resolve(
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
    let live = {
        let state = lock(&shared.state);
        let session = if incoming_side {
            state.incoming.as_ref()
        } else {
            state.outgoing_peer(peer)
        };
        matches!(session, Some(session)
            if session.wire_stamp.generation == resolve.generation
                && session.phase == crate::session::SessionPhase::Active
                && session.peer_key == *peer)
    };
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
                record.state = JobState::Transferring;
                record.waiting_file = None;
                job::STATUS_OK
            }
            _ => job::STATUS_ERROR,
        }
    };
    // `status` computed under lock; save outside (save takes jobs lock).
    let _ = save_jobs(shared);
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
/// Cancelled/Interrupted/Failed job to active and reauthorizes its blob
/// offers through handles, so retry works from either side. Senders re-offer
/// after source verification; receivers return to `Transferring`.
async fn handle_resume(
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
    let live = {
        let state = lock(&shared.state);
        let session = if incoming_side {
            state.incoming.as_ref()
        } else {
            state.outgoing_peer(peer)
        };
        matches!(session, Some(session)
            if session.wire_stamp.generation == resume.generation
                && session.phase == crate::session::SessionPhase::Active
                && session.peer_key == *peer)
    };
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
                        }
                    }
                    _ => ResumeSnap::AckOk,
                }
            }
            _ => ResumeSnap::AckErr,
        }
    };
    let (sender, local_root, local_rel, manifest) = match snap {
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
        } => (sender, local_root, local_rel, manifest),
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
        // Reauthorize offers through handles: re-verify each source size and
        // re-import missing hashes via fd paths; present hashes re-offered
        // without rereading. Source changes fail instead of moving new bytes.
        let manifest = manifest.ok_or(())?;
        // Compute root-relative source paths (same reconstruction as
        // `resume_sender`: parent of `local_rel` + manifest path).
        let src_parent = if local_rel.is_empty() {
            String::new()
        } else {
            match local_rel.rfind('/') {
                Some(slash) => local_rel[..slash].to_owned(),
                None => String::new(),
            }
        };
        let store = super::blob::open_store(shared).await.map_err(|_| ())?;
        for entry in manifest.entries.iter().filter(|e| !e.is_dir) {
            let root_rel = if local_rel.is_empty() {
                entry.path.clone()
            } else if src_parent.is_empty() {
                entry.path.clone()
            } else {
                format!("{src_parent}/{}", entry.path)
            };
            let (parent_rel, name) = match root_rel.rfind('/') {
                Some(slash) => (&root_rel[..slash], &root_rel[slash + 1..]),
                None => ("", root_rel.as_str()),
            };
            #[cfg(unix)]
            {
                use std::os::unix::io::AsFd;
                let parent_fd = {
                    let roots = lock(&shared.file_roots);
                    let fd = roots.open_dir_fd(local_root, parent_rel).map_err(|_| ())?;
                    drop(roots);
                    fd
                };
                let st =
                    vnilink_rename::anchored::stat_at(parent_fd.as_fd(), name).map_err(|_| ())?;
                if !st.is_file || st.is_symlink || st.size != entry.size {
                    // Source changed: fail the job rather than serving new bytes.
                    // Scoped so no std guard lives across the reply await below.
                    {
                        let mut jobs = lock(&shared.jobs);
                        if let Some(record) = wire_job_mut(&mut jobs, peer, resume.job_id) {
                            record.state = JobState::Failed;
                            record.error = Some(SOURCE_CHANGED.to_owned());
                        }
                    }
                    let _ = save_jobs(shared);
                    let err = job::JobReply {
                        v: job::JOB_VERSION,
                        id: resume.id,
                        status: job::STATUS_ERROR,
                        job_id: resume.job_id,
                    };
                    reply(send, job::encode(&err).map_err(|_| ())?).await?;
                    return Ok(());
                }
                let hash = iroh_blobs::Hash::from_bytes(entry.hash);
                // Present blobs re-offered directly; missing re-imported via fd.
                // No std locks held across awaits below (`parent_fd` is Send).
                let present = store.has(hash).await.map_err(|_| ())?;
                if present {
                    lock(&shared.blob_job_offers).insert((*peer, entry.hash));
                } else {
                    let _file_fd = vnilink_rename::anchored::open_file_verified(
                        parent_fd.as_fd(),
                        name,
                        st.dev,
                        st.ino,
                    )
                    .map_err(|_| ())?;
                    drop(_file_fd);
                    // Stable absolute path for TryReference (no temp fd reference):
                    // verified parent just above; serve-time fd re-verification denies
                    // if swapped later.
                    let parent_abs = {
                        let roots = lock(&shared.file_roots);
                        let abs = roots.resolve_dir(local_root, parent_rel).map_err(|_| ())?;
                        drop(roots);
                        abs
                    };
                    let abs_path = parent_abs.join(name);
                    drop(parent_fd);
                    let tag = store
                        .add_path_with_opts(AddPathOptions {
                            path: abs_path,
                            format: BlobFormat::Raw,
                            mode: ImportMode::TryReference,
                        })
                        .await
                        .map_err(|_| ())?;
                    if tag.hash != hash {
                        {
                            let mut jobs = lock(&shared.jobs);
                            if let Some(record) = wire_job_mut(&mut jobs, peer, resume.job_id) {
                                record.state = JobState::Failed;
                                record.error = Some(SOURCE_CHANGED.to_owned());
                            }
                        }
                        let _ = save_jobs(shared);
                        let err = job::JobReply {
                            v: job::JOB_VERSION,
                            id: resume.id,
                            status: job::STATUS_ERROR,
                            job_id: resume.job_id,
                        };
                        reply(send, job::encode(&err).map_err(|_| ())?).await?;
                        return Ok(());
                    }
                    lock(&shared.blob_job_offers).insert((*peer, entry.hash));
                }
            }
            #[cfg(not(unix))]
            {
                let _ = (&parent_rel, name);
                let hash = iroh_blobs::Hash::from_bytes(entry.hash);
                let present = store.has(hash).await.map_err(|_| ())?;
                if present {
                    lock(&shared.blob_job_offers).insert((*peer, entry.hash));
                } else {
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
        }
        {
            let mut jobs = lock(&shared.jobs);
            if let Some(record) = wire_job_mut(&mut jobs, peer, resume.job_id) {
                if super::service_jobs::check_owner(&lock(&shared.state), record.owner).is_err() {
                    return Err(());
                }
                record.state = JobState::Serving;
                record.error = None;
            }
        }
        let _ = save_jobs(shared);
    } else {
        let mut jobs = lock(&shared.jobs);
        if let Some(record) = wire_job_mut(&mut jobs, peer, resume.job_id) {
            if super::service_jobs::check_owner(&lock(&shared.state), record.owner).is_err() {
                return Err(());
            }
            record.state = JobState::Transferring;
            record.waiting_file = None;
            record.error = None;
        }
        drop(jobs);
        let _ = save_jobs(shared);
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

pub(super) async fn handle_upload_offer(
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
        if lock(&shared.blob_dir).is_none() {
            return Err(job::STATUS_ERROR);
        }
        if !lock(&shared.state).trust.granted(peer, Grant::Transfer) {
            return Err(job::STATUS_DENIED);
        }
        if manifest::validate(&offer.manifest).is_err() || fs::check_rel(&offer.dst_rel).is_err() {
            return Err(job::STATUS_ERROR);
        }
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
            },
        );
        Ok(())
    })();
    let _ = save_jobs(shared);
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

fn prepared_download_permitted(
    state: &State,
    peer: &[u8; 32],
    session_id: u64,
    generation: u64,
    permission_epoch: u64,
) -> bool {
    state.trust.granted(peer, Grant::Transfer)
        && *state.transfer_epochs.get(peer).unwrap_or(&0) == permission_epoch
        && matches!(state.incoming.as_ref(), Some(session)
            if session.id == session_id && session.generation == generation
            && session.phase == SessionPhase::Active && session.peer_key == *peer)
}

pub(super) async fn handle_download_request(
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
    if lock(&shared.blob_dir).is_none() {
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
    // Scan, hash and offer from this host's roots. Failures reject the
    // request; nothing is announced before every hash is served.
    let prepared = prepare_host_manifest(shared, request.src_root, &request.src_rel).await;
    let outcome: Result<(u64, Manifest, u64), u8> = match prepared {
        Err(status) => Err(status),
        Ok((id, manifest, skipped)) => {
            // Same order as publication/rename: jobs -> state. Activation
            // and permission changes cannot interleave between this check
            // and insertion. A new session cannot inherit prepared work.
            let mut jobs = lock(&shared.jobs);
            let state = lock(&shared.state);
            let permitted = prepared_download_permitted(
                &state,
                peer,
                session_id,
                request.generation,
                permission_epoch,
            );
            if !permitted || super::service_jobs::check_owner(&state, owner).is_err() {
                Err(job::STATUS_DENIED)
            } else if jobs.contains_key(&id) {
                Err(job::STATUS_ERROR)
            } else {
                let skipped_symlinks = skipped;
                for entry in manifest.entries.iter().filter(|entry| !entry.is_dir) {
                    lock(&shared.blob_job_offers).insert((*peer, entry.hash));
                }
                jobs.insert(
                    id,
                    JobRecord {
                        owner,
                        id,
                        wire_id: id,
                        created_at: retention_now(),
                        peer_key: *peer,
                        peer_id: endpoint_id_of(peer),
                        direction: JobDirection::Upload,
                        sender: true,
                        state: JobState::Serving,
                        manifest: Some(manifest.clone()),
                        local_root: request.src_root,
                        local_rel: request.src_rel.clone(),
                        outgoing: false,
                        skipped_symlinks,
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
                    },
                );
                Ok((id, manifest, skipped))
            }
        }
    };
    let _ = save_jobs(shared);
    match outcome {
        Ok((id, manifest, _)) => {
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

/// Scans and imports every file, returning the sender job id and
/// the filled manifest. Runs on the host inside its runtime.
async fn prepare_host_manifest(
    shared: &Shared,
    src_root: u64,
    src_rel: &str,
) -> Result<(u64, Manifest, u64), u8> {
    let scan = {
        let roots = lock(&shared.file_roots);
        manifest::scan(&roots, src_root, src_rel).map_err(|_| job::STATUS_ERROR)?
    };
    let mut manifest = manifest::skeleton(&scan);
    let store = super::blob::open_store(shared)
        .await
        .map_err(|_| job::STATUS_ERROR)?;
    for file in &scan.files {
        // Anchored import: re-open through handles, never the retained
        // absolute pathname.
        let fd_path = {
            let (parent_rel, name) = match file.root_rel.rfind('/') {
                Some(slash) => (&file.root_rel[..slash], &file.root_rel[slash + 1..]),
                None => ("", file.root_rel.as_str()),
            };
            // `src_root` is the scan root; `file.root_rel` is root-relative.
            // Open parent + file with NOFOLLOW while holding no other locks.
            let roots = lock(&shared.file_roots);
            #[cfg(unix)]
            {
                use std::os::unix::io::AsFd;
                let parent_fd = roots
                    .open_dir_fd(src_root, parent_rel)
                    .map_err(|_| job::STATUS_ERROR)?;
                let st = vnilink_rename::anchored::stat_at(parent_fd.as_fd(), name)
                    .map_err(|_| job::STATUS_ERROR)?;
                if !st.is_file
                    || st.is_symlink
                    || st.size != file.size
                    || st.dev != file.dev
                    || st.ino != file.ino
                    || st.mtime_ms != file.mtime_ms
                {
                    return Err(job::STATUS_ERROR);
                }
                let _file_fd = vnilink_rename::anchored::open_file_verified(
                    parent_fd.as_fd(),
                    name,
                    st.dev,
                    st.ino,
                )
                .map_err(|_| job::STATUS_ERROR)?;
                let expected_hash = Core::hash_fd(_file_fd).map_err(|_| job::STATUS_ERROR)?;
                // Stable absolute path for TryReference (no temp fd reference
                // persisted): verified parent (not symlink) just above; serve-time
                // fd re-verification denies if swapped later. No second full copy
                // when the store can reference (small/differing mounts may copy).
                (file.absolute.clone(), expected_hash, parent_fd)
            }
            #[cfg(not(unix))]
            {
                let _ = (&roots, parent_rel, name);
                (file.absolute.clone(), (), ())
            }
        };
        #[cfg(unix)]
        let (abs_path, expected_hash, _parent_fd) = fd_path;
        #[cfg(not(unix))]
        let (abs_path, _, _) = fd_path;
        #[cfg(unix)]
        drop(_parent_fd);
        let tag = store
            .add_path_with_opts(AddPathOptions {
                path: abs_path,
                format: BlobFormat::Raw,
                mode: ImportMode::TryReference,
            })
            .temp_tag()
            .await
            .map_err(|_| job::STATUS_ERROR)?;
        #[cfg(unix)]
        if tag.hash().as_bytes() != &expected_hash {
            return Err(job::STATUS_ERROR);
        }
        if tag.hash().as_bytes() == &[0u8; 32] {
            return Err(job::STATUS_ERROR);
        }
        let hash = *tag.hash().as_bytes();
        lock(&shared.blob_import_pins).insert(hash, (std::time::Instant::now(), tag));
        manifest
            .entries
            .iter_mut()
            .find(|entry| entry.path == file.rel)
            .map(|entry| {
                entry.hash = hash;
                entry.size = file.size;
            });
    }
    // Directory sources keep their top folder; hash filling above matches
    // scan paths, so prefixing lands after.
    {
        let roots = lock(&shared.file_roots);
        if roots.resolve_dir(src_root, src_rel).is_ok() {
            manifest::prefix_source_dir(src_rel, &mut manifest);
        }
    }
    if manifest
        .entries
        .iter()
        .filter(|entry| !entry.is_dir)
        .any(|entry| entry.hash == [0; 32])
    {
        return Err(job::STATUS_ERROR);
    }
    manifest::validate(&manifest).map_err(|_| job::STATUS_ERROR)?;
    // Sender job ids minted on the host share the local allocator, so
    // adopted controller ids can never collide with a later mint.
    let id = allocate_job_id(shared).map_err(|_| job::STATUS_ERROR)?;
    Ok((id, manifest, scan.skipped_symlinks))
}

fn endpoint_id_of(peer: &[u8; 32]) -> String {
    iroh::PublicKey::from_bytes(peer)
        .map(|key| key.to_string())
        .unwrap_or_default()
}

#[cfg(test)]
mod lifecycle_tests {
    use super::*;

    #[test]
    fn prepared_download_cannot_survive_permission_toggle_or_new_generation() {
        let boot = || {
            Core::boot(
                CoreConfig {
                    listen: Listen::Loopback,
                    ..CoreConfig::default()
                },
                iroh::RelayMode::Disabled,
            )
            .unwrap()
        };
        let host = boot();
        let controller = boot();
        host.set_h264_support(true, true).unwrap();
        controller.set_h264_support(true, true).unwrap();
        host.start_hosting().unwrap();
        controller
            .accept_invitation(
                &host
                    .create_invitation(std::time::Duration::from_secs(60))
                    .unwrap(),
            )
            .unwrap();
        host.accept_invitation(
            &controller
                .create_invitation(std::time::Duration::from_secs(60))
                .unwrap(),
        )
        .unwrap();
        let peer = parse_endpoint_id(&controller.endpoint_id()).unwrap();
        let grants = crate::session::Grants {
            control: true,
            clipboard: false,
            transfer: true,
        };
        host.authorize(&controller.endpoint_id(), grants).unwrap();
        controller.connect(&host.endpoint_id()).unwrap();
        let (id, generation, epoch) = {
            let state = lock(&host.shared.state);
            let session = state.incoming.as_ref().unwrap();
            let epoch = *state.transfer_epochs.get(&peer).unwrap();
            assert!(prepared_download_permitted(
                &state,
                &peer,
                session.id,
                session.generation,
                epoch
            ));
            (session.id, session.generation, epoch)
        };
        host.authorize(&controller.endpoint_id(), crate::session::Grants::default())
            .unwrap();
        host.authorize(&controller.endpoint_id(), grants).unwrap();
        let state = lock(&host.shared.state);
        assert!(!prepared_download_permitted(
            &state, &peer, id, generation, epoch
        ));
        let current = *state.transfer_epochs.get(&peer).unwrap();
        assert!(prepared_download_permitted(
            &state, &peer, id, generation, current
        ));
        assert!(!prepared_download_permitted(
            &state,
            &peer,
            id,
            generation + 1,
            current
        ));
        drop(state);
        host.shutdown();
        controller.shutdown();
    }

    fn record(id: u64, size: u64, age: u64, state: JobState) -> JobRecord {
        JobRecord {
            owner: None,
            id,
            wire_id: id,
            created_at: retention_now().saturating_sub(age),
            peer_key: [0; 32],
            peer_id: String::new(),
            direction: JobDirection::Download,
            sender: false,
            state,
            manifest: Some(Manifest {
                entries: vec![ManifestEntry {
                    path: format!("file-{id}"),
                    hash: [id as u8; 32],
                    size,
                    source_mtime_ms: 0,
                    is_dir: false,
                }],
            }),
            local_root: 0,
            local_rel: String::new(),
            outgoing: false,
            file_index: 0,
            done_bytes: 0,
            files_done: 0,
            resumed_files: 0,
            skipped_files: 0,
            waiting_file: None,
            skipped_symlinks: 0,
            landed: HashMap::new(),
            choices: HashMap::new(),
            apply_remaining: None,
            error: None,
        }
    }

    #[test]
    fn retention_bounds_inactive_data_and_preserves_active_shared_hashes() {
        let gib = 1024 * 1024 * 1024;
        let jobs = HashMap::from([
            (1, record(1, 3 * gib, 20, JobState::Cancelled)),
            (2, record(2, 3 * gib, 10, JobState::Interrupted)),
            (
                3,
                record(3, 100, RESUME_CACHE_AGE + 1, JobState::Interrupted),
            ),
            (4, record(4, 100, 0, JobState::Complete)),
            (5, record(5, 100, 0, JobState::Failed)),
            (
                6,
                record(6, 8 * gib, RESUME_CACHE_AGE + 1, JobState::Transferring),
            ),
        ]);
        let live = retained_hashes(&jobs);
        assert_eq!(
            live,
            std::collections::HashSet::from([
                iroh_blobs::Hash::from_bytes([2; 32]),
                iroh_blobs::Hash::from_bytes([6; 32])
            ])
        );
        let owners = retained_job_ids(&jobs);
        assert!(record_reclaimable(&jobs[&1], &owners));
        assert!(record_reclaimable(&jobs[&3], &owners));
        assert!(record_reclaimable(&jobs[&4], &owners));
        assert!(!record_reclaimable(&jobs[&6], &owners));
        let mut complete = jobs[&6].clone();
        complete.state = JobState::Complete;
        // Shared hash protection remains independent of reclaiming another job's staging.
        complete.id = 7;
        assert!(record_reclaimable(&complete, &owners));
        assert!(live.contains(&iroh_blobs::Hash::from_bytes([6; 32])));
    }
}

pub(super) fn release_prepared_offers(shared: &Shared, peer: &[u8; 32], hashes: &[[u8; 32]]) {
    let jobs = lock(&shared.jobs);
    let mut offers = lock(&shared.blob_job_offers);
    for hash in hashes {
        let needed = jobs.values().any(|r| {
            r.peer_key == *peer
                && r.sender
                && !r.state.is_terminal()
                && record_hashes(r).contains(hash)
        });
        if !needed {
            offers.remove(&(*peer, *hash));
        }
    }
}
