// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! How long a capture loop waits before it opens its source again. A source
//! that keeps failing (a driver that resets, a portal that refuses) is asked
//! less often the longer it fails, while the first retry stays quick.
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

const FIRST: Duration = Duration::from_millis(200);
const LONGEST: Duration = Duration::from_secs(10);
/// How often a wait looks whether it was told to stop.
const STOP_CHECK: Duration = Duration::from_millis(50);

#[derive(Debug, Default)]
pub(crate) struct Backoff {
    failures: u32,
}

impl Backoff {
    /// The wait after `failures` failures in a row, doubling from the first.
    fn after(failures: u32) -> Duration {
        FIRST
            .checked_mul(1 << failures.min(16))
            .map_or(LONGEST, |wait| wait.min(LONGEST))
    }

    /// A run succeeded: the next failure is a first.
    pub(crate) fn reset(&mut self) {
        self.failures = 0;
    }

    /// Records a failure and waits out its delay, returning early when `stop` is set.
    pub(crate) fn wait(&mut self, stop: &AtomicBool) {
        let until = Instant::now() + Self::after(self.failures);
        self.failures = self.failures.saturating_add(1);
        while !stop.load(Ordering::Acquire) {
            let left = until.saturating_duration_since(Instant::now());
            if left.is_zero() {
                break;
            }
            std::thread::sleep(left.min(STOP_CHECK));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_wait_doubles_from_the_first_retry_up_to_the_longest() {
        let waits: Vec<_> = (0..8)
            .map(|failures| Backoff::after(failures).as_millis())
            .collect();
        assert_eq!(waits, [200, 400, 800, 1600, 3200, 6400, 10_000, 10_000]);
        assert_eq!(
            Backoff::after(u32::MAX),
            LONGEST,
            "a long outage cannot overflow"
        );
    }

    #[test]
    fn a_wait_ends_at_once_when_told_to_stop() {
        let stop = AtomicBool::new(true);
        let mut backoff = Backoff { failures: 20 };
        let started = Instant::now();
        backoff.wait(&stop);
        assert!(started.elapsed() < LONGEST / 2, "a stop cut the wait short");
        backoff.reset();
        assert_eq!(backoff.failures, 0);
    }
}
