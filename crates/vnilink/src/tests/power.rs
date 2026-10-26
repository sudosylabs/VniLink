use crate::power::*;
use std::time::{Duration, Instant};

/// Pure policy, no network: the machine never fires unused, fires once on
/// an explicit end with a live parent, waits out the grace without one, and
/// a redial inside the window resumes instead of locking.
#[test]
fn lock_machine_fires_once_and_graces_parent_loss() {
    let start = Instant::now();
    let mut machine = LockAfterControl::default();
    // tick(now, enabled, controlling, used, parent_live)
    assert_eq!(
        machine.tick(start, true, false, false, false),
        LockDecision::Idle
    );
    assert_eq!(
        machine.tick(start, true, true, true, true),
        LockDecision::Idle
    );
    assert_eq!(
        machine.tick(start, false, false, false, true),
        LockDecision::Idle
    );
    assert_eq!(
        machine.tick(start, true, true, true, true),
        LockDecision::Idle
    );
    assert_eq!(
        machine.tick(start, true, false, false, true),
        LockDecision::Fire
    );
    assert_eq!(
        machine.tick(start, true, false, false, true),
        LockDecision::Idle
    );
    assert_eq!(
        machine.tick(start, true, true, true, false),
        LockDecision::Idle
    );
    assert_eq!(
        machine.tick(start, true, false, false, false),
        LockDecision::Wait
    );
    assert_eq!(
        machine.tick(start + Duration::from_secs(29), true, false, false, false),
        LockDecision::Wait
    );
    assert_eq!(
        machine.tick(start + Duration::from_secs(30), true, false, false, false),
        LockDecision::Fire
    );
    assert_eq!(
        machine.tick(start, true, true, true, false),
        LockDecision::Idle
    );
    assert_eq!(
        machine.tick(start, true, false, false, false),
        LockDecision::Wait
    );
    assert_eq!(
        machine.tick(start + Duration::from_secs(5), true, true, false, true),
        LockDecision::Idle
    );
    assert_eq!(
        machine.tick(start + Duration::from_secs(31), true, false, false, true),
        LockDecision::Fire
    );
    // A use and an end within one tick still fire on the next sample.
    let mut quick = LockAfterControl::default();
    assert_eq!(
        quick.tick(start, true, false, true, true),
        LockDecision::Fire
    );
}

/// Control granted but never used, such as a viewer cancelled while still
/// connecting, ends without locking; using it once arms the lock until
/// control ends.
#[test]
fn lock_machine_ignores_control_that_was_never_used() {
    let start = Instant::now();
    let mut machine = LockAfterControl::default();
    assert_eq!(
        machine.tick(start, true, true, false, true),
        LockDecision::Idle
    );
    assert_eq!(
        machine.tick(start, true, false, false, true),
        LockDecision::Idle
    );
    assert_eq!(
        machine.tick(start, true, false, false, false),
        LockDecision::Idle
    );
    assert_eq!(
        machine.tick(start, true, true, true, true),
        LockDecision::Idle
    );
    assert_eq!(
        machine.tick(start, true, true, false, true),
        LockDecision::Idle
    );
    assert_eq!(
        machine.tick(start, true, false, false, true),
        LockDecision::Fire
    );
}
