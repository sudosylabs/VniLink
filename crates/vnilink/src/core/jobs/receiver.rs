// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! The receiving side of a job: asking for files, placing them, retrying and ending.

use super::*;

impl Core {
    pub(super) fn poll_receiver(&self, id: u64) -> Result<JobSnapshot, Error> {
        // Cancellation or connection interruption stops before any fetch.
        {
            let jobs = lock(&self.shared.jobs);
            let record = jobs.get(&id).ok_or(Error::InvalidSetting(NO_JOB))?;
            if matches!(record.state, JobState::Cancelled | JobState::Interrupted) {
                return Ok(record.snapshot());
            }
            if record.manifest.is_none() {
                drop(jobs);
                return self.ask_prepared_manifest(id);
            }
        }
        let (wire_job, peer_bytes, file_index, file_count) = {
            let jobs = lock(&self.shared.jobs);
            let record = jobs.get(&id).ok_or(Error::InvalidSetting(NO_JOB))?;
            (
                wire_job_id(record),
                record.peer_key,
                record.file_index,
                record.file_count(),
            )
        };
        if file_index >= file_count {
            // Zero-file manifests still carry directory entries: build the
            // skeleton before claiming completion so empty folders land.
            // Revocation stops here: no new directories after the grant is gone.
            if !Self::receiver_granted(&self.shared, &peer_bytes) {
                return self.mark_interrupted(id, PERMISSION_OFF);
            }
            if let Err(error) = self.ensure_receiver_dirs(id) {
                return self.interrupt_on_drive_change(id, error);
            }
            match self.finalize_receiver(id) {
                Err(error) => return self.interrupt_on_drive_change(id, error),
                Ok(snapshot) => return Ok(snapshot),
            }
        }
        // First poll ensures the directory skeleton exactly once per job;
        // creation is idempotent, so repeats stay safe. Revocation stops
        // directory creation as well as file publication.
        if file_index == 0 {
            if !Self::receiver_granted(&self.shared, &peer_bytes) {
                return self.mark_interrupted(id, PERMISSION_OFF);
            }
            if let Err(error) = self.ensure_receiver_dirs(id) {
                return self.interrupt_on_drive_change(id, error);
            }
        }
        let entry = {
            let jobs = lock(&self.shared.jobs);
            let record = jobs.get(&id).ok_or(Error::InvalidSetting(NO_JOB))?;
            record.file(file_index).cloned()
        }
        .ok_or(Error::InvalidSetting(NO_JOB))?;
        // Conflict check before any fetch: the destination wins ties until
        // the owner decides.
        let choice = match self.conflict_choice(id, file_index, &entry) {
            Err(error) => return self.interrupt_on_drive_change(id, error),
            Ok(choice) => choice,
        };
        let Some(choice) = choice else {
            let mut jobs = lock(&self.shared.jobs);
            let record = jobs.get_mut(&id).ok_or(Error::InvalidSetting(NO_JOB))?;
            if matches!(record.state, JobState::Cancelled | JobState::Interrupted) {
                return Ok(record.snapshot());
            }
            record.enter(JobState::WaitingDecision);
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
            record.in_flight = 0;
            record.file_index += 1;
            drop(jobs);
            save_jobs(&self.shared)?;
            return self.job_snapshot(id);
        }
        // Fetch under the receiver's own grant: revocation stops new
        // reads here even while the sender still serves.
        if !Self::receiver_granted(&self.shared, &peer_bytes) {
            return self.mark_interrupted(id, PERMISSION_OFF);
        }
        // Admission precedes network fetching and export. Saturated workers
        // leave the job queued for a subsequent poll; impossible budgets fail
        // with a durable, payload-free explanation.
        let available = match self.receiver_available_storage(id, &entry.path) {
            Ok(available) => available,
            Err(Error::InvalidSetting(msg)) if msg == fs::DRIVE_CHANGED => {
                return self.mark_interrupted(id, fs::DRIVE_CHANGED);
            }
            // The destination folder is gone or unreadable: say so on the
            // job rather than failing every poll.
            Err(Error::InvalidSetting(msg)) => return self.mark_failed(id, msg),
            Err(error) => return Err(error),
        };
        let _reservation =
            match super::file_resources::reserve(&self.shared, id, entry.size, available) {
                Ok(Some(reservation)) => reservation,
                Ok(None) => return self.job_snapshot(id),
                Err(Error::InvalidSetting(message)) => return self.mark_failed(id, message),
                Err(error) => return Err(error),
            };
        // Fetch into the file's partial with a per-poll cancel watch.
        let (stop_tx, stop_rx) = tokio::sync::watch::channel(false);
        // The run this fetch belongs to: what it finds is applied only while
        // the job is still that run.
        let attempt = {
            let jobs = lock(&self.shared.jobs);
            let record = jobs.get(&id).ok_or(Error::InvalidSetting(NO_JOB))?;
            let state = lock(&self.shared.state);
            if matches!(record.state, JobState::Cancelled | JobState::Interrupted) {
                return Ok(record.snapshot());
            }
            super::service_jobs::check_owner(&state, record.owner)?;
            lock(&self.shared.job_cancel).insert(id, stop_tx);
            record.attempt
        };
        // A partial kept from an earlier attempt resumes where it ended.
        let (staged, segments) = self.stage_paths(id, &entry.path)?;
        // The journal makes its own folder, when a file has one at all.
        if let Some(parent) = staged.parent() {
            std::fs::create_dir_all(parent).map_err(|_| Error::InvalidSetting(STAGE_FAILED))?;
        }
        // A fetch stopped with the job's last run may still be writing a
        // partial: it ends before this run writes any.
        for stopped in take_stopped_prefetch(&self.shared, id) {
            crate::ignored!(
                crate::trace::FILES,
                "a stopped fetch that panicked left a partial, checked again when used",
                self.runtime.block_on(stopped)
            );
        }
        // This file may already be arriving: take it before the next small
        // files start, so no second fetch ever writes its partial.
        let ahead = lock(&self.shared.job_prefetch)
            .get_mut(&id)
            .and_then(|prefetch| prefetch.files.remove(&file_index));
        self.prefetch_after(id, &peer_bytes, wire_job, file_index);
        let arrived = ahead
            .map(|(_, fetching)| self.runtime.block_on(fetching))
            .and_then(|joined| joined.ok());
        let file = u32::try_from(file_index).map_err(|_| Error::InvalidSetting(NO_JOB))?;
        // A file fetched ahead is used only when it arrived whole, on the
        // connection still kept: revoking closes it, and a file that came
        // before must not land after. Anything else is fetched again here,
        // with this poll's retries and cancel.
        let fetch = match arrived {
            Some((Ok(done @ Received::Done { .. }), Some(via)))
                if super::transfer::still_connected(&self.shared, &peer_bytes, via) =>
            {
                Ok(done)
            }
            _ => {
                let (progress, arriving) = std::sync::mpsc::channel();
                let watcher =
                    follow_arrival(self.shared.jobs.clone(), self.shared.clone(), id, arriving);
                let fetched = self.fetch_job_file(
                    &peer_bytes,
                    wire_job,
                    file,
                    entry.size,
                    &staged,
                    &segments,
                    progress,
                    stop_rx,
                );
                if watcher.join().is_err() {
                    tracing::warn!(target: crate::trace::FILES, job = id, "a transfer's progress watcher panicked");
                }
                fetched
            }
        };
        // A cancel or interruption during fetch preserves its state even if
        // bytes arrived; a job that stopped and runs again is a later run's,
        // whose cancel handle this fetch leaves in place.
        {
            let jobs = lock(&self.shared.jobs);
            let record = jobs.get(&id).ok_or(Error::InvalidSetting(NO_JOB))?;
            if record.attempt != attempt {
                return Ok(record.snapshot());
            }
            lock(&self.shared.job_cancel).remove(&id);
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
            return self.mark_interrupted(id, PERMISSION_OFF);
        }
        use crate::transfer::Received;
        let downloaded = match fetch {
            Ok(Received::Done { downloaded }) => downloaded,
            // A sender stops serving only when its job stopped or its
            // permission or folder went: it will not serve again by itself.
            Ok(Received::Denied) => return self.mark_interrupted(id, SENDER_STOPPED),
            Ok(Received::ShutDown) => return self.retry_later(id, SENDER_SHUT_DOWN),
            Ok(Received::Stalled) => return self.retry_later(id, STALLED),
            Ok(Received::Busy) => return self.retry_later(id, SENDER_BUSY),
            Err(Error::Disk(fault)) => return self.mark_failed(id, fault.message()),
            Err(_) => return self.retry_later(id, UNREACHABLE),
            Ok(Received::Changed) => return self.mark_failed(id, SOURCE_CHANGED),
            Ok(Received::Mismatch) => return self.mark_failed(id, VERIFY_FAILED),
            // A stopped fetch belongs to a job still cancelled or interrupted,
            // or to an earlier run: both returned above.
            Ok(Received::Cancelled) => return self.job_snapshot(id),
        };
        // The partial holds the verified file: publish it per the decision.
        let staged_size = std::fs::metadata(&staged)
            .map(|meta| meta.len())
            .unwrap_or(u64::MAX);
        if staged_size != entry.size {
            crate::ignored!(
                crate::trace::FILES,
                "a staged file left behind is swept with its job's staging",
                std::fs::remove_file(&staged)
            );
            return self.mark_failed(id, VERIFY_FAILED);
        }
        let landed = match self.publish_staged(id, &entry, &staged, choice) {
            Err(error) => return self.interrupt_on_drive_change(id, error),
            Ok(Some(landed)) => landed,
            Ok(None) => return self.job_snapshot(id),
        };
        // The partial is published; its journal describes nothing now.
        if entry.size > crate::transfer::SEGMENT_LEN {
            crate::ignored!(
                crate::trace::FILES,
                "a staged file left behind is swept with its job's staging",
                std::fs::remove_file(&segments)
            );
        }
        // Skip leaves the destination untouched; anything published lands
        // under its final name for verification.
        {
            let mut jobs = lock(&self.shared.jobs);
            let record = jobs.get_mut(&id).ok_or(Error::InvalidSetting(NO_JOB))?;
            let at = record.file_index;
            if choice != ConflictChoice::Skip {
                record.land(at, landed);
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
            Self::retry_settled(record);
            record.files_done += 1;
            record.done_bytes = record.done_bytes.saturating_add(entry.size);
            record.in_flight = 0;
            record.file_index += 1;
            drop(jobs);
        }
        save_jobs(&self.shared)?;
        self.job_snapshot(id)
    }

    /// Asks the host whether a download it is preparing is ready, and
    /// takes its manifest once it is. Still preparing leaves the job as it
    /// is; a refusal fails it.
    pub(super) fn ask_prepared_manifest(&self, id: u64) -> Result<JobSnapshot, Error> {
        let (peer, outgoing, owner, job_id) = {
            let jobs = lock(&self.shared.jobs);
            let record = jobs.get(&id).ok_or(Error::InvalidSetting(NO_JOB))?;
            (
                record.peer_key,
                record.outgoing,
                record.owner,
                wire_job_id(record),
            )
        };
        let generation = match self.generation_for(&peer, outgoing, owner) {
            Ok(generation) => generation,
            Err(Error::NoSession | Error::StaleSession) => {
                return self.retry_later(id, UNREACHABLE)
            }
            Err(error) => return Err(error),
        };
        let query = job::JobStatus {
            v: job::JOB_VERSION,
            id: 1,
            generation,
            job_id,
        };
        let Ok(bytes) = self.job_round_trip_side(
            &peer,
            outgoing,
            owner,
            job::TAG_JOB_MANIFEST,
            &job::encode(&query)?,
        ) else {
            return self.job_snapshot(id);
        };
        let reply: job::ManifestReply = job::decode(&bytes)?;
        if reply.id != query.id || reply.job_id != job_id {
            return Err(Error::Malformed);
        }
        let manifest = match (reply.status, reply.manifest) {
            (job::STATUS_OK, None) => return self.job_snapshot(id),
            (job::STATUS_OK, Some(manifest)) => manifest,
            (job::STATUS_STALE, _) => return self.retry_later(id, CONNECTION_ENDED),
            _ => return self.mark_failed(id, PREPARE_FAILED),
        };
        if manifest::validate(&manifest).is_err() {
            return self.mark_failed(id, PREPARE_FAILED);
        }
        {
            let mut jobs = lock(&self.shared.jobs);
            let record = jobs.get_mut(&id).ok_or(Error::InvalidSetting(NO_JOB))?;
            if record.manifest.is_some() || record.state.is_terminal() {
                return Ok(record.snapshot());
            }
            record.manifest = Some(manifest);
            if record.state == JobState::Preparing {
                record.enter(JobState::Transferring);
            }
        }
        save_jobs(&self.shared)?;
        self.job_snapshot(id)
    }

    /// Returns the stored choice, the job-wide choice, or `None` when the
    /// destination is free. A waiting state is entered by the caller.
    pub(super) fn conflict_choice(
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
        if self
            .receiver_entry(id, local_root, &local_rel, &entry.path, None)?
            .is_none()
        {
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

    pub(super) fn receiver_dest(
        &self,
        id: u64,
        root: u64,
        rel: &str,
        path: &str,
    ) -> Result<std::path::PathBuf, Error> {
        let mut pin = {
            let jobs = lock(&self.shared.jobs);
            let record = jobs.get(&id).ok_or(Error::InvalidSetting(NO_JOB))?;
            let state = lock(&self.shared.state);
            super::service_jobs::check_owner(&state, record.owner)?;
            record.local_pin.clone()
        };
        let adopted = pin.is_none();
        let dest = {
            let roots = lock(&self.shared.file_roots);
            // A replaced destination drive fails the leg instead of writing
            // staged bytes into whatever the path resolves to now.
            verify_leg_pin(&roots, root, &mut pin)?;
            let (parent_rel, name) = match path.rfind('/') {
                Some(slash) => (&path[..slash], &path[slash + 1..]),
                None => ("", path),
            };
            // The manifest's path rules already validated every component; resolve the
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
            roots.resolve_dir(root, &full_parent)?.join(name)
        };
        if adopted {
            if let Some(record) = lock(&self.shared.jobs).get_mut(&id) {
                record.local_pin = pin;
            }
        }
        Ok(dest)
    }

    /// The receiver's folder `full` for `path` as a directory handle, once
    /// the job's leg is verified. A folder that is not there yet, or a job
    /// whose leg is not pinned yet, takes [`Self::receiver_dest`] first,
    /// which creates the missing levels and pins the leg.
    #[cfg(any(unix, windows))]
    pub(super) fn receiver_dir(
        &self,
        id: u64,
        root: u64,
        rel: &str,
        path: &str,
        full: &str,
    ) -> Result<vnilink_rename::anchored::Dir, Error> {
        let pin = {
            let jobs = lock(&self.shared.jobs);
            let record = jobs.get(&id).ok_or(Error::InvalidSetting(NO_JOB))?;
            let state = lock(&self.shared.state);
            super::service_jobs::check_owner(&state, record.owner)?;
            record.local_pin.clone()
        };
        if let Some(pin) = pin {
            let roots = lock(&self.shared.file_roots);
            // A replaced destination drive fails the leg here too.
            verify_leg_pin(&roots, root, &mut Some(pin))?;
            if let Ok(dir) = roots.open_dir_fd(root, full) {
                return Ok(dir);
            }
        }
        self.receiver_dest(id, root, rel, path)?;
        lock(&self.shared.file_roots).open_dir_fd(root, full)
    }

    /// The entry `name` (by default the manifest path's own name) in the
    /// receiver's folder for `path`, stated without following a link: its
    /// size when it is a plain file, `u64::MAX` for anything else there, and
    /// `None` when nothing is. It is stated through anchored handles, so a
    /// folder swapped for a link is never read through.
    pub(super) fn receiver_entry(
        &self,
        id: u64,
        root: u64,
        rel: &str,
        path: &str,
        name: Option<&str>,
    ) -> Result<Option<u64>, Error> {
        let (parent_rel, own) = path.rsplit_once('/').unwrap_or(("", path));
        let name = name.unwrap_or(own);
        #[cfg(any(unix, windows))]
        {
            use vnilink_rename::anchored::AsDir;
            let full = match (rel.is_empty(), parent_rel.is_empty()) {
                (true, _) => parent_rel.to_owned(),
                (_, true) => rel.to_owned(),
                _ => format!("{rel}/{parent_rel}"),
            };
            let dir = self.receiver_dir(id, root, rel, path, &full)?;
            match vnilink_rename::anchored::stat_at(dir.as_dir(), name) {
                Ok(stat) if stat.is_file && !stat.is_symlink => Ok(Some(stat.size)),
                Ok(_) => Ok(Some(u64::MAX)),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
                Err(_) => Ok(Some(u64::MAX)),
            }
        }
        #[cfg(not(any(unix, windows)))]
        {
            let dest = self.receiver_dest(id, root, rel, path)?;
            let dest = dest.with_file_name(name);
            Ok(std::fs::symlink_metadata(&dest).ok().map(|meta| {
                if meta.is_file() {
                    meta.len()
                } else {
                    u64::MAX
                }
            }))
        }
    }

    pub(super) fn receiver_available_storage(&self, id: u64, path: &str) -> Result<u64, Error> {
        let (root, rel) = {
            let jobs = lock(&self.shared.jobs);
            let record = jobs.get(&id).ok_or(Error::InvalidSetting(NO_JOB))?;
            (record.local_root, record.local_rel.clone())
        };
        let parent = path.rsplit_once('/').map_or("", |(parent, _)| parent);
        let full = match (rel.is_empty(), parent.is_empty()) {
            (true, _) => parent.to_owned(),
            (_, true) => rel,
            _ => format!("{rel}/{parent}"),
        };
        #[cfg(unix)]
        let destination = {
            let fd = lock(&self.shared.file_roots).open_dir_fd(root, &full)?;
            super::file_resources::available_fd(&fd)?
        };
        // Asked of the anchored folder's handle, never of a path a link
        // could steer to another volume.
        #[cfg(windows)]
        let destination = {
            use vnilink_rename::anchored::AsDir;
            let dir = lock(&self.shared.file_roots).open_dir_fd(root, &full)?;
            vnilink_rename::anchored::available_space(dir.as_dir())
                .map_err(|_| Error::InvalidSetting("available storage could not be checked"))?
        };
        #[cfg(not(any(unix, windows)))]
        let destination = {
            let path = lock(&self.shared.file_roots).resolve_dir(root, &full)?;
            super::file_resources::available(&path)?
        };
        let transfer_dir = super::transfer::transfer_directory(&self.shared)?;
        Ok(destination.min(super::file_resources::available(&transfer_dir)?))
    }

    /// Starts fetching the small files after `current` that are not
    /// already arriving, up to [`PREFETCH_FILES`] and [`PREFETCH_BYTES`]
    /// ahead. A file of more than one segment is left for its turn.
    pub(super) fn prefetch_after(&self, id: u64, peer: &[u8; 32], wire_job: u64, current: usize) {
        #[cfg(any(test, feature = "test-probes"))]
        if self.shared.prefetch_off.load(Ordering::Relaxed) {
            return;
        }
        let Ok(addr) = super::transfer::peer_addr(&self.shared, peer) else {
            return;
        };
        let started = {
            let jobs = lock(&self.shared.jobs);
            let Some(record) = jobs.get(&id) else {
                return;
            };
            let mut prefetches = lock(&self.shared.job_prefetch);
            let prefetch = prefetches.entry(id).or_default();
            // Files the job already passed, landed or skipped, are done with.
            prefetch.files.retain(|index, _| *index > current);
            let mut count = prefetch.files.len();
            let mut bytes: u64 = prefetch.files.values().map(|(size, _)| size).sum();
            let mut started = Vec::new();
            let files = record
                .manifest
                .iter()
                .flat_map(|m| m.files_from(current + 1));
            for (index, entry) in files {
                if count >= PREFETCH_FILES || bytes >= PREFETCH_BYTES {
                    break;
                }
                if entry.size > crate::transfer::SEGMENT_LEN || prefetch.files.contains_key(&index)
                {
                    continue;
                }
                count += 1;
                bytes += entry.size;
                started.push((index, entry.path.clone(), entry.size));
            }
            let stop = prefetch
                .stop
                .get_or_insert_with(|| tokio::sync::watch::channel(false).0)
                .clone();
            (started, stop)
        };
        let (started, stop) = started;
        for (index, path, size) in started {
            let Ok((staged, segments)) = self.stage_paths(id, &path) else {
                return;
            };
            let made = staged
                .parent()
                .is_none_or(|parent| std::fs::create_dir_all(parent).is_ok());
            let Ok(file) = u32::try_from(index) else {
                return;
            };
            if !made {
                return;
            }
            let shared = self.shared.clone();
            let handle = self.runtime.handle().clone();
            let peer = *peer;
            let addr = addr.clone();
            let mut cancel = stop.subscribe();
            // A blocking worker drives it: partial writes block.
            let fetching = self.runtime.spawn_blocking(move || {
                let (progress, _) = std::sync::mpsc::channel();
                let mut via = None;
                let fetched = handle.block_on(super::transfer::fetch_file(
                    &shared,
                    &peer,
                    addr,
                    crate::transfer::FileRequest {
                        job: wire_job,
                        file,
                        offset: 0,
                        held: 0,
                    },
                    size,
                    &staged,
                    &segments,
                    &progress,
                    &mut cancel,
                    &mut via,
                ));
                (fetched, via)
            });
            lock(&self.shared.job_prefetch)
                .entry(id)
                .or_default()
                .files
                .insert(index, (size, fetching));
        }
    }

    /// Where file `rel_path` of job `id` arrives, and its segment journal.
    pub(super) fn stage_paths(
        &self,
        id: u64,
        rel_path: &str,
    ) -> Result<(std::path::PathBuf, std::path::PathBuf), Error> {
        let job = super::transfer::transfer_directory(&self.shared)?
            .join(STAGING_DIR)
            .join(format!("job-{id}"));
        Ok((
            job.join(STAGED_FILES).join(rel_path),
            job.join(STAGED_SEGMENTS).join(rel_path),
        ))
    }

    /// A destination leg on a replaced drive interrupts the job instead of
    /// wedging the poll: the error is retryable, so putting the drive back
    /// and resuming continues. Any other error keeps its meaning.
    pub(super) fn interrupt_on_drive_change(
        &self,
        id: u64,
        error: Error,
    ) -> Result<JobSnapshot, Error> {
        match error {
            Error::InvalidSetting(msg) if msg == fs::DRIVE_CHANGED => {
                self.mark_interrupted(id, fs::DRIVE_CHANGED)
            }
            error => Err(error),
        }
    }

    pub(super) fn ensure_receiver_dirs(&self, id: u64) -> Result<(), Error> {
        // Snapshot under short locks; mkdirs may block on slow storage, so
        // no job/state/roots lock is held across them. Publish re-checks
        // owner/grant before committing.
        let (local_root, local_rel, manifest, roots_snapshot, pin) = {
            let jobs = lock(&self.shared.jobs);
            let state = lock(&self.shared.state);
            let record = jobs.get(&id).ok_or(Error::InvalidSetting(NO_JOB))?;
            super::service_jobs::check_owner(&state, record.owner)?;
            let mut pin = record.local_pin.clone();
            let adopted = pin.is_none();
            // Verify and clone under one roots hold: the snapshot is
            // exactly the verified epoch, so a replaced drive cannot slip
            // skeleton mkdirs into the new directory.
            let roots_snapshot = {
                let roots = lock(&self.shared.file_roots);
                verify_leg_pin(&roots, record.local_root, &mut pin)?;
                roots.clone()
            };
            (
                record.local_root,
                record.local_rel.clone(),
                record
                    .manifest
                    .clone()
                    .ok_or(Error::InvalidSetting(NO_JOB))?,
                roots_snapshot,
                adopted.then_some(pin),
            )
        };
        // Persist a legacy adopt; the snapshot above is already dropped
        // (sequential, never nested).
        if let Some(pin) = pin {
            if let Some(record) = lock(&self.shared.jobs).get_mut(&id) {
                record.local_pin = pin;
            }
        }
        #[cfg(any(unix, windows))]
        let mkdir_probe = {
            let hooks = lock(&PRESTAGE_HOOKS);
            if hooks.engine == self.prestage_key() && hooks.job_id == Some(id) {
                hooks.mkdir_probe
            } else {
                None
            }
        };
        #[cfg(any(unix, windows))]
        if let Some(probe) = mkdir_probe {
            probe(id);
        }
        // The destination base itself may be fresh: build it first so the
        // per-entry levels below always start from an existing folder.
        if !local_rel.is_empty() && roots_snapshot.resolve_dir(local_root, &local_rel).is_err() {
            manifest::create_levels(&roots_snapshot, local_root, "", &local_rel)?;
        }
        manifest::ensure_manifest_dirs(&roots_snapshot, local_root, &local_rel, &manifest)
    }

    pub(super) fn finalize_receiver(&self, id: u64) -> Result<JobSnapshot, Error> {
        // Verify every manifest path landed before claiming completion.
        // Skipped files intentionally keep the destination as it was.
        let (local_root, local_rel, manifest, choices, apply_remaining) = {
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
                record.apply_remaining,
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
            // Every file before the end either landed or was skipped: by
            // its own answer, or by one given for the rest.
            let skipped = choices.get(&index).copied().or(apply_remaining);
            let name = match landed.get(&index) {
                None if skipped == Some(ConflictChoice::Skip) => continue,
                None => return Err(Error::InvalidSetting(NO_JOB)),
                Some(_) if choices.get(&index).copied() == Some(ConflictChoice::Skip) => continue,
                Some(name) => name,
            };
            let size = self
                .receiver_entry(id, local_root, &local_rel, &entry.path, Some(name.as_str()))?
                .unwrap_or(u64::MAX);
            if size != entry.size {
                return self.mark_failed(id, VERIFY_FAILED);
            }
        }
        // Every file was synced as it landed; the job is complete only once
        // the volume they landed on is flushed as well.
        if !landed.is_empty() {
            let folder = lock(&self.shared.file_roots).resolve_dir(local_root, &local_rel);
            let flushed = folder
                .map_err(|_| ())
                .and_then(|folder| crate::transfer::flush_volume(&folder).map_err(|_| ()));
            if flushed.is_err() {
                return self.mark_failed(id, FLUSH_FAILED);
            }
            tracing::info!(target: crate::trace::FILES, job = id, "received files flushed");
        }
        remove_staging(&self.shared, id);
        {
            let mut jobs = lock(&self.shared.jobs);
            let record = jobs.get_mut(&id).ok_or(Error::InvalidSetting(NO_JOB))?;
            if matches!(record.state, JobState::Cancelled | JobState::Interrupted) {
                return Ok(record.snapshot());
            }
            record.enter(JobState::Complete);
            record.error = None;
            drop(jobs);
        }
        save_jobs(&self.shared)?;
        self.job_snapshot(id)
    }

    /// Schedules another try of a step that failed for `reason`, a failure
    /// that may clear on its own, keeping the job's state. The job shows
    /// `reason` meanwhile. Once [`MAX_RETRIES`] tries or [`RETRY_WINDOW`]
    /// pass without the job moving a byte, it is interrupted for `reason`.
    pub(super) fn retry_later(&self, id: u64, reason: &'static str) -> Result<JobSnapshot, Error> {
        let give_up = {
            let mut jobs = lock(&self.shared.jobs);
            let record = jobs.get_mut(&id).ok_or(Error::InvalidSetting(NO_JOB))?;
            if record.state.is_terminal() || record.state == JobState::Interrupted {
                return Ok(record.snapshot());
            }
            let now = std::time::Instant::now();
            let done = record.done();
            let fresh = Retry {
                attempts: 0,
                first_at: now,
                next_at: now,
                done_at: done,
            };
            let retry = record.retry.get_or_insert_with(|| fresh.clone());
            // Bytes moved since: this is a new run of failures.
            if done > retry.done_at {
                *retry = fresh;
            }
            if retry.attempts >= MAX_RETRIES || now.duration_since(retry.first_at) >= RETRY_WINDOW {
                record.retry = None;
                true
            } else {
                retry.attempts += 1;
                retry.next_at = now + retry_pause(retry.attempts);
                self.shared.transfer_counters.retried();
                record.error = Some(reason.to_owned());
                false
            }
        };
        if give_up {
            return self.mark_interrupted(id, reason);
        }
        save_jobs(&self.shared)?;
        self.job_snapshot(id)
    }

    /// Ends a run of retries after a step that worked.
    pub(super) fn retry_settled(record: &mut JobRecord) {
        if record.retry.take().is_some() {
            record.error = None;
        }
    }

    /// Stops a job for `reason`, which it shows until it resumes. A
    /// cancelled job stays cancelled.
    pub(super) fn mark_interrupted(
        &self,
        id: u64,
        reason: &'static str,
    ) -> Result<JobSnapshot, Error> {
        {
            let mut jobs = lock(&self.shared.jobs);
            let record = jobs.get_mut(&id).ok_or(Error::InvalidSetting(NO_JOB))?;
            record.retry = None;
            if record.state != JobState::Cancelled {
                record.enter(JobState::Interrupted);
                record.error = Some(reason.to_owned());
            }
        }
        stop_prefetch(&self.shared, id);
        save_jobs(&self.shared)?;
        self.job_snapshot(id)
    }

    /// Receiver-side authorization at publication time: the peer must still
    /// be paired with the transfer grant. Revocation stops publication even
    /// when bytes already arrived.
    pub(super) fn receiver_granted(shared: &Shared, peer: &[u8; 32]) -> bool {
        let state = lock(&shared.state);
        match state.trust.get(peer) {
            Some(device) => state.trust.granted(&device.endpoint_id, Grant::Transfer),
            None => false,
        }
    }

    pub(super) fn mark_failed(&self, id: u64, reason: &'static str) -> Result<JobSnapshot, Error> {
        {
            let mut jobs = lock(&self.shared.jobs);
            let record = jobs.get_mut(&id).ok_or(Error::InvalidSetting(NO_JOB))?;
            record.enter(JobState::Failed);
            record.error = Some(reason.to_owned());
        }
        // Failed jobs do not resume: their partial only takes disk.
        remove_staging(&self.shared, id);
        save_jobs(&self.shared)?;
        self.job_snapshot(id)
    }
}
