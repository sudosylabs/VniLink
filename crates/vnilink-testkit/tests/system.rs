//! System actions alone: a controller asking the host to lock, restart,
//! shut down or keep its screen private, under the host's policy.

use expect_test::expect;
use std::sync::Arc;
use std::time::Duration;

use vnilink::{OperationState, SystemAction, SystemPolicy};
use vnilink_testkit::system::{RecordingSystem, SystemSession};
use vnilink_testkit::{traced, wait_for, DEFAULT_WAIT};

fn allow(session: &SystemSession, policy: SystemPolicy) {
    session.pair.host.core.set_system_policy(policy).unwrap();
}

/// A lock runs at once and says so; restart and shutdown stay refused
/// until the host allows them; each outcome is recorded.
#[test]
fn a_lock_runs_and_a_restart_waits_for_the_host_s_policy() {
    let trace = traced();
    let system = Arc::new(RecordingSystem::default());
    let session = SystemSession::open(Some(Arc::clone(&system)));
    let locked = session.service.execute(SystemAction::Lock).unwrap();
    assert_eq!(locked.state, OperationState::Executed);
    assert_eq!(system.asked().locks, 1);
    let restart = session.service.execute(SystemAction::Restart).unwrap();
    assert_eq!(restart.state, OperationState::Refused);
    assert_eq!(system.asked().restarts, 0);

    expect![[r#"
        host:
          system: system action ended action=Lock operation=1 scheduled=false state=Executed
          system: system action refused action=Restart connection=1 detail=disabled by host policy operation=2
    "#]].assert_eq(&trace.story(&["system"]));
}

/// An allowed restart waits out its notice, can be cancelled meanwhile,
/// and otherwise runs.
#[test]
fn an_allowed_restart_waits_its_notice_and_can_be_cancelled() {
    let _trace = traced();
    let system = Arc::new(RecordingSystem::default());
    let session = SystemSession::open(Some(Arc::clone(&system)));
    allow(
        &session,
        SystemPolicy {
            allow_restart: true,
            ..SystemPolicy::default()
        },
    );
    session
        .pair
        .host
        .core
        .set_system_action_notice(Duration::from_secs(30));
    let restart = session.service.execute(SystemAction::Restart).unwrap();
    assert_eq!(restart.state, OperationState::Scheduled);
    let cancelled = session.service.cancel(restart.id).unwrap();
    assert_eq!(cancelled.state, OperationState::Cancelled);
    assert_eq!(system.asked().restarts, 0);

    session
        .pair
        .host
        .core
        .set_system_action_notice(Duration::ZERO);
    let restart = session.service.execute(SystemAction::Restart).unwrap();
    wait_for("the restart to run", DEFAULT_WAIT, || {
        (session.service.operation(restart.id).ok()?.state == OperationState::Executed)
            .then_some(())
    });
    assert_eq!(system.asked().restarts, 1);
}

/// Without an operating system backend every action is refused, and says why.
#[test]
fn without_a_backend_actions_are_refused_and_say_why() {
    let trace = traced();
    let session = SystemSession::open(None);
    let refused = session.service.execute(SystemAction::Lock).unwrap();
    assert_eq!(refused.state, OperationState::Refused);
    expect![[r#"
        host:
          system: system action refused action=Lock connection=1 detail=no system backend operation=1
    "#]].assert_eq(&trace.story(&["system"]));
}

/// Privacy hides the screen and blocks input while held, and closing the
/// service gives both back.
#[test]
fn privacy_holds_until_the_service_closes() {
    let trace = traced();
    let system = Arc::new(RecordingSystem::default());
    let session = SystemSession::open(Some(Arc::clone(&system)));
    allow(
        &session,
        SystemPolicy {
            allow_privacy: true,
            ..SystemPolicy::default()
        },
    );
    let held = session.service.set_privacy(true, true).unwrap();
    assert!(held.display_private && held.input_blocked);
    assert!(system.asked().display_private && system.asked().input_blocked);
    session.service.close();
    wait_for("privacy to be given back", DEFAULT_WAIT, || {
        let asked = system.asked();
        (!asked.display_private && !asked.input_blocked).then_some(())
    });
    trace.wait_for_records("privacy restored", 1);
    expect![[r#"
        host:
          system: privacy applied connection=1 display_ok=true display_private=true input_blocked=true input_ok=true
          system: privacy restored connection=1 display=true input=true
    "#]].assert_eq(&trace.story(&["system"]));
}
