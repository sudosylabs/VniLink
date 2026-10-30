// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! Independent per-source capture owners for Linux hosting.
//!
//! Each shared display (one X11 CRTC, or the single portal grant) runs under
//! its own owner: a stop flag plus the picture size the UI lists. Opening,
//! closing or switching one owner never touches its siblings; the supervisor
//! derives the wanted set from live selections and retires the rest. Leaving
//! the registry always stops the owner. No GTK, no display server: this
//! registry is pure and unit-tested on every host.

use std::collections::{BTreeSet, HashMap};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

/// Geometry that identifies one owner's pixels: origin, size, orientation
/// and scale. A same-id display whose fingerprint changed shows different
/// pixels and must be reopened, never kept on its old crop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GeometryFingerprint {
    pub x: i32,
    pub y: i32,
    pub width: u16,
    pub height: u16,
    pub rotation_deg: u16,
    pub scale_percent: u16,
}

impl GeometryFingerprint {
    pub fn of(display: &vnilink::DisplayInfo) -> Self {
        Self {
            x: display.origin_x,
            y: display.origin_y,
            width: display.width,
            height: display.height,
            rotation_deg: display.rotation_deg,
            scale_percent: display.scale_percent,
        }
    }
}

/// One live capture owner: its stop flag and its picture size.
pub struct SourceOwner {
    /// Set when the owner leaves the registry. Serve threads poll it.
    pub stop: Arc<AtomicBool>,
    pub width: u16,
    pub height: u16,
    /// Geometry the owner's pixels attest. None for fixed grants that
    /// cannot be reopened (portal): they are never stale.
    fingerprint: Option<GeometryFingerprint>,
    /// Topology revision the owner's frames stamp, shared with the
    /// capture so the supervisor can re-attest an unchanged owner to
    /// the current revision. None follows the current topology.
    revision: Option<Arc<AtomicU64>>,
}

impl SourceOwner {
    pub fn new(stop: Arc<AtomicBool>, width: u16, height: u16) -> Self {
        Self {
            stop,
            width,
            height,
            fingerprint: None,
            revision: None,
        }
    }

    pub fn with_fingerprint(mut self, fingerprint: GeometryFingerprint) -> Self {
        self.fingerprint = Some(fingerprint);
        self
    }

    pub fn with_revision(mut self, revision: Arc<AtomicU64>) -> Self {
        self.revision = Some(revision);
        self
    }

    fn stopped(self) -> Self {
        self.stop.store(true, Ordering::Release);
        self
    }
}

/// Capture owners keyed by topology display id.
#[derive(Default)]
pub struct SourceOwners {
    owners: HashMap<u64, SourceOwner>,
}

impl SourceOwners {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_empty(&self) -> bool {
        self.owners.is_empty()
    }

    pub fn len(&self) -> usize {
        self.owners.len()
    }

    pub fn contains(&self, id: u64) -> bool {
        self.owners.contains_key(&id)
    }

    pub fn get(&self, id: u64) -> Option<&SourceOwner> {
        self.owners.get(&id)
    }

    /// Live ids, ascending, for deterministic topology order.
    pub fn ids(&self) -> BTreeSet<u64> {
        self.owners.keys().copied().collect()
    }

    /// Insert an owner, stopping and displacing any previous owner of the
    /// same source. Siblings are untouched. Returns the displaced owner for
    /// joining its thread.
    pub fn insert(&mut self, id: u64, owner: SourceOwner) -> Option<SourceOwner> {
        self.owners.insert(id, owner).map(SourceOwner::stopped)
    }

    /// Remove an owner, stopping it. Siblings are untouched.
    pub fn remove(&mut self, id: u64) -> Option<SourceOwner> {
        self.owners.remove(&id).map(SourceOwner::stopped)
    }

    /// Fill in the picture size once the source opens. Never signals: the
    /// pending owner and the opened source are the same owner.
    pub fn update_size(&mut self, id: u64, width: u16, height: u16) {
        if let Some(owner) = self.owners.get_mut(&id) {
            owner.width = width;
            owner.height = height;
        }
    }

    /// Geometry the owner's pixels attest, if the owner tracks one.
    pub fn fingerprint(&self, id: u64) -> Option<GeometryFingerprint> {
        self.owners.get(&id).and_then(|owner| owner.fingerprint)
    }

    /// Re-attest an unchanged owner to the current revision: its pixels
    /// match the current geometry (the fingerprint was just verified),
    /// so its frames stamp current instead of going stale. Unlinked
    /// owners are untouched and keep following the current topology.
    pub fn bump_revision(&self, id: u64, revision: u64) {
        if let Some(owner) = self.owners.get(&id) {
            if let Some(shared) = &owner.revision {
                shared.store(revision, Ordering::Release);
            }
        }
    }

    /// Stop every owner outside `wanted` and remove them, ascending by id.
    /// Owners in `wanted` keep streaming. Returns the retired owners for
    /// joining their threads.
    pub fn retire_except(&mut self, wanted: &BTreeSet<u64>) -> Vec<(u64, SourceOwner)> {
        let mut stale: Vec<u64> = self
            .owners
            .keys()
            .copied()
            .filter(|id| !wanted.contains(id))
            .collect();
        stale.sort_unstable();
        let mut retired = Vec::with_capacity(stale.len());
        for id in stale {
            if let Some(owner) = self.remove(id) {
                retired.push((id, owner));
            }
        }
        retired
    }

    /// Stop and remove every owner.
    pub fn clear(&mut self) -> Vec<(u64, SourceOwner)> {
        self.owners
            .drain()
            .map(|(id, owner)| (id, owner.stopped()))
            .collect()
    }
}

/// Owner ids the supervisor must hold: every requested display that names a
/// live topology entry, plus the first entry (which serves unselected
/// viewers through the fanout default). Stale selections name nothing and
/// hold no owner: their viewers reselect on the next pointer event.
pub fn wanted_owners(requested: &[u64], displays: &[vnilink::DisplayInfo]) -> BTreeSet<u64> {
    let live: BTreeSet<u64> = displays
        .iter()
        .map(|display| display.id)
        .filter(|id| *id != 0)
        .collect();
    let mut wanted: BTreeSet<u64> = requested
        .iter()
        .copied()
        .filter(|id| live.contains(id))
        .collect();
    if let Some(first) = displays.iter().find(|display| display.id != 0) {
        wanted.insert(first.id);
    }
    wanted
}

/// Signature of one probed topology: the supervisor republishes only when
/// this changes. Rotation and scale are included: an orientation or scale
/// change alone re-attests every affected owner.
pub fn topology_signature(displays: &[vnilink::DisplayInfo]) -> Vec<(u64, GeometryFingerprint)> {
    displays
        .iter()
        .map(|display| (display.id, GeometryFingerprint::of(display)))
        .collect()
}

/// Wanted owners whose live display changed geometry: same id, different
/// pixels. Ascending. The supervisor retires and reopens exactly these;
/// matched owners keep serving. Owners without a fingerprint (fixed
/// grants) and ids outside `wanted` are never stale here.
pub fn stale_owners(
    owners: &SourceOwners,
    wanted: &BTreeSet<u64>,
    displays: &[vnilink::DisplayInfo],
) -> Vec<u64> {
    let live: HashMap<u64, GeometryFingerprint> = displays
        .iter()
        .map(|display| (display.id, GeometryFingerprint::of(display)))
        .collect();
    owners
        .ids()
        .into_iter()
        .filter(|id| wanted.contains(id))
        .filter(|id| {
            owners
                .fingerprint(*id)
                .is_some_and(|old| live.get(id).is_some_and(|current| *current != old))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn display(
        id: u64,
        x: i32,
        width: u16,
        height: u16,
        rotation_deg: u16,
        scale_percent: u16,
    ) -> vnilink::DisplayInfo {
        vnilink::DisplayInfo {
            id,
            origin_x: x,
            origin_y: 0,
            width,
            height,
            rotation_deg,
            scale_percent,
            revision: 1,
        }
    }

    fn owner_for(display: &vnilink::DisplayInfo, revision: u64) -> (SourceOwner, Arc<AtomicU64>) {
        let shared = Arc::new(AtomicU64::new(revision));
        let owner = SourceOwner::new(
            Arc::new(AtomicBool::new(false)),
            display.width,
            display.height,
        )
        .with_fingerprint(GeometryFingerprint::of(display))
        .with_revision(shared.clone());
        (owner, shared)
    }

    /// SPEC14: same-id move, resize, rotation and scale changes each mark
    /// exactly the affected owner stale; the matched sibling is kept.
    #[test]
    fn geometry_changes_mark_only_the_affected_owner_stale() {
        let left = display(1, 0, 1920, 1080, 0, 100);
        let right = display(2, 1920, 1920, 1080, 0, 100);
        let wanted: BTreeSet<u64> = [1, 2].into_iter().collect();
        let changed = [
            // Same-id move.
            display(2, 0, 1920, 1080, 0, 100),
            // Same-id resize.
            display(2, 1920, 1280, 720, 0, 100),
            // Same-id rotation with an identical footprint.
            display(2, 1920, 1920, 1080, 90, 100),
            // Same-id scale change.
            display(2, 1920, 1920, 1080, 0, 200),
        ];
        for moved in &changed {
            let mut owners = SourceOwners::new();
            let (left_owner, _) = owner_for(&left, 1);
            let (right_owner, _) = owner_for(&right, 1);
            owners.insert(1, left_owner);
            owners.insert(2, right_owner);
            assert_eq!(
                stale_owners(&owners, &wanted, &[left, *moved]),
                vec![2],
                "change not detected: {moved:?}"
            );
        }
        // Unchanged: nothing is stale.
        let mut owners = SourceOwners::new();
        let (left_owner, _) = owner_for(&left, 1);
        let (right_owner, _) = owner_for(&right, 1);
        owners.insert(1, left_owner);
        owners.insert(2, right_owner);
        assert!(stale_owners(&owners, &wanted, &[left, right]).is_empty());
    }

    /// SPEC14: fixed grants (no fingerprint) and unwanted owners are never
    /// reported stale; only wanted fingerprinted owners are compared.
    #[test]
    fn fixed_and_unwanted_owners_are_never_stale() {
        let live = display(1, 0, 1920, 1080, 0, 100);
        let moved = display(1, 100, 1920, 1080, 0, 100);
        let wanted: BTreeSet<u64> = [1].into_iter().collect();
        // Fixed grant despite moved geometry.
        let mut owners = SourceOwners::new();
        owners.insert(
            1,
            SourceOwner::new(Arc::new(AtomicBool::new(false)), 1920, 1080),
        );
        assert!(stale_owners(&owners, &wanted, &[moved]).is_empty());
        // Unwanted owner despite tracked fingerprint.
        let mut owners = SourceOwners::new();
        let (tracked, _) = owner_for(&live, 1);
        owners.insert(1, tracked);
        assert!(stale_owners(&owners, &BTreeSet::new(), &[moved]).is_empty());
    }

    /// SPEC14: bumping re-attests a linked owner through the shared
    /// revision; unlinked owners are untouched.
    #[test]
    fn bump_revision_reattests_only_linked_owners() {
        let live = display(1, 0, 1920, 1080, 0, 100);
        let mut owners = SourceOwners::new();
        let (linked, shared) = owner_for(&live, 7);
        owners.insert(1, linked);
        owners.insert(
            2,
            SourceOwner::new(Arc::new(AtomicBool::new(false)), 640, 480),
        );
        assert_eq!(shared.load(Ordering::Acquire), 7);
        owners.bump_revision(1, 10);
        assert_eq!(shared.load(Ordering::Acquire), 10);
        owners.bump_revision(2, 10);
        owners.bump_revision(9, 10);
        assert_eq!(shared.load(Ordering::Acquire), 10);
    }

    /// SPEC14: the published signature includes rotation and scale, so an
    /// orientation-only change republishes.
    #[test]
    fn topology_signature_covers_rotation_and_scale() {
        let base = display(1, 0, 1920, 1080, 0, 100);
        let rotated = display(1, 0, 1920, 1080, 90, 100);
        let scaled = display(1, 0, 1920, 1080, 0, 200);
        assert_eq!(topology_signature(&[base]), topology_signature(&[base]));
        assert_ne!(topology_signature(&[base]), topology_signature(&[rotated]));
        assert_ne!(topology_signature(&[base]), topology_signature(&[scaled]));
    }
}
