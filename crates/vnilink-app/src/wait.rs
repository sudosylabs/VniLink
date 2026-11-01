// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! How this crate's tests wait for something another thread does.
//!
//! A session reaching a phase has an event source, [`session_phase`]. The
//! rest (a handle's state, bytes a fake backend recorded) has none and is
//! observed with [`wait_for`].

use crate::{FfiSessionInfo, FfiSessionPhase, VnilinkCore};
use std::time::{Duration, Instant};

/// The bound every wait here has.
const WITHIN: Duration = Duration::from_secs(8);

/// Looks at `check` until it returns a value, and panics naming `what` if
/// that takes longer than [`WITHIN`]. It polls a state that has no event
/// source, so it is the last resort after an event, a channel or a join.
pub(crate) fn wait_for<T>(what: &str, mut check: impl FnMut() -> Option<T>) -> T {
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
pub(crate) fn wait_until(what: &str, mut condition: impl FnMut() -> bool) {
    wait_for(what, || condition().then_some(()));
}

/// [`wait_until`] naming the condition by its own text.
macro_rules! until {
    ($condition:expr) => {
        $crate::wait::wait_until(concat!("`", stringify!($condition), "`"), || $condition)
    };
}
pub(crate) use until;

/// Returns once `controller`'s session `id` is in `phase`, woken by the
/// core's `SessionChanged` events. It subscribes before it looks, so a change
/// between the two is not missed.
pub(crate) fn session_phase(
    controller: &VnilinkCore,
    id: u64,
    phase: FfiSessionPhase,
) -> FfiSessionInfo {
    let events = controller.inner.subscribe().unwrap();
    let started = Instant::now();
    loop {
        let session = controller.controller_session(id).unwrap();
        if session.phase == phase {
            return session;
        }
        let left = WITHIN.saturating_sub(started.elapsed());
        assert!(
            !left.is_zero(),
            "waited {WITHIN:?} for session {id} to be {phase:?}; it is {:?}",
            session.phase
        );
        events.recv_timeout(left);
    }
}

/// [`session_phase`] for `Active`.
pub(crate) fn session_active(controller: &VnilinkCore, id: u64) -> FfiSessionInfo {
    session_phase(controller, id, FfiSessionPhase::Active)
}

/// Returns what `pick` makes of the first event on `events` it accepts, woken
/// by the events themselves.
pub(crate) fn event<T>(
    events: &vnilink::EventSubscription,
    what: &str,
    mut pick: impl FnMut(vnilink::CoreEvent) -> Option<T>,
) -> T {
    let started = Instant::now();
    loop {
        let left = WITHIN.saturating_sub(started.elapsed());
        assert!(
            !left.is_zero(),
            "waited {WITHIN:?} for {what}, which did not happen"
        );
        if let Some(value) = events.recv_timeout(left).and_then(&mut pick) {
            return value;
        }
    }
}
