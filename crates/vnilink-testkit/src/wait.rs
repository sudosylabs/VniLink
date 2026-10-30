// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

use std::time::{Duration, Instant};

/// How long a wait lasts when a suite has no reason to choose.
pub const DEFAULT_WAIT: Duration = Duration::from_secs(10);

/// Asks `check` until it gives a value, for at most `within`; then fails
/// naming `what` it waited for. It looks again after a tenth of the time it
/// has waited, between 50 µs and a millisecond, so a bench measuring
/// through it adds about a tenth to what it measures.
pub fn wait_for<T>(what: &str, within: Duration, mut check: impl FnMut() -> Option<T>) -> T {
    let started = Instant::now();
    loop {
        if let Some(value) = check() {
            return value;
        }
        assert!(
            started.elapsed() < within,
            "waited {within:?} for {what}, which did not happen"
        );
        let pause =
            (started.elapsed() / 10).clamp(Duration::from_micros(50), Duration::from_millis(1));
        std::thread::sleep(pause);
    }
}

/// Asks `check` throughout `period` and fails naming `what` the first time
/// it does not hold: the bound of a contract that something does not
/// happen. It looks again every millisecond, so a test holds for `period`.
pub fn holds(what: &str, period: Duration, mut check: impl FnMut() -> bool) {
    let started = Instant::now();
    while started.elapsed() < period {
        assert!(
            check(),
            "{what} stopped holding after {:?}",
            started.elapsed()
        );
        std::thread::sleep(Duration::from_millis(1));
    }
    assert!(check(), "{what} stopped holding at {period:?}");
}
