use crate::owners::{wanted_owners, SourceOwner, SourceOwners};
use std::collections::BTreeSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

fn owner() -> (SourceOwner, Arc<AtomicBool>) {
    let stop = Arc::new(AtomicBool::new(false));
    (SourceOwner::new(Arc::clone(&stop), 1920, 1080), stop)
}

fn is_stopped(stop: &Arc<AtomicBool>) -> bool {
    stop.load(Ordering::Acquire)
}

fn display(id: u64) -> vnilink::DisplayInfo {
    vnilink::DisplayInfo {
        id,
        origin_x: 0,
        origin_y: 0,
        width: 1920,
        height: 1080,
        rotation_deg: 0,
        scale_percent: 100,
        revision: 3,
    }
}

#[test]
fn owners_open_and_close_independently() {
    let mut owners = SourceOwners::new();
    assert!(owners.is_empty());
    let (left, left_stop) = owner();
    let (right, right_stop) = owner();
    assert!(owners.insert(1, left).is_none());
    assert!(owners.insert(2, right).is_none());
    assert_eq!(owners.len(), 2);
    assert_eq!(owners.ids(), BTreeSet::from([1, 2]));
    assert_eq!(owners.get(2).unwrap().width, 1920);

    let removed = owners.remove(1).unwrap();
    assert!(is_stopped(&removed.stop));
    assert!(is_stopped(&left_stop));
    assert!(!is_stopped(&right_stop));
    assert!(!owners.contains(1));
    assert!(owners.contains(2));
    assert!(owners.remove(1).is_none());

    let cleared = owners.clear();
    assert_eq!(cleared.len(), 1);
    assert!(is_stopped(&right_stop));
    assert!(owners.is_empty());
}

#[test]
fn insert_displaces_without_touching_siblings() {
    let mut owners = SourceOwners::new();
    let (first, first_stop) = owner();
    let (sibling, sibling_stop) = owner();
    owners.insert(1, first);
    owners.insert(2, sibling);
    let (replacement, replacement_stop) = owner();
    let displaced = owners.insert(1, replacement).unwrap();
    assert!(is_stopped(&displaced.stop));
    assert!(is_stopped(&first_stop));
    assert!(!is_stopped(&replacement_stop));
    assert!(!is_stopped(&sibling_stop));
    assert_eq!(owners.len(), 2);
}

#[test]
fn retire_except_keeps_wanted_and_stops_the_rest_in_order() {
    let mut owners = SourceOwners::new();
    let mut stops = Vec::new();
    for id in [1, 2, 3] {
        let (owner, stop) = owner();
        owners.insert(id, owner);
        stops.push(stop);
    }
    let retired = owners.retire_except(&BTreeSet::from([2]));
    assert_eq!(
        retired.iter().map(|(id, _)| *id).collect::<Vec<_>>(),
        vec![1, 3]
    );
    assert!(is_stopped(&stops[0]));
    assert!(!is_stopped(&stops[1]));
    assert!(is_stopped(&stops[2]));
    assert_eq!(owners.ids(), BTreeSet::from([2]));

    // Retiring everything empties the registry.
    let retired = owners.retire_except(&BTreeSet::new());
    assert_eq!(retired.len(), 1);
    assert!(is_stopped(&stops[1]));
    assert!(owners.is_empty());
}

#[test]
fn update_size_fills_the_pending_owner_without_signalling() {
    let mut owners = SourceOwners::new();
    let stop = Arc::new(AtomicBool::new(false));
    owners.insert(1, SourceOwner::new(Arc::clone(&stop), 0, 0));
    owners.update_size(1, 1920, 1080);
    let owner = owners.get(1).unwrap();
    assert_eq!((owner.width, owner.height), (1920, 1080));
    assert!(!is_stopped(&stop));
    // A vanished id is a no-op, never a new owner.
    owners.update_size(9, 800, 600);
    assert!(!owners.contains(9));
}

#[test]
fn wanted_owners_binds_requested_and_first_live_display() {
    let twins = [display(1), display(2)];
    // One selection holds its source plus the default that serves
    // unselected viewers.
    assert_eq!(wanted_owners(&[2], &twins), BTreeSet::from([1, 2]));
    // Nothing requested: only the default streams (bootstrap).
    assert_eq!(wanted_owners(&[], &twins), BTreeSet::from([1]));
    // Stale selections name nothing and hold no owner.
    assert_eq!(wanted_owners(&[9], &twins), BTreeSet::from([1]));
    assert_eq!(wanted_owners(&[2, 9], &twins), BTreeSet::from([1, 2]));
    // Zero ids never name an owner, even first.
    assert_eq!(wanted_owners(&[0], &twins), BTreeSet::from([1]));
    let mut zero_first = display(0);
    zero_first.width = 800;
    assert_eq!(
        wanted_owners(&[], &[zero_first, display(4)]),
        BTreeSet::from([4])
    );
    // No live topology holds nothing.
    assert!(wanted_owners(&[1], &[]).is_empty());
    assert!(wanted_owners(&[], &[]).is_empty());
}
