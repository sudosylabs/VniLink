// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! What the power suites share: an operating system's power leases that
//! count what they give and take back, one that refuses them, and what an
//! engine says it keeps awake.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use vnilink::power::{HostPowerFactory, PowerInhibitor, PowerKind, PowerPhase, PowerStatus};
use vnilink::{BackendStartup, Core, Error};

/// Where a kind's counts are kept.
fn slot(kind: PowerKind) -> usize {
    match kind {
        PowerKind::Display => 0,
        PowerKind::System => 1,
    }
}

/// How many leases of each kind an engine's OS gave and took back.
#[derive(Default)]
pub struct Counts {
    acquired: [AtomicUsize; 2],
    released: [AtomicUsize; 2],
}

impl Counts {
    /// The leases of `kind` given so far.
    pub fn acquired(&self, kind: PowerKind) -> usize {
        self.acquired[slot(kind)].load(Ordering::SeqCst)
    }

    /// The leases of `kind` taken back so far.
    pub fn released(&self, kind: PowerKind) -> usize {
        self.released[slot(kind)].load(Ordering::SeqCst)
    }
}

/// An OS lease that keeps a display or the system awake while held.
struct Lease(Arc<Counts>, PowerKind);

impl PowerInhibitor for Lease {}

impl Drop for Lease {
    fn drop(&mut self) {
        self.0.released[slot(self.1)].fetch_add(1, Ordering::SeqCst);
    }
}

/// A lease of `kind`, counted in `counts` when given and when dropped.
pub fn lease(counts: &Arc<Counts>, kind: PowerKind) -> Box<dyn PowerInhibitor> {
    counts.acquired[slot(kind)].fetch_add(1, Ordering::SeqCst);
    Box::new(Lease(Arc::clone(counts), kind))
}

/// An OS that gives each lease at once and counts it.
pub struct Leases(pub Arc<Counts>);

impl HostPowerFactory for Leases {
    fn acquire(
        &self,
        kind: PowerKind,
        startup: &BackendStartup,
    ) -> Result<Box<dyn PowerInhibitor>, Error> {
        startup.check()?;
        Ok(lease(&self.0, kind))
    }
}

/// An OS that refuses every lease, counting each time it is asked.
pub struct Refusing(pub Arc<AtomicUsize>);

impl HostPowerFactory for Refusing {
    fn acquire(&self, _: PowerKind, _: &BackendStartup) -> Result<Box<dyn PowerInhibitor>, Error> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Err(Error::Unsupported("the lease was refused by the OS"))
    }
}

/// What `core` says of keeping `kind` awake now.
pub fn status(core: &Core, kind: PowerKind) -> Option<PowerStatus> {
    core.power_status()
        .ok()?
        .into_iter()
        .find(|status| status.kind == kind)
}

/// Whether `core` keeps `kind` awake for `owners` desktops.
pub fn active(core: &Core, kind: PowerKind, owners: u32) -> bool {
    status(core, kind)
        .is_some_and(|status| status.owners == owners && status.phase == PowerPhase::Active)
}
