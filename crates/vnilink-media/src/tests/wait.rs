// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! How this crate's tests wait for something another thread does.
//!
//! A session reaching a phase has an event source, [`session_active`]. The
//! rest (a counter a fake encoder bumps, a topology the host publishes) has
//! none and is observed with [`wait_for`].

use std::time::{Duration, Instant};

use vnilink::{Core, SessionPhase};

/// The bound every wait here has.
const WITHIN: Duration = Duration::from_secs(10);

/// Looks at `check` until it returns a value, and panics naming `what` if
/// that takes longer than [`WITHIN`]. It polls a state that has no event
/// source, so it is the last resort after an event, a channel or a join.
pub(super) fn wait_for<T>(what: &str, mut check: impl FnMut() -> Option<T>) -> T {
    let started = Instant::now();
    loop {
        if let Some(value) = check() {
            return value;
        }
        assert!(
            started.elapsed() < WITHIN,
            "waited {WITHIN:?} for {what}, which did not happen"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
}

/// [`wait_for`] a condition that carries no value.
pub(super) fn wait_until(what: &str, mut condition: impl FnMut() -> bool) {
    wait_for(what, || condition().then_some(()));
}

/// Returns once `controller`'s session `id` is active, woken by the core's
/// events. It subscribes before it looks, so a change between the two is not
/// missed.
pub(super) fn wait_active(controller: &Core, id: u64) {
    let events = controller.subscribe().unwrap();
    let started = Instant::now();
    loop {
        let phase = controller.controller_session(id).unwrap().phase;
        if phase == SessionPhase::Active {
            return;
        }
        let left = WITHIN.saturating_sub(started.elapsed());
        assert!(
            !left.is_zero(),
            "waited {WITHIN:?} for session {id} to be active; it is {phase:?}"
        );
        events.recv_timeout(left);
    }
}
