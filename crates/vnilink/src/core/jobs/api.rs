// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! The job API a window drives: submit, list, poll, cancel, resume, resolve, forget.

use super::*;

impl Core {
    /// Uploads the entries `names` of `src_folder`, or with no names the
    /// whole root, as one job of the Files service `owner`: scans, hashes,
    /// offers and sends the manifest. The returned id is the sender
    /// record; the peer adopts the same id for its receiver record.
    #[allow(clippy::too_many_arguments)]
    pub(in crate::core) fn submit_upload(
        &self,
        peer_endpoint_id: &str,
        src_root: u64,
        src_folder: &str,
        names: &[String],
        dst_root: u64,
        dst_rel: &str,
        owner: super::service_jobs::JobOwner,
    ) -> Result<u64, Error> {
        let _engine = self.engine();
        let service = owner;
        let owner = Some(service);
        let peer = parse_endpoint_id(peer_endpoint_id)?;
        self.ensure_alive()?;
        check_sources(src_folder, names)?;
        Self::check_job_capacity(&self.shared)?;
        {
            let state = lock(&self.shared.state);
            state.trust.get(&peer).cloned().ok_or(Error::NotPaired)?;
        }
        fs::check_rel(dst_rel)?;
        // The live session first: no scanning happens without one.
        // The host re-fences the generation on arrival.
        let generation = self.generation_for(&peer, true, owner)?;
        super::service_jobs::check_owner(&lock(&self.shared.state), owner)?;
        // Snapshot under a short lock; the recursive walk runs off every lock.
        let snapshot = lock(&self.shared.file_roots).clone();
        // Pin the leg to the scanned generation: each part re-verifies it,
        // so a drive replaced during admission fails the whole offer instead of
        // mixing generations.
        let mut pin = Some(snapshot.pin(src_root)?);
        let mut manifest = Manifest::new(Vec::new());
        let mut skipped_symlinks = 0;
        let mut scanned = HashMap::new();
        for rel in source_rels(src_folder, names) {
            let scan = manifest::scan(&snapshot, src_root, &rel)?;
            skipped_symlinks += scan.skipped_symlinks;
            scanned.extend(
                scan.files
                    .iter()
                    .map(|file| (file.root_rel.clone(), file.ino)),
            );
            let mut part = manifest::skeleton(&scan);
            super::service_jobs::check_owner(&lock(&self.shared.state), owner)?;
            {
                // A directory source keeps its top folder; files stay flat.
                let roots = lock(&self.shared.file_roots);
                verify_leg_pin(&roots, src_root, &mut pin)?;
                if roots.resolve_dir(src_root, &rel).is_ok() {
                    manifest::prefix_source_dir(&rel, &mut part);
                }
            }
            manifest.entries.append(&mut part.entries);
            if manifest.entries.len() > manifest::MAX_MANIFEST_FILES {
                return Err(Error::InvalidSetting(manifest::MANIFEST_LIMIT));
            }
        }
        manifest.entries.sort_by(|a, b| a.path.cmp(&b.path));
        manifest::validate(&manifest)?;
        let source_ids = source_ids(&manifest, src_folder, &scanned);
        let id = allocate_job_id(&self.shared)?;
        let record = JobRecord {
            owner,
            destination: Some(Landing {
                root: dst_root,
                folder: dst_rel.to_owned(),
                names: names.to_vec(),
            }),
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
            local_rel: src_folder.to_owned(),
            names: names.to_vec(),
            local_pin: pin,
            outgoing: true,
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
            in_flight: 0,
            attempt: 0,
            unsaved_landed: Vec::new(),
            retry: None,
            telling_resume: false,
            sent_at: None,
            source_ids,
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
        // Serve before announcing: the receiver may ask for the first file
        // as soon as it accepts, before its reply reaches this side.
        {
            let mut jobs = lock(&self.shared.jobs);
            let state = lock(&self.shared.state);
            super::service_jobs::check_owner(&state, owner)?;
            let mut record = record;
            // Re-verify before announcing: a drive replaced between the last file
            // and the insert fails here instead of offering moved bytes.
            verify_leg_pin(
                &lock(&self.shared.file_roots),
                record.local_root,
                &mut record.local_pin,
            )?;
            record.enter(JobState::Serving);
            jobs.insert(id, record);
        }
        let announced = (|| {
            let reply_bytes = self.service_job_round_trip(
                service,
                &peer,
                job::TAG_JOB_UPLOAD,
                &job::encode(&offer)?,
            )?;
            let reply: job::JobReply = job::decode(&reply_bytes)?;
            if reply.id != offer.id {
                return Err(Error::Malformed);
            }
            match reply.status {
                status if status == job::STATUS_OK => Ok(()),
                status => Err(job::error_for_status(status)),
            }
        })();
        if let Err(error) = announced {
            // An offer the receiver did not take leaves nothing to serve.
            lock(&self.shared.jobs).remove(&id);
            return Err(error);
        }
        save_jobs(&self.shared)?;
        Ok(id)
    }

    /// Downloads the peer's entries `names` of `src_folder`, or with no
    /// names its whole root, as one job of the Files service `owner`. The
    /// host scans, hashes and offers; this side adopts the host's job id
    /// for its receiver record.
    #[allow(clippy::too_many_arguments)]
    pub(in crate::core) fn submit_download(
        &self,
        peer_endpoint_id: &str,
        src_root: u64,
        src_folder: &str,
        names: &[String],
        dst_root: u64,
        dst_rel: &str,
        owner: super::service_jobs::JobOwner,
    ) -> Result<u64, Error> {
        let _engine = self.engine();
        let service = owner;
        let owner = Some(service);
        let peer = parse_endpoint_id(peer_endpoint_id)?;
        self.ensure_alive()?;
        check_sources(src_folder, names)?;
        Self::check_job_capacity(&self.shared)?;
        {
            let state = lock(&self.shared.state);
            state.trust.get(&peer).cloned().ok_or(Error::NotPaired)?;
        }
        fs::check_rel(dst_rel)?;
        // Pin the destination leg at admission: every receive below
        // re-verifies it, so a replaced target drive reroutes no bytes.
        let pin = {
            let roots = lock(&self.shared.file_roots);
            if roots.snapshot().iter().all(|(id, _)| *id != dst_root) {
                return Err(Error::InvalidSetting("the destination root is unknown"));
            }
            Some(roots.pin(dst_root)?)
        };
        // The live session first, as in uploads.
        let generation = self.generation_for(&peer, true, owner)?;
        let request = job::DownloadRequest {
            v: job::JOB_VERSION,
            id: 1,
            generation,
            src_root,
            src_folder: src_folder.to_owned(),
            names: names.to_vec(),
        };
        let reply_bytes = self.service_job_round_trip(
            service,
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
        // A preparation still under way leaves the job Preparing; its
        // manifest is asked for until ready (`poll_receiver`).
        if let Some(manifest) = &reply.manifest {
            manifest::validate(manifest)?;
        }
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
                    destination: Some(Landing {
                        root: dst_root,
                        folder: dst_rel.to_owned(),
                        names: names.to_vec(),
                    }),
                    id,
                    wire_id: reply.job_id,
                    created_at: retention_now(),
                    peer_key: peer,
                    peer_id: peer_endpoint_id.to_owned(),
                    direction: JobDirection::Download,
                    sender: false,
                    state: if reply.manifest.is_some() {
                        JobState::Transferring
                    } else {
                        JobState::Preparing
                    },
                    manifest: reply.manifest,
                    local_root: dst_root,
                    local_rel: dst_rel.to_owned(),
                    names: Vec::new(),
                    local_pin: pin,
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
                    in_flight: 0,
                    attempt: 0,
                    unsaved_landed: Vec::new(),
                    retry: None,
                    telling_resume: false,
                    sent_at: None,
                    source_ids: Vec::new(),
                },
            );
        }
        save_jobs(&self.shared)?;
        Ok(id)
    }

    /// Snapshots one job record.
    pub fn job_snapshot(&self, id: u64) -> Result<JobSnapshot, Error> {
        let _engine = self.engine();
        self.ensure_alive()?;
        lock(&self.shared.jobs)
            .get(&id)
            .map(JobRecord::snapshot)
            .ok_or(Error::InvalidSetting(NO_JOB))
    }

    /// Snapshots all job records by id.
    pub fn job_list(&self) -> Result<Vec<JobSnapshot>, Error> {
        let _engine = self.engine();
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
        let _engine = self.engine();
        self.ensure_alive()?;
        super::service_jobs::interrupt_orphaned(&self.shared);
        let sender = {
            let jobs = lock(&self.shared.jobs);
            let record = jobs.get(&id).ok_or(Error::InvalidSetting(NO_JOB))?;
            if record.state.is_terminal()
                || record.state == JobState::Interrupted
                || (record.state == JobState::WaitingDecision && !record.sender)
            {
                return Ok(record.snapshot());
            }
            // A retry waits out its pause.
            if record
                .retry
                .as_ref()
                .is_some_and(|retry| retry.next_at > std::time::Instant::now())
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
        struct Driving<'a>(&'a Shared, u64);
        impl Drop for Driving<'_> {
            fn drop(&mut self) {
                lock(&self.0.job_driving).remove(&self.1);
            }
        }
        let _driving = Driving(&self.shared, id);
        let polled = if sender {
            self.poll_sender(id)
        } else {
            self.poll_receiver(id)
        };
        match polled {
            // Its Files owner went during the step: the job is interrupted,
            // saying why, not failed.
            Err(Error::NoSession | Error::StaleSession) => {
                super::service_jobs::interrupt_orphaned(&self.shared);
                match self.job_snapshot(id) {
                    Ok(snapshot) if snapshot.state == JobState::Interrupted => Ok(snapshot),
                    _ => polled,
                }
            }
            polled => polled,
        }
    }

    /// Advances every active job by one step (receivers fetch/publish,
    /// senders query receiver state). Used by the background driver and by
    /// native windows; receivers waiting for a decision are skipped.
    /// Senders keep polling so receiver-side decisions cannot strand them.
    /// Runs on the caller's thread and blocks like `job_poll`.
    pub fn drive_jobs(&self) -> Result<Vec<JobSnapshot>, Error> {
        let _engine = self.engine();
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

    /// Starts the background transfer driver, independent of any Files
    /// window: closing every window pauses window ticks but not this
    /// driver, and jobs keep their state and resume. Each active job gets a
    /// worker of its own for as long as it has a step to take, so a large
    /// file for one job never holds up another. A receiver takes file after
    /// file until it is blocked, on a decision, on admission or on its peer,
    /// then waits for any job to change. A sender asks its receiver how far
    /// it got, soon again while that changes and ever more seldom while it
    /// does not (`SENDER_SOON`, `SENDER_SELDOM`); its own answers wake
    /// nothing. The driver only starts workers, when a job changes and once
    /// a second. Call once after `set_transfer_directory` from the owning
    /// `Arc<Core>`; the driver and its workers end when the Core shuts down
    /// or drops.
    pub fn start_job_driver(self: &std::sync::Arc<Self>) {
        let _engine = self.engine();
        const IDLE: Duration = Duration::from_secs(1);
        let weak = std::sync::Arc::downgrade(self);
        let shared = self.shared.clone();
        std::thread::spawn(crate::trace::within(self.shared.span.clone(), move || {
            let mut seen = shared.job_revision.load(Ordering::Acquire);
            loop {
                let Some(core) = weak.upgrade() else {
                    break;
                };
                if lock(&core.shared.state).shut_down {
                    break;
                }
                core.start_job_workers();
                drop(core);
                seen = wait_for_job_change(&shared, seen, IDLE);
            }
        }));
    }

    /// Gives each job with a step to take a worker, unless it has one.
    pub(super) fn start_job_workers(self: &std::sync::Arc<Self>) {
        let ids: Vec<u64> = {
            let jobs = lock(&self.shared.jobs);
            let mut workers = lock(&self.shared.job_workers);
            jobs.values()
                .filter(|record| record.drivable() && workers.insert(record.id))
                .map(|record| record.id)
                .collect()
        };
        for id in ids {
            let weak = std::sync::Arc::downgrade(self);
            let shared = self.shared.clone();
            let span = shared.span.clone();
            std::thread::spawn(crate::trace::within(span, move || {
                work_job(weak, shared, id)
            }));
        }
    }

    /// Takes a receiver's steps until one moves it no further: it waits on
    /// a decision, on admission or on its peer, or it ended.
    pub(super) fn drive_receiver(&self, id: u64) {
        let Ok(mut before) = self.job_snapshot(id) else {
            return;
        };
        loop {
            let Ok(after) = self.job_poll(id) else {
                return;
            };
            let advanced =
                after.files_done + after.skipped_files > before.files_done + before.skipped_files;
            if after.state != JobState::Transferring || !advanced {
                return;
            }
            before = after;
        }
    }

    /// Advances whenever any job changes.
    pub fn job_revision(&self) -> u64 {
        let _engine = self.engine();
        self.shared.job_revision.load(Ordering::Acquire)
    }

    pub(super) fn check_job_capacity(shared: &Shared) -> Result<(), Error> {
        let active = lock(&shared.jobs)
            .values()
            .filter(|record| ACTIVE_STATES.contains(&record.state))
            .count();
        if active >= MAX_ACTIVE_JOBS {
            return Err(Error::Busy);
        }
        Ok(())
    }

    /// Cancels one job. The partial file stays for an explicit resume; the
    /// destination never holds a partial file. Cancellation is symmetric:
    /// the peer is notified best-effort so its record stops as well. A
    /// cancelled sender serves nothing more, and the receiver re-checks
    /// cancellation before every publication, so a sender cancel still
    /// prevents delivery even when bytes already arrived.
    pub fn job_cancel(&self, id: u64) -> Result<JobSnapshot, Error> {
        let _engine = self.engine();
        self.ensure_alive()?;
        let (peer_key, outgoing) = {
            let mut jobs = lock(&self.shared.jobs);
            let record = jobs.get_mut(&id).ok_or(Error::InvalidSetting(NO_JOB))?;
            if !record.state.is_terminal() {
                record.enter(JobState::Cancelled);
                record.error = None;
            }
            (record.peer_key, record.outgoing)
        };
        if let Some(stop) = lock(&self.shared.job_cancel).remove(&id) {
            crate::ignored!(
                crate::trace::FILES,
                "a fetch that already ended needs no stop",
                stop.send(true)
            );
        }
        stop_prefetch(&self.shared, id);
        // Best-effort peer notification: without a live session the local
        // record is still cancelled and serves nothing, so a later status
        // poll or resume observes the terminal state.
        told_peer(
            "cancel",
            id,
            self.notify_peer_cancel(&peer_key, outgoing, id),
        );
        save_jobs(&self.shared)?;
        self.job_snapshot(id)
    }

    pub(super) fn notify_peer_cancel(
        &self,
        peer: &[u8; 32],
        outgoing: bool,
        job_id: u64,
    ) -> Result<(), Error> {
        let local = job_id;
        let (job_id, owner) = {
            let jobs = lock(&self.shared.jobs);
            let record = jobs.get(&job_id).ok_or(Error::InvalidSetting(NO_JOB))?;
            (wire_job_id(record), record.owner)
        };
        let generation = self.generation_for(peer, outgoing, owner)?;
        let cancel = job::JobCancel {
            v: job::JOB_VERSION,
            id: 1,
            generation,
            job_id,
        };
        let payload = job::encode(&cancel)?;
        // The reply is an ack; any failure (no session, stale generation,
        // denied) still leaves the local cancellation in place.
        told_peer(
            "cancel",
            local,
            self.job_round_trip_side(peer, outgoing, owner, job::TAG_JOB_CANCEL, &payload)
                .map(drop),
        );
        Ok(())
    }

    /// Resumes an interrupted, cancelled or failed job, coordinating across
    /// endpoints after symmetric cancellation: the local record resumes (sender
    /// re-offers through handles, receiver returns to `Transferring`) and the
    /// peer is notified best-effort to resume and reauthorize its offers, so
    /// retry works from either side. Source changes fail instead of moving
    /// different bytes.
    pub fn job_resume(&self, id: u64) -> Result<JobSnapshot, Error> {
        let _engine = self.engine();
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
            match self.resume_sender(id) {
                Ok(snapshot) => snapshot,
                Err(error) => {
                    self.told_resume(id);
                    return Err(error);
                }
            }
        } else {
            let mut jobs = lock(&self.shared.jobs);
            let record = jobs.get_mut(&id).ok_or(Error::InvalidSetting(NO_JOB))?;
            let state = lock(&self.shared.state);
            super::service_jobs::check_owner(&state, record.owner)?;
            record.enter(if record.manifest.is_some() {
                JobState::Transferring
            } else {
                JobState::Preparing
            });
            record.waiting_file = None;
            record.error = None;
            drop(state);
            drop(jobs);
            save_jobs(&self.shared)?;
            self.job_snapshot(id)?
        };
        // Best-effort peer coordination: without a live session the local
        // resume still stands and a later status poll reconciles.
        told_peer(
            "resume",
            id,
            self.notify_peer_resume(&peer_key, outgoing, id),
        );
        if sender {
            self.told_resume(id);
        }
        Ok(snapshot)
    }

    /// A sender's resume has been told to its receiver, or will not be:
    /// it follows its receiver again.
    pub(super) fn told_resume(&self, id: u64) {
        if let Some(record) = lock(&self.shared.jobs).get_mut(&id) {
            record.telling_resume = false;
        }
    }

    pub(super) fn notify_peer_resume(
        &self,
        peer: &[u8; 32],
        outgoing: bool,
        job_id: u64,
    ) -> Result<(), Error> {
        let local = job_id;
        let (job_id, owner) = {
            let jobs = lock(&self.shared.jobs);
            let record = jobs.get(&job_id).ok_or(Error::InvalidSetting(NO_JOB))?;
            (wire_job_id(record), record.owner)
        };
        let generation = self.generation_for(peer, outgoing, owner)?;
        let resume = job::JobResume {
            v: job::JOB_VERSION,
            id: 1,
            generation,
            job_id,
        };
        let payload = job::encode(&resume)?;
        told_peer(
            "resume",
            local,
            self.job_round_trip_side(peer, outgoing, owner, job::TAG_JOB_RESUME, &payload)
                .map(drop),
        );
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
        let _engine = self.engine();
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
            record.enter(JobState::Transferring);
            record.waiting_file = None;
            (record.sender, record.peer_key, record.outgoing)
        };
        save_jobs(&self.shared)?;
        if sender {
            told_peer(
                "choice",
                id,
                self.notify_peer_resolve(&peer_key, outgoing, id, choice, apply_to_remaining),
            );
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
        let _engine = self.engine();
        self.ensure_alive()?;
        {
            let jobs = lock(&self.shared.jobs);
            let record = jobs.get(&id).ok_or(Error::InvalidSetting(NO_JOB))?;
            match record.state {
                JobState::Complete | JobState::Cancelled | JobState::Failed => {}
                _ => {
                    return Err(Error::InvalidSetting(
                        "only finished jobs can be forgotten; cancel or complete first",
                    ));
                }
            }
        }
        // Release staging: Complete already removed it; Cancelled may keep
        // a partial for resume, which forgetting releases.
        remove_staging(&self.shared, id);
        lock(&self.shared.jobs).remove(&id);
        save_jobs(&self.shared)?;
        Ok(())
    }

    pub(super) fn notify_peer_resolve(
        &self,
        peer: &[u8; 32],
        outgoing: bool,
        job_id: u64,
        choice: ConflictChoice,
        apply_to_remaining: bool,
    ) -> Result<(), Error> {
        let local = job_id;
        let (job_id, owner) = {
            let jobs = lock(&self.shared.jobs);
            let record = jobs.get(&job_id).ok_or(Error::InvalidSetting(NO_JOB))?;
            (wire_job_id(record), record.owner)
        };
        let generation = self.generation_for(peer, outgoing, owner)?;
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
        told_peer(
            "choice",
            local,
            self.job_round_trip_side(peer, outgoing, owner, job::TAG_JOB_RESOLVE, &payload)
                .map(drop),
        );
        Ok(())
    }

    /// The wire generation of the session a job with `peer` and `owner`
    /// rides on this side ([`super::service_jobs::job_session`]).
    pub(super) fn generation_for(
        &self,
        peer: &[u8; 32],
        outgoing: bool,
        owner: Option<super::service_jobs::JobOwner>,
    ) -> Result<u64, Error> {
        let state = self.alive_state()?;
        let session = super::service_jobs::job_session(&state, peer, outgoing, owner)
            .ok_or(Error::NoSession)?;
        if session.phase != crate::session::SessionPhase::Active {
            return Err(Error::NoSession);
        }
        Ok(session.wire_stamp.generation)
    }

    /// One job-control round trip on the session the job rides.
    pub(in crate::core) fn job_round_trip_side(
        &self,
        peer: &[u8; 32],
        outgoing: bool,
        owner: Option<super::service_jobs::JobOwner>,
        tag: u8,
        payload: &[u8],
    ) -> Result<Vec<u8>, Error> {
        self.job_round_trip_side_owned(peer, outgoing, tag, payload, owner, false)
    }

    /// As [`Core::job_round_trip_side`]; `leased` also holds the owning
    /// service's lease across it, bounded in time, as a submission does.
    pub(in crate::core) fn job_round_trip_side_owned(
        &self,
        peer: &[u8; 32],
        outgoing: bool,
        tag: u8,
        payload: &[u8],
        owner: Option<super::service_jobs::JobOwner>,
        leased: bool,
    ) -> Result<Vec<u8>, Error> {
        let mut lease = owner
            .filter(|_| leased)
            .map(|owner| self.service_lease(owner.id, owner.generation))
            .transpose()?;
        // Senders poll over the session facing the peer: outgoing for
        // controller senders, incoming for host senders, the one the job
        // rides each time.
        let connection = {
            let state = self.alive_state()?;
            let session = super::service_jobs::job_session(&state, peer, outgoing, owner)
                .ok_or(Error::NoSession)?;
            if session.phase != crate::session::SessionPhase::Active {
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
