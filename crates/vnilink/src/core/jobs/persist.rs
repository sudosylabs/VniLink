// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! Saving, loading and reconciling the job records.

use super::*;

/// Saves the jobs that changed and tells watchers: each change to a job
/// ends here. A save that changes nothing writes and tells nothing. A save
/// that fails is recorded: what it held may not survive a restart.
pub(in crate::core) fn save_jobs(shared: &Shared) -> Result<(), Error> {
    let saved = super::job_journal::save_jobs(shared);
    if let Err(error) = &saved {
        tracing::warn!(
            target: crate::trace::FILES,
            failure = error.name(),
            reason = error.reason(),
            "transfer records were not saved: a restart may lose their latest state"
        );
    }
    saved
}

/// [`save_jobs`] for callers that carry on either way; the failure is
/// recorded there.
pub(in crate::core) fn persist_jobs(shared: &Shared) {
    crate::ignored!(
        crate::trace::FILES,
        "save_jobs recorded the failure",
        save_jobs(shared)
    );
}

/// Records that the peer was not told of a change to a shared job: its
/// side keeps the job as it was until it next asks.
pub(super) fn told_peer(change: &'static str, job: u64, told: Result<(), Error>) {
    if let Err(error) = told {
        tracing::info!(
            target: crate::trace::FILES,
            job,
            change,
            failure = error.name(),
            reason = error.reason(),
            "the peer was not told: its side keeps the job as it was until it next asks"
        );
    }
}

pub(in crate::core) fn load_jobs(shared: &Shared) {
    // Each record is written afresh at its next save.
    lock(&shared.job_journal).reset();
    let Some(dir) = lock(&shared.transfer_dir).clone() else {
        return;
    };
    let Some((mut records, clean)) = super::job_journal::read_journals(&dir) else {
        return;
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
            // Anything in flight died with the process; a preparation has
            // nothing to resume from.
            match record.state {
                _ if record.preparing_here() && !record.state.is_terminal() => {
                    record.enter(JobState::Failed);
                    record.error = Some(PREPARE_STOPPED.to_owned());
                }
                JobState::Preparing | JobState::Serving | JobState::Transferring => {
                    record.enter(JobState::Interrupted);
                    record.error = Some(APP_CLOSED.to_owned());
                }
                _ => {}
            }
            max = max.max(record.id).max(wire_job_id(&record));
            jobs.insert(record.id, record);
        }
    }
    if clean {
        reconcile_staging(shared, &dir);
    }
    let next = shared.job_next.load(Ordering::Relaxed);
    if max.saturating_add(1) > next {
        shared
            .job_next
            .store(max.saturating_add(1), Ordering::Relaxed);
    }
}

/// Only reconcile after every journal was read. Missing or unreadable
/// journals retain partials for diagnosis rather than interpreting
/// corruption as permission to delete all previously owned data. Jobs
/// past the retention policy lose their partials. Only canonical managed
/// names are candidates; unexpected entries remain untouched.
pub(super) fn reconcile_staging(shared: &Shared, dir: &std::path::Path) {
    let policy = *lock(&shared.file_retention_policy);
    let keep = retained_job_ids(&lock(&shared.jobs), policy, retention_now());
    #[cfg(any(unix, windows))]
    reconcile_staging_at(dir, &keep);
    #[cfg(not(any(unix, windows)))]
    let _ = (dir, keep);
}

#[cfg(any(unix, windows))]
pub(super) fn reconcile_staging_at(dir: &std::path::Path, keep: &std::collections::HashSet<u64>) {
    use vnilink_rename::anchored;
    use vnilink_rename::anchored::AsDir;
    let Ok(root) = anchored::open_root(dir) else {
        return;
    };
    if let Ok(staging) = anchored::open_child(root.as_dir(), STAGING_DIR) {
        if let Ok(entries) = anchored::list_names(staging.as_dir()) {
            for name in entries {
                let Some(name) = name.to_str() else { continue };
                let Some(id) = name
                    .strip_prefix("job-")
                    .and_then(|n| n.parse::<u64>().ok())
                else {
                    continue;
                };
                if name != format!("job-{id}") || keep.contains(&id) {
                    continue;
                }
                crate::ignored!(
                    crate::trace::FILES,
                    "staging left behind is swept on the next start",
                    anchored::remove_tree_at(staging.as_dir(), name)
                );
            }
        }
    }
}
