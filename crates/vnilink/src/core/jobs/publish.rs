// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! Staging a received file and publishing it at its destination.

use super::*;

impl Core {
    /// Publishes one staged file, returning the landed file name.
    /// Returns `None` when the poll must stop with the stored snapshot
    /// (a lost KeepBoth race re-enters the conflict state).
    /// Enforces the transfer grant at publication: revocation between fetch
    /// and commit refuses the write instead of creating the destination.
    pub(super) fn publish_staged(
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
            crate::ignored!(
                crate::trace::FILES,
                "a staged file left behind is swept with its job's staging",
                std::fs::remove_file(staged)
            );
            self.mark_interrupted(id, PERMISSION_OFF)?;
            return Ok(None);
        }
        let policy = match choice {
            ConflictChoice::Replace => manifest::PublishPolicy::Replace,
            ConflictChoice::KeepBoth => manifest::PublishPolicy::KeepBoth,
            ConflictChoice::Skip => return Ok(None),
        };
        #[cfg(any(unix, windows))]
        {
            self.publish_staged_anchored(id, &entry.path, staged, policy, local_root, &local_rel)
        }
        #[cfg(not(any(unix, windows)))]
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
                    record.enter(JobState::WaitingDecision);
                    record.waiting_file = Some(record.file_index);
                    drop(jobs);
                    save_jobs(&self.shared)?;
                    Ok(None)
                }
                Err(_) => {
                    crate::ignored!(
                        crate::trace::FILES,
                        "a staged file left behind is swept with its job's staging",
                        std::fs::remove_file(staged)
                    );
                    self.mark_failed(id, PUBLISH_FAILED)?;
                    Ok(None)
                }
            }
        }
    }

    /// Test hooks for cross-volume pre-staging; see `PRESTAGE_HOOKS`.
    /// Production leaves them unset.
    #[cfg(any(unix, windows))]
    pub fn set_prestage_hooks(hooks: PrestageHooks) {
        *lock(&PRESTAGE_HOOKS) = hooks;
    }

    /// Identifies this engine to its test hooks: `PrestageHooks` on
    /// Unix and Windows, and `PREPARE_SCAN_HOOKS` everywhere.
    #[cfg(any(unix, windows, test, feature = "test-probes"))]
    pub fn prestage_key(&self) -> usize {
        let _engine = self.engine();
        Arc::as_ptr(&self.shared) as usize
    }

    /// Test probe for in-flight serve opens; see [`SERVE_SOURCE_PROBE`].
    /// Production leaves it unset.
    #[cfg(any(test, feature = "test-probes"))]
    pub fn set_serve_source_probe(probe: Option<fn(usize, u64)>) {
        *lock(&SERVE_SOURCE_PROBE) = probe;
    }

    /// Files of job `id` being fetched ahead of their turn.
    #[cfg(any(test, feature = "test-probes"))]
    pub fn fetching_ahead(&self, id: u64) -> usize {
        let _engine = self.engine();
        lock(&self.shared.job_prefetch)
            .get(&id)
            .map_or(0, |prefetch| prefetch.files.len())
    }

    /// Takes one file at a time, fetching none ahead, for tests that stop
    /// a job between two files.
    #[cfg(any(test, feature = "test-probes"))]
    pub fn fetch_one_file_at_a_time(&self) {
        let _engine = self.engine();
        self.shared.prefetch_off.store(true, Ordering::Relaxed);
    }

    /// Identifies this engine to [`SERVE_SOURCE_PROBE`].
    #[cfg(any(test, feature = "test-probes"))]
    pub fn engine_key(&self) -> usize {
        let _engine = self.engine();
        Arc::as_ptr(&self.shared) as usize
    }

    /// Receiver liveness for one pre-stage block: the job still exists and
    /// is not stood down, the peer keeps the transfer grant, and the Files
    /// owner (if any) is still live. Locks are taken and released per
    /// call; the copy loop never holds them across I/O.
    #[cfg(any(unix, windows))]
    pub(super) fn prestage_live(&self, id: u64) -> bool {
        let (owner, peer_key) = {
            let jobs = lock(&self.shared.jobs);
            let Some(record) = jobs.get(&id) else {
                return false;
            };
            if matches!(
                record.state,
                JobState::Cancelled | JobState::Interrupted | JobState::Failed
            ) {
                return false;
            }
            (record.owner, record.peer_key)
        };
        let state = lock(&self.shared.state);
        let granted = match state.trust.get(&peer_key) {
            Some(device) => state.trust.granted(&device.endpoint_id, Grant::Transfer),
            None => false,
        };
        granted && super::service_jobs::check_owner(&state, owner).is_ok()
    }

    /// Cross-volume pre-stage, run OUTSIDE global locks. When the staged
    /// file and the destination live on different filesystems, stream the
    /// bytes to a temp sibling on the destination volume with per-block
    /// cancel/grant checks; the commit under locks is then a fast
    /// same-filesystem rename. Returns the stage to commit, `None` for
    /// the same-volume fast path, or stands the file down (record
    /// updated, siblings cleaned) and returns `None` with `stood_down`.
    #[cfg(any(unix, windows))]
    pub(super) fn prestage_cross_volume<'a>(
        &self,
        id: u64,
        staged: &std::path::Path,
        dir_fd: vnilink_rename::anchored::DirRef<'a>,
    ) -> Result<(Option<vnilink_rename::anchored::SiblingStage<'a>>, bool), Error> {
        use std::io::Read;
        let (force, probe, sync_probe) = {
            let hooks = lock(&PRESTAGE_HOOKS);
            let scoped = hooks.engine == self.prestage_key() && hooks.job_id == Some(id);
            (
                scoped && hooks.force_cross_volume,
                if scoped { hooks.probe } else { None },
                if scoped { hooks.sync_probe } else { None },
            )
        };
        if !force {
            match vnilink_rename::anchored::same_filesystem(staged, dir_fd) {
                Ok(true) => return Ok((None, false)),
                Ok(false) => {}
                Err(_) => {
                    crate::ignored!(
                        crate::trace::FILES,
                        "a staged file left behind is swept with its job's staging",
                        std::fs::remove_file(staged)
                    );
                    self.mark_failed(id, PUBLISH_FAILED)?;
                    return Ok((None, true));
                }
            }
        }
        let mut stage = match vnilink_rename::anchored::SiblingStage::create(dir_fd) {
            Ok(stage) => stage,
            Err(_) => {
                crate::ignored!(
                    crate::trace::FILES,
                    "a staged file left behind is swept with its job's staging",
                    std::fs::remove_file(staged)
                );
                self.mark_failed(id, PUBLISH_FAILED)?;
                return Ok((None, true));
            }
        };
        let mut src = match std::fs::File::open(staged) {
            Ok(file) => file,
            Err(_) => {
                crate::ignored!(
                    crate::trace::FILES,
                    "a staged file left behind is swept with its job's staging",
                    std::fs::remove_file(staged)
                );
                self.mark_failed(id, PUBLISH_FAILED)?;
                return Ok((None, true));
            }
        };
        let mut buf = vec![0u8; 64 * 1024];
        let mut copied: u64 = 0;
        loop {
            if !self.prestage_live(id) {
                crate::ignored!(
                    crate::trace::FILES,
                    "a staged file left behind is swept with its job's staging",
                    std::fs::remove_file(staged)
                );
                self.mark_interrupted(id, STOPPED)?;
                return Ok((None, true));
            }
            if let Some(probe) = probe {
                probe(id, copied);
            }
            let n = match src.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => n,
                Err(_) => {
                    crate::ignored!(
                        crate::trace::FILES,
                        "a staged file left behind is swept with its job's staging",
                        std::fs::remove_file(staged)
                    );
                    self.mark_failed(id, PUBLISH_FAILED)?;
                    return Ok((None, true));
                }
            };
            if stage.write_all(&buf[..n]).is_err() {
                crate::ignored!(
                    crate::trace::FILES,
                    "a staged file left behind is swept with its job's staging",
                    std::fs::remove_file(staged)
                );
                self.mark_failed(id, PUBLISH_FAILED)?;
                return Ok((None, true));
            }
            copied += n as u64;
        }
        // Durability lands here, before the authority gate: the commit under
        // locks performs only the atomic rename via `commit_prepared`.
        if let Some(probe) = sync_probe {
            probe(id);
        }
        if stage.fsync().is_err() {
            crate::ignored!(
                crate::trace::FILES,
                "a staged file left behind is swept with its job's staging",
                std::fs::remove_file(staged)
            );
            self.mark_failed(id, PUBLISH_FAILED)?;
            return Ok((None, true));
        }
        Ok((Some(stage), false))
    }

    /// Anchored publication: the destination parent is opened as a directory
    /// handle and the staged file is committed by a rename relative to it
    /// (`renameat`, or a rename by handle on Windows), so a parent replaced
    /// by a link cannot redirect the write outside the root.
    /// Cross-volume bytes are pre-staged to a destination sibling BEFORE
    /// locking; the final authorization/state decision is synchronized with
    /// the commit: both the job and grant locks are held across the re-check
    /// and the fast same-filesystem `renameat` only, so receiver
    /// cancellation (`Cancelled`) or revocation (`Interrupted` via grant
    /// loss) completing before the rename prevents destination creation.
    #[cfg(any(unix, windows))]
    pub(super) fn publish_staged_anchored(
        &self,
        id: u64,
        manifest_path: &str,
        staged: &std::path::Path,
        policy: manifest::PublishPolicy,
        local_root: u64,
        local_rel: &str,
    ) -> Result<Option<String>, Error> {
        use vnilink_rename::anchored::AsDir;
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
        // Mkdirs may block on slow storage: snapshot the owner check and the
        // root set under short locks, then run every filesystem call with no
        // job/state/roots lock held. The commit below re-locks and re-checks
        // cancellation, grant and owner before the rename.
        let (roots_snapshot, mut pin, adopted) = {
            let jobs = lock(&self.shared.jobs);
            let record = jobs.get(&id).ok_or(Error::InvalidSetting(NO_JOB))?;
            let state = lock(&self.shared.state);
            super::service_jobs::check_owner(&state, record.owner)?;
            let mut pin = record.local_pin.clone();
            let adopted = pin.is_none();
            // Verify and clone under one roots hold: pre-stage mkdirs
            // below run against exactly the verified epoch.
            let roots = lock(&self.shared.file_roots);
            verify_leg_pin(&roots, record.local_root, &mut pin)?;
            (roots.clone(), pin, adopted)
        };
        let mkdir_probe = {
            let hooks = lock(&PRESTAGE_HOOKS);
            if hooks.engine == self.prestage_key() && hooks.job_id == Some(id) {
                hooks.mkdir_probe
            } else {
                None
            }
        };
        if roots_snapshot
            .open_dir_fd(local_root, &full_parent)
            .is_err()
        {
            if let Some(probe) = mkdir_probe {
                probe(id);
            }
            manifest::create_levels(&roots_snapshot, local_root, local_rel, parent_rel)?;
        }
        let roots = lock(&self.shared.file_roots);
        // The dir handle below pins whatever the live roots resolve NOW:
        // re-verify the leg first so a replaced destination drive fails instead
        // of binding the new directory.
        verify_leg_pin(&roots, local_root, &mut pin)?;
        let dir_fd = roots.open_dir_held(local_root, &full_parent)?;
        drop(roots);
        // For KeepBoth, choose the free name through the anchored listing
        // before locking; the no-replace commit below decides atomically and
        // re-enters conflict on a lost race.
        let free_name: Option<String> = match policy {
            manifest::PublishPolicy::Replace => None,
            manifest::PublishPolicy::KeepBoth => {
                let existing: std::collections::HashSet<String> =
                    vnilink_rename::anchored::list_names(dir_fd.as_dir())
                        .map_err(|_| Error::InvalidSetting(PUBLISH_FAILED))?
                        .into_iter()
                        .filter_map(|os| os.into_string().ok())
                        .collect();
                Some(manifest::keep_both_name(file_name, |candidate| {
                    existing.contains(candidate)
                }))
            }
        };
        // Cross-volume bytes move to a destination sibling BEFORE locking;
        // the commit below only renames within one filesystem, so slow
        // destination I/O never holds global locks. A stood-down file
        // (cancelled/revoked/failed mid-copy) already updated its record.
        let (mut stage, stood_down) = self.prestage_cross_volume(id, staged, dir_fd.as_dir())?;
        if stood_down {
            return Ok(None);
        }
        // One commit per publish: consumes the pre-staged sibling when the
        // volumes differ, else renames staged directly (strict: EXDEV
        // means a remount raced the pre-stage check and stands down
        // retryably). Reports whether the caller must remove `staged`
        // (sibling commits leave it behind; renames move it).
        let mut commit_file = |dest_name: &str, replace: bool| -> std::io::Result<bool> {
            if let Some(stage) = stage.take() {
                // Pre-synced above: rename only, no slow I/O under the gate.
                stage.commit_prepared(dest_name, replace)?;
                Ok(true)
            } else {
                vnilink_rename::anchored::rename_from_staged(
                    staged,
                    dir_fd.as_dir(),
                    dest_name,
                    replace,
                )?;
                Ok(false)
            }
        };
        // Commit under both locks: re-check cancellation and grant after any
        // barrier, then fast same-filesystem `renameat` without releasing.
        // Revocation (grant update needs `state`, interruption needs `jobs`)
        // and cancellation (needs `jobs`) both block here, so completing
        // before the rename prevents it.
        let commit: Result<Option<String>, Error> = (|| {
            let mut jobs = lock(&self.shared.jobs);
            let record = jobs.get_mut(&id).ok_or(Error::InvalidSetting(NO_JOB))?;
            if matches!(record.state, JobState::Cancelled | JobState::Interrupted) {
                return Ok(None);
            }
            // Persist a legacy adopt: later publishes are strict. The
            // adopted pin was verified against the live roots above before
            // the destination handle was opened.
            if adopted {
                record.local_pin = pin.clone();
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
                    record.enter(JobState::Interrupted);
                    let reason = if granted {
                        super::service_jobs::orphan_reason(
                            &state_guard,
                            record.owner,
                            record.sender,
                            &peer_key,
                        )
                    } else {
                        PERMISSION_OFF
                    };
                    record.error = Some(reason.to_owned());
                    record.waiting_file = None;
                }
                drop(state_guard);
                drop(jobs);
                crate::ignored!(
                    crate::trace::FILES,
                    "a staged file left behind is swept with its job's staging",
                    std::fs::remove_file(staged)
                );
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
                        vnilink_rename::anchored::stat_at(dir_fd.as_dir(), file_name).is_ok();
                    let at = record.file_index;
                    let explicit_replace = record.choices.get(&at).copied()
                        == Some(ConflictChoice::Replace)
                        || record.apply_remaining == Some(ConflictChoice::Replace);
                    if dest_exists && !explicit_replace {
                        record.choices.remove(&at);
                        record.enter(JobState::WaitingDecision);
                        record.waiting_file = Some(at);
                        drop(state_guard);
                        drop(jobs);
                        save_jobs(&self.shared)?;
                        return Ok(None);
                    }
                    // Absent (auto or explicit with deleted dest) -> no-replace
                    // (detects race via Exists); present + explicit -> overwrite.
                    let allow_overwrite = dest_exists && explicit_replace;
                    match commit_file(file_name, allow_overwrite) {
                        Ok(consumed_sibling) => {
                            if consumed_sibling {
                                crate::ignored!(
                                    crate::trace::FILES,
                                    "a staged file left behind is swept with its job's staging",
                                    std::fs::remove_file(staged)
                                );
                            }
                            Ok(Some(file_name.to_owned()))
                        }
                        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                            // Lost auto-Replace race (dest appeared): conflict.
                            record.choices.remove(&record.file_index);
                            record.enter(JobState::WaitingDecision);
                            record.waiting_file = Some(record.file_index);
                            drop(state_guard);
                            drop(jobs);
                            save_jobs(&self.shared)?;
                            Ok(None)
                        }
                        Err(error) if error.kind() == std::io::ErrorKind::CrossesDevices => {
                            // Remount raced the pre-stage check: stand down
                            // retryably; the next poll re-detects volumes.
                            if record.state != JobState::Cancelled {
                                record.enter(JobState::Interrupted);
                                record.error = Some(VOLUME_CHANGED.to_owned());
                                record.waiting_file = None;
                            }
                            drop(state_guard);
                            drop(jobs);
                            crate::ignored!(
                                crate::trace::FILES,
                                "a staged file left behind is swept with its job's staging",
                                std::fs::remove_file(staged)
                            );
                            save_jobs(&self.shared)?;
                            Ok(None)
                        }
                        Err(_) => {
                            drop(state_guard);
                            drop(jobs);
                            crate::ignored!(
                                crate::trace::FILES,
                                "a staged file left behind is swept with its job's staging",
                                std::fs::remove_file(staged)
                            );
                            self.mark_failed(id, PUBLISH_FAILED)?;
                            Ok(None)
                        }
                    }
                }
                manifest::PublishPolicy::KeepBoth => {
                    let free = free_name
                        .clone()
                        .ok_or(Error::InvalidSetting(PUBLISH_FAILED))?;
                    if free == file_name {
                        match commit_file(file_name, false) {
                            Ok(consumed_sibling) => {
                                if consumed_sibling {
                                    crate::ignored!(
                                        crate::trace::FILES,
                                        "a staged file left behind is swept with its job's staging",
                                        std::fs::remove_file(staged)
                                    );
                                }
                                Ok(Some(file_name.to_owned()))
                            }
                            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                                // Lost race: re-enter conflict.
                                record.choices.remove(&record.file_index);
                                record.enter(JobState::WaitingDecision);
                                record.waiting_file = Some(record.file_index);
                                drop(state_guard);
                                drop(jobs);
                                save_jobs(&self.shared)?;
                                Ok(None)
                            }
                            Err(error) if error.kind() == std::io::ErrorKind::CrossesDevices => {
                                if record.state != JobState::Cancelled {
                                    record.enter(JobState::Interrupted);
                                    record.error = Some(VOLUME_CHANGED.to_owned());
                                    record.waiting_file = None;
                                }
                                drop(state_guard);
                                drop(jobs);
                                crate::ignored!(
                                    crate::trace::FILES,
                                    "a staged file left behind is swept with its job's staging",
                                    std::fs::remove_file(staged)
                                );
                                save_jobs(&self.shared)?;
                                Ok(None)
                            }
                            Err(_) => {
                                drop(state_guard);
                                drop(jobs);
                                crate::ignored!(
                                    crate::trace::FILES,
                                    "a staged file left behind is swept with its job's staging",
                                    std::fs::remove_file(staged)
                                );
                                self.mark_failed(id, PUBLISH_FAILED)?;
                                Ok(None)
                            }
                        }
                    } else {
                        match commit_file(&free, false) {
                            Ok(consumed_sibling) => {
                                if consumed_sibling {
                                    crate::ignored!(
                                        crate::trace::FILES,
                                        "a staged file left behind is swept with its job's staging",
                                        std::fs::remove_file(staged)
                                    );
                                }
                                Ok(Some(free))
                            }
                            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                                record.choices.remove(&record.file_index);
                                record.enter(JobState::WaitingDecision);
                                record.waiting_file = Some(record.file_index);
                                drop(state_guard);
                                drop(jobs);
                                save_jobs(&self.shared)?;
                                Ok(None)
                            }
                            Err(error) if error.kind() == std::io::ErrorKind::CrossesDevices => {
                                if record.state != JobState::Cancelled {
                                    record.enter(JobState::Interrupted);
                                    record.error = Some(VOLUME_CHANGED.to_owned());
                                    record.waiting_file = None;
                                }
                                drop(state_guard);
                                drop(jobs);
                                crate::ignored!(
                                    crate::trace::FILES,
                                    "a staged file left behind is swept with its job's staging",
                                    std::fs::remove_file(staged)
                                );
                                save_jobs(&self.shared)?;
                                Ok(None)
                            }
                            Err(_) => {
                                drop(state_guard);
                                drop(jobs);
                                crate::ignored!(
                                    crate::trace::FILES,
                                    "a staged file left behind is swept with its job's staging",
                                    std::fs::remove_file(staged)
                                );
                                self.mark_failed(id, PUBLISH_FAILED)?;
                                Ok(None)
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
}
