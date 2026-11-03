// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! The locks and atomics of the core's own concurrent pieces (the event
//! hub, the input queue and the video counters): `std::sync` in every build,
//! and loom's models of them when built with `--cfg vnilink_loom`, where the
//! `loom_*` tests beside each piece run every interleaving of its threads.
//!
//! `Arc` and `Weak` are `std::sync`'s in both builds: loom's `Arc` has no
//! `Weak`, which the event hub's liveness token needs, and loom follows the
//! locks and atomics an `Arc` holds, not the count that keeps them alive.

pub(crate) use std::sync::{Arc, Weak};

#[cfg(vnilink_loom)]
pub(crate) use loom::sync::{Mutex, MutexGuard};
#[cfg(not(vnilink_loom))]
pub(crate) use std::sync::{Mutex, MutexGuard};

pub(crate) mod atomic {
    #[cfg(vnilink_loom)]
    pub(crate) use loom::sync::atomic::{AtomicU64, Ordering};
    #[cfg(not(vnilink_loom))]
    pub(crate) use std::sync::atomic::{AtomicU64, Ordering};
}
