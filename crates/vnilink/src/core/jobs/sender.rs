// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! The sending side of a job: polling it and resuming it.

use super::*;

impl Core {
    pub(super) fn poll_sender(&self, id: u64) -> Result<JobSnapshot, Error> {
        let (peer_key, outgoing, owner) = {
            let jobs = lock(&self.shared.jobs);
            let record = jobs.get(&id).ok_or(Error::InvalidSetting(NO_JOB))?;
            if record.telling_resume {
                return Ok(record.snapshot());
            }
            (record.peer_key, record.outgoing, record.owner)
        };
        let generation = match self.generation_for(&peer_key, outgoing, owner) {
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
            job_id: wire_job_id(
                lock(&self.shared.jobs)
                    .get(&id)
                    .ok_or(Error::InvalidSetting(NO_JOB))?,
            ),
        };
        let reply_bytes = match self.job_round_trip_side(
            &peer_key,
            outgoing,
            owner,
            job::TAG_JOB_STATUS,
            &job::encode(&query)?,
        ) {
            Ok(bytes) => bytes,
            Err(_) => return self.retry_later(id, UNREACHABLE),
        };
        let reply: job::JobStateReply = job::decode(&reply_bytes).map_err(|_| Error::Malformed)?;
        if reply.id != query.id {
            return Err(Error::Malformed);
        }
        if reply.status != job::STATUS_OK {
            return self.job_snapshot(id);
        }
        let state = job::JobState::from_name(&reply.state).ok_or(Error::Malformed)?;
        let changed = {
            let mut jobs = lock(&self.shared.jobs);
            let record = jobs.get_mut(&id).ok_or(Error::InvalidSetting(NO_JOB))?;
            let before = record.reported();
            Self::retry_settled(record);
            // Do not overwrite a locally cancelled record with a stale
            // receiver state: cancellation wins.
            if record.state != JobState::Cancelled {
                // The receiver is the one that knows what has arrived.
                record.done_bytes = reply.done_bytes;
                record.files_done = reply.files_done;
                record.skipped_files = reply.skipped_files;
                match state {
                    JobState::Complete => {
                        record.enter(JobState::Complete);
                        record.error = None;
                        record.waiting_file = None;
                    }
                    JobState::Failed | JobState::Cancelled | JobState::Interrupted => {
                        record.enter(state);
                        record.error = reply.error.map(|text| truncate_error(&text));
                        record.waiting_file = None;
                    }
                    JobState::WaitingDecision => {
                        // Map the receiver's waiting path back to this
                        // record's file index (manifests correlate by id).
                        let waiting = reply.waiting_file.as_deref().and_then(|path| {
                            record
                                .manifest
                                .as_ref()?
                                .entries
                                .iter()
                                .filter(|entry| !entry.is_dir)
                                .position(|entry| entry.path == path)
                        });
                        // A reply sent before this side's answer reached the
                        // receiver asks a question already answered here.
                        let answered = record.state != JobState::WaitingDecision
                            && waiting.is_some_and(|file| record.choices.contains_key(&file));
                        if !answered {
                            record.enter(JobState::WaitingDecision);
                            record.error = None;
                            record.waiting_file = waiting;
                        }
                    }
                    JobState::Transferring if record.state == JobState::WaitingDecision => {
                        super::service_jobs::check_owner(&lock(&self.shared.state), record.owner)?;
                        record.enter(JobState::Serving);
                        record.waiting_file = None;
                    }
                    _ => {}
                }
            }
            record.reported() != before
        };
        // An answer that changes nothing is not saved and wakes no one.
        if changed {
            save_jobs(&self.shared)?;
        }
        self.job_snapshot(id)
    }

    pub(super) fn resume_sender(&self, id: u64) -> Result<JobSnapshot, Error> {
        // Re-verify sources after a restart. A changed source fails the
        // job instead of moving new bytes.
        // Adopt the leg's pin first (jobs->roots order); the pin is
        // leg-wide, so an upfront mismatch is definitive and fails with
        // the drive's cause instead of erroring the resume call.
        let drive_changed = {
            let mut jobs = lock(&self.shared.jobs);
            let record = jobs.get_mut(&id).ok_or(Error::InvalidSetting(NO_JOB))?;
            verify_leg_pin(
                &lock(&self.shared.file_roots),
                record.local_root,
                &mut record.local_pin,
            )
            .is_err()
        };
        if drive_changed {
            return self.mark_failed(id, fs::DRIVE_CHANGED);
        }
        let (local_root, local_rel, local_pin, manifest, source_ids) = {
            let jobs = lock(&self.shared.jobs);
            let record = jobs.get(&id).ok_or(Error::InvalidSetting(NO_JOB))?;
            (
                record.local_root,
                record.local_rel.clone(),
                record.local_pin.clone().expect("leg pin just verified"),
                record
                    .manifest
                    .clone()
                    .ok_or(Error::InvalidSetting(NO_JOB))?,
                record.source_ids.clone(),
            )
        };
        if let Err(reason) = check_sending_sources(
            &self.shared,
            local_root,
            &local_rel,
            &local_pin,
            &manifest,
            &source_ids,
        ) {
            return self.mark_failed(id, reason);
        }
        {
            let mut jobs = lock(&self.shared.jobs);
            let record = jobs.get_mut(&id).ok_or(Error::InvalidSetting(NO_JOB))?;
            let state = lock(&self.shared.state);
            super::service_jobs::check_owner(&state, record.owner)?;
            record.enter(JobState::Serving);
            record.error = None;
            record.telling_resume = true;
            drop(state);
            drop(jobs);
        }
        save_jobs(&self.shared)?;
        self.job_snapshot(id)
    }
}
