//! A controller's lock, restart and shutdown: what the host's grant, policy
//! and backend let run, the notice a restart waits out, who may cancel it,
//! and what a connection keeps of its operations.

use expect_test::expect;
use std::sync::Arc;
use std::time::Duration;

use vnilink::{
    AvailabilityReason, Capability, CoreEvent, Error, OperationState, ServiceKind, SessionPhase,
    SystemAction, SystemPolicy, SystemSupport,
};
use vnilink_testkit::system::FakeSystem;
use vnilink_testkit::system::{RecordingSystem, SystemSession};
use vnilink_testkit::{holds, traced, wait_for, Pair, TestResult, DEFAULT_WAIT};

use super::sharing;

/// The host's policy allows restart and shutdown besides the lock.
const RESTART_AND_SHUTDOWN: SystemPolicy = SystemPolicy {
    allow_lock: true,
    allow_restart: true,
    allow_shutdown: true,
    allow_privacy: false,
};

/// The host allows restart and shutdown, after a notice of `notice`.
fn allow_restart(pair: &Pair, notice: Duration) -> Result<(), Error> {
    pair.host.core.set_system_policy(RESTART_AND_SHUTDOWN)?;
    pair.host.core.set_system_action_notice(notice);
    Ok(())
}

/// Without the host's grant the service does not open. Granted without an
/// operating system backend, it opens to say what the host supports, which
/// is nothing, and every action is refused, says why, and is kept as an
/// operation to look up.
#[test]
fn a_service_opens_only_when_shared_and_without_a_backend_refuses_and_says_why() -> TestResult {
    let trace = traced();
    let pair = Pair::for_desktop();
    let connection = pair.connect_services();
    let owner = pair.attach(&connection, ServiceKind::System);
    assert!(
        matches!(
            pair.controller
                .core
                .open_system_service(owner.id, owner.generation),
            Err(Error::Unauthorized)
        ),
        "a System service the host does not share does not open"
    );
    pair.host
        .core
        .set_service_sharing(ServiceKind::System, true)?;
    let system = pair
        .controller
        .core
        .open_system_service(owner.id, owner.generation)?;
    let (support, session) = system.support()?;
    assert_eq!(
        support,
        SystemSupport::default(),
        "a host without a backend supports nothing"
    );
    assert_eq!(
        session.locked, None,
        "a host without a backend does not know whether it is locked"
    );
    let refused = system.execute(SystemAction::Lock)?;
    assert_eq!(
        refused.state,
        OperationState::Refused,
        "an action without a backend is refused"
    );
    assert_eq!(
        system.operation(refused.id)?,
        refused,
        "a refused action is kept as an operation"
    );
    assert!(
        matches!(system.operation(refused.id + 100), Err(Error::NoSession)),
        "an operation never asked for is not found"
    );
    expect![[r#"
        host:
          system: system action refused action=Lock connection=1 detail=no system backend operation=1
    "#]].assert_eq(&trace.story(&["system"]));
    Ok(())
}

/// A lock runs at once and says so; restart and shutdown stay refused
/// until the host allows them, then wait out a notice, here a zero-length
/// one, and run; each outcome is recorded.
#[test]
fn a_lock_runs_and_a_restart_waits_for_the_host_s_policy() -> TestResult {
    let trace = traced();
    let system = Arc::new(RecordingSystem::default());
    let session = SystemSession::open(Some(Arc::clone(&system)));
    let host = &session.pair.host.core;
    let locked = session.service.execute(SystemAction::Lock)?;
    assert_eq!(
        locked.state,
        OperationState::Executed,
        "a lock runs at once"
    );
    assert!(
        locked.completion_observable,
        "a lock's completion is observable"
    );
    assert_eq!(
        system.asked().locks,
        1,
        "the host's OS was asked to lock once"
    );
    assert_eq!(
        session.service.operation(locked.id)?,
        locked,
        "an executed lock is kept as it ended"
    );
    let restart = session.service.execute(SystemAction::Restart)?;
    assert_eq!(
        restart.state,
        OperationState::Refused,
        "a restart the host's policy does not allow is refused"
    );
    let shutdown = session.service.execute(SystemAction::Shutdown)?;
    assert_eq!(
        shutdown.state,
        OperationState::Refused,
        "a shutdown the host's policy does not allow is refused"
    );
    assert_eq!(
        (system.asked().restarts, system.asked().shutdowns),
        (0, 0),
        "a refused action never reaches the host's OS"
    );

    host.set_system_policy(RESTART_AND_SHUTDOWN)?;
    host.set_system_action_notice(Duration::ZERO);
    let restart = session.service.execute(SystemAction::Restart)?;
    assert_eq!(
        restart.state,
        OperationState::Scheduled,
        "an allowed restart waits out its notice"
    );
    assert!(
        !restart.completion_observable,
        "a restart's completion is not observable"
    );
    wait_for("the restart to run", DEFAULT_WAIT, || {
        (session.service.operation(restart.id).ok()?.state == OperationState::Executed)
            .then_some(())
    });
    assert_eq!(system.asked().restarts, 1, "the host's OS restarted once");
    let shutdown = session.service.execute(SystemAction::Shutdown)?;
    wait_for("the shutdown to run", DEFAULT_WAIT, || {
        (session.service.operation(shutdown.id).ok()?.state == OperationState::Executed)
            .then_some(())
    });
    assert_eq!(system.asked().shutdowns, 1, "the host's OS shut down once");
    trace.wait_for_records("system action ended", 3);
    expect![[r#"
        host:
          system: system action admitted action=Lock connection=1 operation=1 scheduled=false
          system: system action ended action=Lock operation=1 scheduled=false state=Executed
          system: system action refused action=Restart connection=1 detail=disabled by host policy operation=2
          system: system action refused action=Shutdown connection=1 detail=disabled by host policy operation=3
          system: system action admitted action=Restart connection=1 operation=4 scheduled=true
          system: system action ended action=Restart operation=4 scheduled=true state=Executed
          system: system action admitted action=Shutdown connection=1 operation=5 scheduled=true
          system: system action ended action=Shutdown operation=5 scheduled=true state=Executed
    "#]]
    .assert_eq(&trace.story(&["system"]));
    Ok(())
}

/// An allowed restart waits out its notice, can be cancelled meanwhile,
/// and otherwise runs.
#[test]
fn an_allowed_restart_waits_its_notice_and_can_be_cancelled() {
    let _trace = traced();
    let system = Arc::new(RecordingSystem::default());
    let session = SystemSession::open(Some(Arc::clone(&system)));
    session
        .pair
        .host
        .core
        .set_system_policy(SystemPolicy {
            allow_restart: true,
            ..SystemPolicy::default()
        })
        .unwrap();
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

/// A lock the host's OS never answers fails the viewer's request, as
/// GNOME's does on a screen already locked, but no longer holds the host's
/// system service: the viewer's next request is served.
#[test]
fn an_action_that_never_answers_does_not_hold_the_service() -> TestResult {
    let trace = traced();
    let fake = Arc::new(FakeSystem::full());
    let pair = sharing(&fake)?;
    allow_restart(&pair, Duration::ZERO)?;
    let controller = &pair.controller.core;
    let connection = pair.connect_services();
    let _stalled = fake.stall_locks();
    let system = pair.controller.open_system(&connection)?;
    let locked = system.execute(SystemAction::Lock);
    assert!(
        locked.is_err(),
        "a lock the OS never answers fails the request: {locked:?}"
    );
    drop(system);
    // As the engine does after a failed request: a fresh service, closed
    // again while the host refuses it.
    let restarted = wait_for(
        "the host to serve another request",
        Duration::from_secs(8),
        || {
            // The service the stalled request held may still be on the
            // connection: busy, as opening it may be, means not yet.
            let owner = match controller.attach_controller_service(
                connection.id,
                connection.generation,
                ServiceKind::System,
            ) {
                Ok(owner) => owner,
                Err(Error::Busy) => return None,
                Err(error) => panic!("the connection refused the service otherwise: {error:?}"),
            };
            match controller.open_system_service(owner.id, owner.generation) {
                Ok(system) => Some(system.execute(SystemAction::Restart)),
                Err(Error::Busy) => {
                    controller
                        .close_controller_service(owner.id, owner.generation)
                        .expect("the refused service closes");
                    controller
                        .forget_controller_service(owner.id)
                        .expect("the refused service is forgotten");
                    None
                }
                Err(error) => panic!("the host refused the service otherwise: {error:?}"),
            }
        },
    )?;
    assert_eq!(
        restarted.state,
        OperationState::Scheduled,
        "the request after a stalled lock is served"
    );
    trace.wait_for_records("system action ended", 1);
    expect![[r#"
        host:
          system: system action admitted action=Lock connection=1 operation=1 scheduled=false
          system: system action admitted action=Restart connection=1 operation=2 scheduled=true
          system: system action ended action=Restart operation=2 scheduled=true state=Executed
    "#]]
    .assert_eq(&trace.story(&["system"]));
    Ok(())
}

/// An allowed restart is scheduled with the notice it waits out, which the
/// host lists with the computer that asked, runs nothing during it, then
/// runs, and tells the host's application both.
#[test]
fn a_restart_waits_out_its_notice_and_tells_the_host() -> TestResult {
    let trace = traced();
    let fake = Arc::new(FakeSystem::full());
    let pair = sharing(&fake)?;
    allow_restart(&pair, Duration::from_millis(600))?;
    let (host, controller) = (&pair.host.core, &pair.controller.core);
    let events = host.subscribe()?;
    let connection = pair.connect_services();
    let system = pair.controller.open_system(&connection)?;
    let restart = system.execute(SystemAction::Restart)?;
    assert_eq!(
        restart.state,
        OperationState::Scheduled,
        "an allowed restart waits out its notice"
    );
    assert!(
        restart.notice_remaining_ms > 0 && restart.notice_remaining_ms <= 600,
        "the restart says what is left of its notice: {}ms",
        restart.notice_remaining_ms
    );
    let scheduled = host.scheduled_system_actions()?;
    assert_eq!(scheduled.len(), 1, "the host lists the scheduled restart");
    assert_eq!(scheduled[0].operation_id, restart.id);
    assert_eq!(
        scheduled[0].endpoint_id,
        controller.endpoint_id(),
        "the host knows which computer asked"
    );
    holds(
        "nothing runs during the notice",
        Duration::from_millis(200),
        || fake.calls().restarts == 0,
    );
    wait_for("the restart to run", DEFAULT_WAIT, || {
        (system.operation(restart.id).ok()?.state == OperationState::Executed).then_some(())
    });
    assert_eq!(fake.calls().restarts, 1, "the host's OS restarted once");
    assert!(
        host.scheduled_system_actions()?.is_empty(),
        "a restart that ran is no longer listed"
    );
    let mut seen = Vec::new();
    wait_for("the host to hear of both", DEFAULT_WAIT, || {
        while let Some(event) = events.try_recv() {
            match event {
                CoreEvent::SystemActionScheduled {
                    operation_id,
                    action,
                    endpoint_id,
                    ..
                } => seen.push(format!(
                    "scheduled {operation_id} {action:?} {}",
                    endpoint_id == controller.endpoint_id()
                )),
                CoreEvent::SystemActionEnded {
                    operation_id,
                    state,
                } => seen.push(format!("ended {operation_id} {state:?}")),
                _ => {}
            }
        }
        (seen.len() == 2).then_some(())
    });
    assert_eq!(
        seen,
        [
            format!("scheduled {} Restart true", restart.id),
            format!("ended {} Executed", restart.id)
        ],
        "the host's application hears the restart scheduled and then run"
    );
    trace.wait_for_records("system action ended", 1);
    expect![[r#"
        host:
          system: system action admitted action=Restart connection=1 operation=1 scheduled=true
          system: system action ended action=Restart operation=1 scheduled=true state=Executed
    "#]]
    .assert_eq(&trace.story(&["system"]));
    Ok(())
}

/// The person at the host and the computer that asked can each cancel a
/// scheduled action, which says who did and stays cancelled.
#[test]
fn either_computer_can_cancel_a_scheduled_restart() -> TestResult {
    let _trace = traced();
    let fake = Arc::new(FakeSystem::full());
    let pair = sharing(&fake)?;
    allow_restart(&pair, Duration::from_secs(30))?;
    let connection = pair.connect_services();
    let system = pair.controller.open_system(&connection)?;
    // At the host, as its user chooses in the notice.
    let first = system.execute(SystemAction::Restart)?;
    let cancelled = pair.host.core.cancel_system_action(first.id)?;
    assert_eq!(cancelled.state, OperationState::Cancelled);
    assert_eq!(
        cancelled.detail, "cancelled at that computer",
        "a cancel at the host says so"
    );
    assert_eq!(
        system.operation(first.id)?.state,
        OperationState::Cancelled,
        "the computer that asked sees the host's cancel"
    );
    // From the computer that asked.
    let second = system.execute(SystemAction::Shutdown)?;
    let cancelled = system.cancel(second.id)?;
    assert_eq!(cancelled.state, OperationState::Cancelled);
    assert_eq!(
        cancelled.detail, "cancelled by the computer that asked",
        "a cancel by the computer that asked says so"
    );
    assert_eq!(
        system.cancel(second.id)?,
        cancelled,
        "an operation that already ended stays as it is"
    );
    holds("nothing cancelled runs", Duration::from_millis(300), || {
        let calls = fake.calls();
        (calls.restarts, calls.shutdowns) == (0, 0)
    });
    Ok(())
}

/// Another paired computer cannot cancel a restart it did not ask for;
/// the person at the host can start it at once, and only once.
#[test]
fn another_connection_cannot_cancel_and_run_now_starts_at_once() -> TestResult {
    let trace = traced();
    let fake = Arc::new(FakeSystem::full());
    let pair = sharing(&fake)?;
    allow_restart(&pair, Duration::from_secs(30))?;
    let host = &pair.host.core;
    let connection = pair.connect_services();
    let system = pair.controller.open_system(&connection)?;
    let restart = system.execute(SystemAction::Restart)?;
    // A second paired computer asks to cancel someone else's restart.
    let other = pair.add_viewer("other");
    let other_connection = other.connect_services(&pair.host)?;
    let other_system = other.open_system(&other_connection)?;
    assert!(
        other_system.cancel(restart.id).is_err(),
        "another computer cannot cancel the restart"
    );
    assert_eq!(
        system.operation(restart.id)?.state,
        OperationState::Scheduled,
        "the restart stays scheduled"
    );
    // The person at the computer starts it sooner.
    host.run_system_action_now(restart.id)?;
    wait_for("the restart to run", DEFAULT_WAIT, || {
        (system.operation(restart.id).ok()?.state == OperationState::Executed).then_some(())
    });
    assert_eq!(fake.calls().restarts, 1, "the host's OS restarted once");
    assert!(
        host.run_system_action_now(restart.id).is_err(),
        "a restart that ran does not run again"
    );
    trace.wait_for_records("system action ended", 1);
    expect![[r#"
        host:
          system: system action admitted action=Restart connection=1 operation=1 scheduled=true
          system: system action ended action=Restart operation=1 scheduled=true state=Executed
    "#]]
    .assert_eq(&trace.story(&["system"]));
    Ok(())
}

/// A scheduled restart ends when the computer that asked disconnects, and
/// is refused when the host's policy stops allowing it during its notice;
/// a lock still runs at once.
#[test]
fn a_restart_ends_when_the_computer_that_asked_leaves_or_policy_changes() -> TestResult {
    let trace = traced();
    let fake = Arc::new(FakeSystem::full());
    let pair = sharing(&fake)?;
    allow_restart(&pair, Duration::from_secs(30))?;
    let (host, controller) = (&pair.host.core, &pair.controller.core);
    let connection = pair.connect_services();
    let system = pair.controller.open_system(&connection)?;
    let restart = system.execute(SystemAction::Restart)?;
    controller.disconnect_session(connection.id)?;
    wait_for(
        "the restart to end with its connection",
        DEFAULT_WAIT,
        || {
            host.scheduled_system_actions()
                .ok()?
                .iter()
                .all(|action| action.operation_id != restart.id)
                .then_some(())
        },
    );
    // Policy turned off during the notice: the start gate refuses it.
    allow_restart(&pair, Duration::from_millis(300))?;
    let connection = pair.controller.connect_services(&pair.host)?;
    let system = pair.controller.open_system(&connection)?;
    let restart = system.execute(SystemAction::Restart)?;
    host.set_system_policy(SystemPolicy::default())?;
    let refused = wait_for("the restart to be refused", DEFAULT_WAIT, || {
        let operation = system.operation(restart.id).ok()?;
        (operation.state == OperationState::Refused).then_some(operation)
    });
    assert_eq!(
        refused.detail, "disabled by host policy",
        "the refused restart says why"
    );
    assert_eq!(fake.calls().restarts, 0, "no restart reached the host's OS");
    let locked = system.execute(SystemAction::Lock)?;
    assert_eq!(locked.state, OperationState::Executed, "a lock still runs");
    expect![[r#"
        host:
          system: system action admitted action=Restart connection=1 operation=1 scheduled=true
          system: system action cancelled action=Restart by_viewer=false detail=the computer that asked disconnected operation=1
          system: system action admitted action=Restart connection=2 operation=2 scheduled=true
          system: system action ended action=Restart operation=2 scheduled=true state=Refused stood_down=disabled by host policy
          system: system action admitted action=Lock connection=2 operation=3 scheduled=false
          system: system action ended action=Lock operation=3 scheduled=false state=Executed
    "#]].assert_eq(&trace.story(&["system"]));
    Ok(())
}

/// An action the host's OS refuses is a failed operation that says so,
/// kept to look up, not a lost request.
#[test]
fn backend_failure_is_a_failed_operation_not_a_lost_request() -> TestResult {
    let trace = traced();
    let fake = Arc::new(FakeSystem::full());
    fake.calls().fail_lock = true;
    let pair = sharing(&fake)?;
    let connection = pair.connect_services();
    let system = pair.controller.open_system(&connection)?;
    let failed = system.execute(SystemAction::Lock)?;
    assert_eq!(failed.state, OperationState::Failed, "a refused lock fails");
    assert!(
        failed.detail.contains("refused"),
        "the failed lock says the OS refused it: {}",
        failed.detail
    );
    assert_eq!(fake.calls().locks, 0, "the host did not lock");
    assert_eq!(
        system.operation(failed.id)?,
        failed,
        "the failed lock is kept as it ended"
    );
    expect![[r#"
        host:
          system: system action admitted action=Lock connection=1 operation=1 scheduled=false
          system: system action ended action=Lock error=unsupported operation=1 scheduled=false state=Failed
    "#]].assert_eq(&trace.story(&["system"]));
    Ok(())
}

/// A connection's operations stay with it: after a reconnect the previous
/// one's lock cannot be looked up, and a new lock is a new operation.
#[test]
fn operations_never_replay_across_reconnect() -> TestResult {
    let trace = traced();
    let fake = Arc::new(FakeSystem::full());
    let pair = sharing(&fake)?;
    let controller = &pair.controller.core;
    let connection = pair.connect_services();
    let system = pair.controller.open_system(&connection)?;
    let first = system.execute(SystemAction::Lock)?;
    assert_eq!(first.state, OperationState::Executed);
    controller.disconnect_session(connection.id)?;
    wait_for("the connection to close", DEFAULT_WAIT, || {
        controller
            .controller_session(connection.id)
            .is_ok_and(|session| session.phase == SessionPhase::Closed)
            .then_some(())
    });
    drop(system);
    let connection = pair.controller.connect_services(&pair.host)?;
    assert!(
        connection.generation > 0,
        "the reconnect is a later generation"
    );
    let system = pair.controller.open_system(&connection)?;
    assert!(
        matches!(system.operation(first.id), Err(Error::NoSession)),
        "the previous connection's operation is unreachable, not resumed"
    );
    let second = system.execute(SystemAction::Lock)?;
    assert_eq!(second.state, OperationState::Executed);
    assert_ne!(second.id, first.id, "a new lock is a new operation");
    expect![[r#"
        host:
          system: system action admitted action=Lock connection=1 operation=1 scheduled=false
          system: system action ended action=Lock operation=1 scheduled=false state=Executed
          system: system action admitted action=Lock connection=2 operation=2 scheduled=false
          system: system action ended action=Lock operation=2 scheduled=false state=Executed
    "#]]
    .assert_eq(&trace.story(&["system"]));
    Ok(())
}

/// Whether an action is available follows what the host's OS supports,
/// not whether it has a backend; a supported action reports its real gate,
/// the host's policy and then the peer's grant.
#[test]
fn system_availability_follows_native_bits_not_adapter_presence() -> TestResult {
    let _trace = traced();
    let pair = Pair::for_desktop();
    let (host, controller) = (&pair.host.core, &pair.controller.core);
    // Mac/Windows-shaped mix: lock and shutdown exist, restart does not,
    // and only input blocking backs privacy.
    let fake = Arc::new(FakeSystem::supporting(SystemSupport {
        lock: true,
        restart: false,
        shutdown: true,
        privacy: false,
        input_block: true,
    }));
    host.set_system_actions(Some(
        Arc::clone(&fake) as Arc<dyn vnilink::HostSystemActions>
    ))?;
    let everything = SystemPolicy {
        allow_lock: true,
        allow_restart: true,
        allow_shutdown: true,
        allow_privacy: true,
    };
    host.set_system_policy(everything)?;
    let query = |capability| host.capability_availability(None, capability);
    assert!(
        query(Capability::SystemLock)?.available,
        "lock is available"
    );
    assert!(
        query(Capability::SystemShutdown)?.available,
        "shutdown is available"
    );
    assert!(
        query(Capability::SystemPrivacy)?.available,
        "input blocking alone backs privacy"
    );
    let restart = query(Capability::SystemRestart)?;
    assert!(!restart.available, "an unsupported restart is unavailable");
    assert_eq!(
        restart.reasons.first(),
        Some(&AvailabilityReason::MissingImplementation),
        "an unsupported restart says it is missing"
    );
    assert!(
        fake.support_asked() >= 4,
        "each query asks the OS what it supports: {} asked",
        fake.support_asked()
    );
    host.set_system_policy(SystemPolicy {
        allow_lock: false,
        ..everything
    })?;
    let lock = query(Capability::SystemLock)?;
    assert!(!lock.available, "a lock the policy disables is unavailable");
    assert_eq!(
        lock.reasons.first(),
        Some(&AvailabilityReason::DisabledLocalPolicy),
        "a supported lock names the host's policy first"
    );
    let peer = controller.endpoint_id();
    let lock = host.capability_availability(Some(&peer), Capability::SystemLock)?;
    assert!(!lock.available);
    assert!(
        lock.reasons.contains(&AvailabilityReason::DeniedPeerGrant),
        "a supported lock names the peer's grant"
    );
    assert!(
        !lock
            .reasons
            .contains(&AvailabilityReason::MissingImplementation),
        "a supported lock is never said to be missing"
    );
    // Nothing backing privacy reads missing.
    host.set_system_actions(Some(Arc::new(FakeSystem::supporting(SystemSupport {
        lock: true,
        restart: true,
        shutdown: true,
        privacy: false,
        input_block: false,
    }))))?;
    let privacy = query(Capability::SystemPrivacy)?;
    assert!(
        !privacy.available,
        "privacy with nothing backing it is unavailable"
    );
    assert_eq!(
        privacy.reasons.first(),
        Some(&AvailabilityReason::MissingImplementation),
        "privacy with nothing backing it says it is missing"
    );
    Ok(())
}
