// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! The background workers that advance each job and wake when one changes.

use super::*;

/// How soon a sender asks its receiver again after an answer that changed
/// it, and the longest it waits while answers change nothing.
pub(super) const SENDER_SOON: Duration = Duration::from_millis(100);

pub(super) const SENDER_SELDOM: Duration = Duration::from_secs(1);

/// The longest a blocked receiver waits for a change before it looks again.
pub(super) const RECEIVER_RETRY: Duration = Duration::from_millis(200);

/// How often a file's arriving byte count is copied onto its job.
const ARRIVAL_EVERY: Duration = Duration::from_millis(250);

/// How long a sender waits after `pause` before asking again: soon after
/// an answer that `changed` the job, twice as long as before otherwise,
/// up to [`SENDER_SELDOM`].
fn next_pause(pause: Duration, changed: bool) -> Duration {
    if changed {
        SENDER_SOON
    } else {
        (pause * 2).min(SENDER_SELDOM)
    }
}

/// Whether an arriving byte count may be shown at `now`, the last having
/// been shown at `shown`.
fn arrival_due(shown: Option<std::time::Instant>, now: std::time::Instant) -> bool {
    shown.is_none_or(|shown| now.saturating_duration_since(shown) >= ARRIVAL_EVERY)
}

/// One job's worker: takes its steps until it has none left, then lets
/// the driver give the job a worker again when it next has one.
pub(super) fn work_job(weak: std::sync::Weak<Core>, shared: Arc<Shared>, id: u64) {
    struct Working<'a>(&'a Shared, u64);
    impl Drop for Working<'_> {
        fn drop(&mut self) {
            lock(&self.0.job_workers).remove(&self.1);
        }
    }
    let _working = Working(&shared, id);
    let mut pause = SENDER_SOON;
    loop {
        let seen = shared.job_revision.load(Ordering::Acquire);
        let Some(paced) = job_pace(&shared, id) else {
            return;
        };
        {
            let Some(core) = weak.upgrade() else {
                return;
            };
            if lock(&shared.state).shut_down {
                return;
            }
            if paced {
                let before = core.job_snapshot(id).ok();
                let after = core.job_poll(id).ok();
                pause = next_pause(pause, after.is_some() && after != before);
            } else {
                core.drive_receiver(id);
            }
        }
        if paced {
            // Changes meanwhile do not hurry it: only one that leaves it
            // nothing to do ends the wait early.
            let due = std::time::Instant::now() + pause;
            let mut seen = shared.job_revision.load(Ordering::Acquire);
            loop {
                let left = due.saturating_duration_since(std::time::Instant::now());
                if left.is_zero() {
                    break;
                }
                seen = wait_for_job_change(&shared, seen, left);
                if job_pace(&shared, id).is_none() {
                    return;
                }
            }
        } else {
            wait_for_job_change(&shared, seen, RECEIVER_RETRY);
        }
    }
}

/// Whether job `id` has a step to take, and if so whether it is paced:
/// a sender asks its peer on a timer instead of at every change.
pub(super) fn job_pace(shared: &Shared, id: u64) -> Option<bool> {
    lock(&shared.jobs)
        .get(&id)
        .filter(|record| record.drivable())
        .map(JobRecord::paced)
}

/// Waits until the job revision differs from `seen` or `timeout` passes,
/// and returns the revision then.
pub(super) fn wait_for_job_change(shared: &Shared, seen: u64, timeout: Duration) -> u64 {
    let deadline = std::time::Instant::now() + timeout;
    let (gate, wake) = &shared.job_wake;
    let mut guard = lock(gate);
    loop {
        let now = shared.job_revision.load(Ordering::Acquire);
        let left = deadline.saturating_duration_since(std::time::Instant::now());
        if now != seen || left.is_zero() {
            return now;
        }
        guard = wake
            .wait_timeout(guard, left)
            .map(|(guard, _)| guard)
            .unwrap_or_else(|poison| poison.into_inner().0);
    }
}

/// Copies a file's arriving byte count onto its job, at most four times a
/// second, until the fetch drops its end of `arriving`.
pub(super) fn follow_arrival(
    jobs: Arc<Mutex<HashMap<u64, JobRecord>>>,
    shared: Arc<Shared>,
    id: u64,
    arriving: std::sync::mpsc::Receiver<u64>,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(crate::trace::within(shared.span.clone(), move || {
        let mut shown = None;
        let mut latest = None;
        loop {
            let next = arriving.recv_timeout(ARRIVAL_EVERY);
            match next {
                Ok(bytes) => latest = Some(bytes),
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
            }
            let now = std::time::Instant::now();
            if let Some(bytes) = latest.filter(|_| arrival_due(shown, now)) {
                if let Some(record) = lock(&jobs).get_mut(&id) {
                    record.in_flight = bytes;
                }
                jobs_changed(&shared);
                shown = Some(std::time::Instant::now());
                latest = None;
            }
        }
    }))
}

/// Removes the partial files of job `id`, stopping its fetches ahead.
pub(super) fn remove_staging(shared: &Shared, id: u64) {
    stop_prefetch(shared, id);
    if let Some(dir) = lock(&shared.transfer_dir).clone() {
        crate::ignored!(
            crate::trace::FILES,
            "staging left behind is swept on the next start",
            std::fs::remove_dir_all(dir.join(STAGING_DIR).join(format!("job-{id}")))
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    #[test]
    fn a_sender_asks_soon_after_a_change_and_backs_off_to_seldom_without_one() {
        assert_eq!(next_pause(SENDER_SELDOM, true), SENDER_SOON);
        let mut pause = SENDER_SOON;
        let mut seen = vec![pause];
        while pause < SENDER_SELDOM {
            pause = next_pause(pause, false);
            seen.push(pause);
        }
        assert_eq!(
            seen,
            [100, 200, 400, 800, 1000].map(Duration::from_millis),
            "each unchanged answer doubles the wait, up to the cap"
        );
        assert_eq!(next_pause(SENDER_SELDOM, false), SENDER_SELDOM);
    }

    #[test]
    fn an_arriving_byte_count_is_shown_at_once_and_then_at_most_four_times_a_second() {
        let start = Instant::now();
        assert!(arrival_due(None, start), "the first count shows at once");
        let shown = Some(start);
        assert!(!arrival_due(
            shown,
            start + ARRIVAL_EVERY - Duration::from_nanos(1)
        ));
        assert!(arrival_due(shown, start + ARRIVAL_EVERY));
        assert!(arrival_due(shown, start + ARRIVAL_EVERY * 3));
    }
}
