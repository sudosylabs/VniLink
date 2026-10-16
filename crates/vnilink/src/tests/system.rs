use super::support::{pair_for_control, start_direct};
use crate::system::truncate_detail;
use crate::{
    Core, Error, HostSessionState, HostSystemActions, OperationState, ServiceKind, SessionPhase,
    SystemAction, SystemPolicy, SystemSupport,
};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

fn wait(mut condition: impl FnMut() -> bool) {
    let until = Instant::now() + Duration::from_secs(8);
    while !condition() {
        assert!(Instant::now() < until, "system condition timed out");
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[derive(Default)]
struct FakeCalls {
    locks: u32,
    restarts: u32,
    shutdowns: u32,
    display_private: bool,
    input_blocked: bool,
    display_calls: u32,
    input_calls: u32,
    restore_attempts: u32,
}

struct FakeActions {
    calls: Mutex<FakeCalls>,
    support: SystemSupport,
    support_calls: Mutex<u32>,
    fail_lock: Mutex<bool>,
    fail_restore: Mutex<bool>,
}

impl FakeActions {
    fn full() -> Self {
        Self {
            calls: Mutex::new(FakeCalls::default()),
            support: SystemSupport {
                lock: true,
                restart: true,
                shutdown: true,
                privacy: true,
                input_block: true,
            },
            support_calls: Mutex::new(0),
            fail_lock: Mutex::new(false),
            fail_restore: Mutex::new(false),
        }
    }
}

impl HostSystemActions for FakeActions {
    fn support(&self) -> SystemSupport {
        *self.support_calls.lock().unwrap() += 1;
        self.support
    }
    fn lock_workstation(&self) -> Result<(), Error> {
        if *self.fail_lock.lock().unwrap() {
            return Err(Error::Unsupported("fake lock refused by the OS"));
        }
        self.calls.lock().unwrap().locks += 1;
        Ok(())
    }
    fn restart(&self) -> Result<(), Error> {
        self.calls.lock().unwrap().restarts += 1;
        Ok(())
    }
    fn shutdown(&self) -> Result<(), Error> {
        self.calls.lock().unwrap().shutdowns += 1;
        Ok(())
    }
    fn set_display_private(&self, private: bool) -> Result<(), Error> {
        let mut calls = self.calls.lock().unwrap();
        if !private {
            calls.restore_attempts += 1;
            if *self.fail_restore.lock().unwrap() {
                return Err(Error::Unsupported("fake display restore refused by the OS"));
            }
        }
        calls.display_private = private;
        calls.display_calls += 1;
        Ok(())
    }
    fn set_input_blocked(&self, blocked: bool) -> Result<(), Error> {
        let mut calls = self.calls.lock().unwrap();
        if !blocked {
            calls.restore_attempts += 1;
            if *self.fail_restore.lock().unwrap() {
                return Err(Error::Unsupported("fake input restore refused by the OS"));
            }
        }
        calls.input_blocked = blocked;
        calls.input_calls += 1;
        Ok(())
    }
    fn observed_privacy(&self) -> Option<(bool, bool)> {
        let calls = self.calls.lock().unwrap();
        Some((calls.display_private, calls.input_blocked))
    }
    fn session_state(&self) -> HostSessionState {
        HostSessionState {
            locked: Some(false),
            login_screen: Some(false),
            headless: false,
            session: "fake".into(),
        }
    }
}

fn lockable(calls: &Mutex<FakeCalls>) -> u32 {
    calls.lock().unwrap().locks
}

fn active_parent(controller: &Core, host: &Core) -> crate::SessionInfo {
    let pending = controller
        .begin_connect_services(&host.endpoint_id())
        .unwrap();
    wait(|| controller.controller_session(pending.id).unwrap().phase == SessionPhase::Active);
    controller.controller_session(pending.id).unwrap()
}

fn open_system(controller: &Core, parent: &crate::SessionInfo) -> crate::RemoteSystemService {
    let owner = controller
        .attach_controller_service(parent.id, parent.generation, ServiceKind::System)
        .unwrap();
    controller
        .open_system_service(owner.id, owner.generation)
        .unwrap()
}

fn paired_system() -> (Core, Core, Arc<FakeActions>) {
    let host = start_direct();
    let controller = start_direct();
    let actions = Arc::new(FakeActions::full());
    host.set_system_actions(Some(actions.clone())).unwrap();
    pair_for_control(&host, &controller);
    host.set_service_sharing(ServiceKind::System, true).unwrap();
    (host, controller, actions)
}

#[test]
fn grant_policy_and_backend_gate_every_execution() {
    let host = start_direct();
    let controller = start_direct();
    pair_for_control(&host, &controller);
    let parent = active_parent(&controller, &host);
    let owner = controller
        .attach_controller_service(parent.id, parent.generation, ServiceKind::System)
        .unwrap();
    // No grant, no adapter: the open itself is refused.
    assert!(matches!(
        controller.open_system_service(owner.id, owner.generation),
        Err(Error::Unauthorized)
    ));
    host.set_service_sharing(ServiceKind::System, true).unwrap();
    // Grant without a backend: open succeeds for support discovery, but the
    // execution is refused and retained as an auditable operation.
    let system = controller
        .open_system_service(owner.id, owner.generation)
        .unwrap();
    let (support, session) = system.support().unwrap();
    assert_eq!(support, SystemSupport::default());
    assert_eq!(session.locked, None);
    let refused = system.execute(SystemAction::Lock).unwrap();
    assert_eq!(refused.state, OperationState::Refused);
    assert_eq!(system.operation(refused.id).unwrap(), refused);
    assert!(matches!(
        system.operation(refused.id + 100),
        Err(Error::NoSession)
    ));
}

#[test]
fn lock_executes_and_restart_needs_explicit_policy() {
    let (host, controller, actions) = paired_system();
    let parent = active_parent(&controller, &host);
    let system = open_system(&controller, &parent);
    let locked = system.execute(SystemAction::Lock).unwrap();
    assert_eq!(locked.state, OperationState::Executed);
    assert!(locked.completion_observable);
    assert_eq!(lockable(&actions.calls), 1);
    assert_eq!(system.operation(locked.id).unwrap(), locked);
    // Restart and shutdown stay refused until the local owner allows them.
    let restart = system.execute(SystemAction::Restart).unwrap();
    assert_eq!(restart.state, OperationState::Refused);
    let shutdown = system.execute(SystemAction::Shutdown).unwrap();
    assert_eq!(shutdown.state, OperationState::Refused);
    assert_eq!(actions.calls.lock().unwrap().restarts, 0);
    host.set_system_policy(SystemPolicy {
        allow_restart: true,
        allow_shutdown: true,
        ..SystemPolicy::default()
    })
    .unwrap();
    let restart = system.execute(SystemAction::Restart).unwrap();
    assert_eq!(restart.state, OperationState::Executed);
    assert!(!restart.completion_observable);
    assert_eq!(actions.calls.lock().unwrap().restarts, 1);
    let shutdown = system.execute(SystemAction::Shutdown).unwrap();
    assert_eq!(shutdown.state, OperationState::Executed);
    assert!(!shutdown.completion_observable);
}

#[test]
fn backend_failure_is_a_failed_operation_not_a_lost_request() {
    let (host, controller, actions) = paired_system();
    *actions.fail_lock.lock().unwrap() = true;
    let parent = active_parent(&controller, &host);
    let system = open_system(&controller, &parent);
    let failed = system.execute(SystemAction::Lock).unwrap();
    assert_eq!(failed.state, OperationState::Failed);
    assert!(failed.detail.contains("refused"));
    assert_eq!(lockable(&actions.calls), 0);
    assert_eq!(system.operation(failed.id).unwrap(), failed);
}

#[test]
fn operations_never_replay_across_reconnect() {
    let (host, controller, _) = paired_system();
    let parent = active_parent(&controller, &host);
    let system = open_system(&controller, &parent);
    let first = system.execute(SystemAction::Lock).unwrap();
    assert_eq!(first.state, OperationState::Executed);
    controller.disconnect_session(parent.id).unwrap();
    wait(|| {
        controller
            .controller_session(parent.id)
            .is_ok_and(|s| s.phase == SessionPhase::Closed)
    });
    drop(system);
    let parent = active_parent(&controller, &host);
    assert!(parent.generation > 0);
    let system = open_system(&controller, &parent);
    // The previous generation's operation is unreachable, not resumed.
    assert!(matches!(system.operation(first.id), Err(Error::NoSession)));
    let second = system.execute(SystemAction::Lock).unwrap();
    assert_eq!(second.state, OperationState::Executed);
    assert_ne!(second.id, first.id);
}

#[test]
fn privacy_lease_is_exclusive_and_close_restores() {
    let (host, controller, actions) = paired_system();
    let second = start_direct();
    pair_for_control(&host, &second);
    host.set_service_sharing(ServiceKind::System, true).unwrap();
    host.set_system_policy(SystemPolicy {
        allow_privacy: true,
        ..SystemPolicy::default()
    })
    .unwrap();
    let parent = active_parent(&controller, &host);
    let system = open_system(&controller, &parent);
    let other_parent = active_parent(&second, &host);
    let other = open_system(&second, &other_parent);
    let held = system.set_privacy(true, true).unwrap();
    assert!(held.display_private && held.input_blocked);
    assert_eq!(held.holder_connection, Some(parent.id));
    assert!(matches!(other.set_privacy(true, false), Err(Error::Busy)));
    assert_eq!(host.system_privacy().unwrap(), held);
    system.close();
    wait(|| {
        !actions.calls.lock().unwrap().display_private
            && !actions.calls.lock().unwrap().input_blocked
    });
    assert_eq!(
        host.system_privacy().unwrap(),
        crate::PrivacyState::default()
    );
    // The other peer can take the released lease.
    let retaken = other.set_privacy(true, false).unwrap();
    assert!(retaken.display_private && !retaken.input_blocked);
    drop(other);
    wait(|| !actions.calls.lock().unwrap().display_private);
}

#[test]
fn revoke_restores_privacy_and_fences_late_requests() {
    let (host, controller, actions) = paired_system();
    host.set_system_policy(SystemPolicy {
        allow_privacy: true,
        ..SystemPolicy::default()
    })
    .unwrap();
    let parent = active_parent(&controller, &host);
    let system = open_system(&controller, &parent);
    system.set_privacy(false, true).unwrap();
    assert!(actions.calls.lock().unwrap().input_blocked);
    host.set_service_sharing(ServiceKind::System, false)
        .unwrap();
    wait(|| !actions.calls.lock().unwrap().input_blocked);
    assert!(system.set_privacy(false, false).is_err());
    assert!(system.execute(SystemAction::Lock).is_err());
}

#[test]
fn privacy_without_policy_is_unauthorized_and_local_state_is_pollable() {
    let (host, controller, _) = paired_system();
    let parent = active_parent(&controller, &host);
    let system = open_system(&controller, &parent);
    assert!(matches!(
        system.set_privacy(true, false),
        Err(Error::Unauthorized)
    ));
    let (support, session) = host.system_support().unwrap();
    assert!(support.lock && support.privacy && support.input_block);
    assert_eq!(session.session, "fake");
    assert_eq!(
        host.system_privacy().unwrap(),
        crate::PrivacyState::default()
    );
}

#[test]
fn system_policy_and_grant_survive_a_record_round_trip() {
    let policy = SystemPolicy {
        allow_lock: false,
        allow_restart: true,
        allow_shutdown: false,
        allow_privacy: true,
    };
    let trust = crate::trust::Trust::default();
    let bytes = crate::installation::encode(
        &[7u8; 32],
        &crate::RelaySettings::Public,
        &trust,
        &crate::invitation::ReplayCache::new(),
        &crate::terminal::TerminalPolicy::default(),
        &crate::job::FileRetentionPolicy::default(),
        &crate::power::PowerPolicy::default(),
        &crate::job::FileResourcePolicy::default(),
        &policy,
    )
    .unwrap();
    let record = crate::installation::decode(&bytes).unwrap();
    assert_eq!(record.system_policy, policy);
    assert_eq!(
        record.terminal_policy,
        crate::terminal::TerminalPolicy::default()
    );
}

#[test]
fn operation_detail_truncates_on_char_boundaries() {
    let long = "é".repeat(300);
    let truncated = truncate_detail(long);
    assert!(truncated.len() <= 256);
    assert!(truncated.chars().all(|c| c == 'é'));
}

#[test]
fn review_privacy_policy_narrowing_restores_local_state() {
    let (host, controller, actions) = paired_system();
    host.set_system_policy(SystemPolicy {
        allow_privacy: true,
        ..SystemPolicy::default()
    })
    .unwrap();
    let parent = active_parent(&controller, &host);
    let system = open_system(&controller, &parent);
    system.set_privacy(true, true).unwrap();
    host.set_system_policy(SystemPolicy::default()).unwrap();
    assert!(
        !host.system_privacy().unwrap().input_blocked,
        "policy disable retains input block"
    );
    assert!(!actions.calls.lock().unwrap().display_private);
}

#[test]
fn review_unsupported_system_action_is_unavailable() {
    let host = start_direct();
    let mut actions = FakeActions::full();
    actions.support.restart = false;
    host.set_system_actions(Some(Arc::new(actions))).unwrap();
    host.set_system_policy(SystemPolicy {
        allow_restart: true,
        ..SystemPolicy::default()
    })
    .unwrap();
    assert!(
        !host
            .capability_availability(None, crate::Capability::SystemRestart)
            .unwrap()
            .available,
        "unsupported restart advertised available"
    );
}

#[test]
fn privacy_narrowing_restores_exactly_once_and_failed_save_preserves_lease() {
    use crate::installation::MemoryStore;
    use crate::relay::RelaySettings;
    let store = MemoryStore::empty();
    let host = Core::open_direct(Arc::new(store.clone()), RelaySettings::public_n0())
        .expect("installation opens");
    host.set_video_codecs(crate::VideoCodecCaps::h264_full(4096, 4096, "test"))
        .unwrap();
    let actions = Arc::new(FakeActions::full());
    host.set_system_actions(Some(actions.clone())).unwrap();
    let holder = start_direct();
    pair_for_control(&host, &holder);
    host.set_service_sharing(ServiceKind::System, true).unwrap();
    let sibling = start_direct();
    pair_for_control(&host, &sibling);
    host.set_service_sharing(ServiceKind::System, true).unwrap();
    host.set_system_policy(SystemPolicy {
        allow_privacy: true,
        ..SystemPolicy::default()
    })
    .unwrap();
    let parent = active_parent(&holder, &host);
    let system = open_system(&holder, &parent);
    let sibling_parent = active_parent(&sibling, &host);
    let sibling_system = open_system(&sibling, &sibling_parent);
    system.set_privacy(true, true).unwrap();
    assert_eq!(actions.calls.lock().unwrap().display_calls, 1);
    assert_eq!(actions.calls.lock().unwrap().input_calls, 1);
    // A failed save keeps the previous policy and the live lease untouched.
    store.fail_next_save(Error::StoreLocked);
    assert!(matches!(
        host.set_system_policy(SystemPolicy::default()),
        Err(Error::StoreLocked)
    ));
    assert!(host.system_policy().unwrap().allow_privacy);
    let live = host.system_privacy().unwrap();
    assert!(live.display_private && live.input_blocked);
    assert_eq!(actions.calls.lock().unwrap().display_calls, 1);
    assert_eq!(actions.calls.lock().unwrap().input_calls, 1);
    // A saved narrowing restores each facility exactly once.
    host.set_system_policy(SystemPolicy::default()).unwrap();
    assert_eq!(
        host.system_privacy().unwrap(),
        crate::PrivacyState::default()
    );
    let calls = actions.calls.lock().unwrap();
    assert_eq!(calls.display_calls, 2);
    assert_eq!(calls.input_calls, 2);
    assert!(!calls.display_private && !calls.input_blocked);
    drop(calls);
    // Both owners and both sessions survive; re-enable stays refused.
    assert_eq!(system.privacy().unwrap(), crate::PrivacyState::default());
    assert_eq!(
        sibling_system.privacy().unwrap(),
        crate::PrivacyState::default()
    );
    assert!(matches!(
        system.set_privacy(true, false),
        Err(Error::Unauthorized)
    ));
    assert_eq!(
        holder.controller_session(parent.id).unwrap().phase,
        SessionPhase::Active
    );
    assert_eq!(
        sibling.controller_session(sibling_parent.id).unwrap().phase,
        SessionPhase::Active
    );
}

/// Mirrors the Windows backend shape: no display privacy (the setter
/// rejects both booleans), working input blocking.
struct WindowsLikeActions {
    display_calls: Mutex<u32>,
    input_calls: Mutex<Vec<bool>>,
}

impl HostSystemActions for WindowsLikeActions {
    fn support(&self) -> SystemSupport {
        SystemSupport {
            lock: true,
            restart: true,
            shutdown: true,
            privacy: false,
            input_block: true,
        }
    }
    fn lock_workstation(&self) -> Result<(), Error> {
        Ok(())
    }
    fn restart(&self) -> Result<(), Error> {
        Ok(())
    }
    fn shutdown(&self) -> Result<(), Error> {
        Ok(())
    }
    fn set_display_private(&self, _: bool) -> Result<(), Error> {
        *self.display_calls.lock().unwrap() += 1;
        Err(Error::Unsupported("privacy mode is not available"))
    }
    fn set_input_blocked(&self, blocked: bool) -> Result<(), Error> {
        self.input_calls.lock().unwrap().push(blocked);
        Ok(())
    }
    fn session_state(&self) -> HostSessionState {
        HostSessionState {
            locked: None,
            login_screen: None,
            headless: false,
            session: "win32".into(),
        }
    }
}

#[test]
fn input_only_enable_skips_unsupported_display_facility() {
    let host = start_direct();
    let controller = start_direct();
    let actions = Arc::new(WindowsLikeActions {
        display_calls: Mutex::new(0),
        input_calls: Mutex::new(Vec::new()),
    });
    host.set_system_actions(Some(actions.clone())).unwrap();
    pair_for_control(&host, &controller);
    host.set_service_sharing(ServiceKind::System, true).unwrap();
    host.set_system_policy(SystemPolicy {
        allow_privacy: true,
        ..SystemPolicy::default()
    })
    .unwrap();
    let parent = active_parent(&controller, &host);
    let system = open_system(&controller, &parent);
    // Asking for display privacy on an unsupported backend stays refused.
    assert!(system.set_privacy(true, false).is_err());
    // Input-only enable succeeds without touching the display setter.
    let state = system.set_privacy(false, true).unwrap();
    assert!(!state.display_private && state.input_blocked);
    assert_eq!(*actions.display_calls.lock().unwrap(), 0);
    assert_eq!(*actions.input_calls.lock().unwrap(), vec![true]);
    // Release restores input exactly once and still skips display.
    let state = system.set_privacy(false, false).unwrap();
    assert_eq!(state, crate::PrivacyState::default());
    assert_eq!(*actions.display_calls.lock().unwrap(), 0);
    assert_eq!(*actions.input_calls.lock().unwrap(), vec![true, false]);
}

#[test]
fn system_availability_follows_native_bits_not_adapter_presence() {
    use crate::{AvailabilityReason, Capability};
    // Mac/Windows-shaped mix: lock and shutdown exist, restart does not,
    // and only input blocking backs privacy.
    let host = start_direct();
    let controller = start_direct();
    pair_for_control(&host, &controller);
    let mut actions = FakeActions::full();
    actions.support.restart = false;
    actions.support.privacy = false;
    let actions = Arc::new(actions);
    host.set_system_actions(Some(actions.clone())).unwrap();
    host.set_system_policy(SystemPolicy {
        allow_lock: true,
        allow_restart: true,
        allow_shutdown: true,
        allow_privacy: true,
    })
    .unwrap();
    let query = |capability| host.capability_availability(None, capability).unwrap();
    assert!(query(Capability::SystemLock).available);
    assert!(query(Capability::SystemShutdown).available);
    // Input blocking alone backs privacy as implemented.
    assert!(query(Capability::SystemPrivacy).available);
    let restart = query(Capability::SystemRestart);
    assert!(!restart.available);
    assert_eq!(
        restart.reasons.first(),
        Some(&AvailabilityReason::MissingImplementation)
    );
    // The query consults native support; presence alone never decides.
    assert!(*actions.support_calls.lock().unwrap() >= 4);
    // A supported action reports its real gate: policy first...
    host.set_system_policy(SystemPolicy {
        allow_lock: false,
        allow_restart: true,
        allow_shutdown: true,
        allow_privacy: true,
    })
    .unwrap();
    let lock = query(Capability::SystemLock);
    assert!(!lock.available);
    assert_eq!(
        lock.reasons.first(),
        Some(&AvailabilityReason::DisabledLocalPolicy)
    );
    // ...then the peer grant, never a phantom missing implementation.
    let peer = controller.endpoint_id();
    let lock = host
        .capability_availability(Some(&peer), Capability::SystemLock)
        .unwrap();
    assert!(!lock.available);
    assert!(lock.reasons.contains(&AvailabilityReason::DeniedPeerGrant));
    assert!(!lock
        .reasons
        .contains(&AvailabilityReason::MissingImplementation));
    // Nothing backing privacy reads missing.
    let mut bare = FakeActions::full();
    bare.support.privacy = false;
    bare.support.input_block = false;
    host.set_system_actions(Some(Arc::new(bare))).unwrap();
    let privacy = query(Capability::SystemPrivacy);
    assert!(!privacy.available);
    assert_eq!(
        privacy.reasons.first(),
        Some(&AvailabilityReason::MissingImplementation)
    );
}

/// STD07: removing the backend restores the ORIGINAL adapter exactly once,
/// even though the slot is empty when the owning serve loop tears down.
#[test]
fn backend_removal_restores_original_privacy_exactly_once() {
    let (host, controller, actions) = paired_system();
    host.set_system_policy(SystemPolicy {
        allow_privacy: true,
        ..SystemPolicy::default()
    })
    .unwrap();
    let parent = active_parent(&controller, &host);
    let system = open_system(&controller, &parent);
    system.set_privacy(true, true).unwrap();
    host.set_system_actions(None).unwrap();
    wait(|| host.system_privacy().unwrap() == crate::PrivacyState::default());
    let calls = actions.calls.lock().unwrap();
    assert!(
        !calls.display_private && !calls.input_blocked,
        "original backend still restricted: display={}, input={}",
        calls.display_private,
        calls.input_blocked
    );
    // One enable plus exactly one restore per facility.
    assert_eq!(calls.display_calls, 2, "display restore count");
    assert_eq!(calls.input_calls, 2, "input restore count");
}

/// STD07: replacing backend A with B while a lease is active restores A
/// exactly once and never touches B.
#[test]
fn backend_replacement_restores_original_and_leaves_new_untouched() {
    let (host, controller, actions) = paired_system();
    host.set_system_policy(SystemPolicy {
        allow_privacy: true,
        ..SystemPolicy::default()
    })
    .unwrap();
    let parent = active_parent(&controller, &host);
    let system = open_system(&controller, &parent);
    system.set_privacy(true, true).unwrap();
    let replacement = Arc::new(FakeActions::full());
    host.set_system_actions(Some(replacement.clone())).unwrap();
    wait(|| host.system_privacy().unwrap() == crate::PrivacyState::default());
    let calls = actions.calls.lock().unwrap();
    assert!(
        !calls.display_private && !calls.input_blocked,
        "original backend still restricted: display={}, input={}",
        calls.display_private,
        calls.input_blocked
    );
    assert_eq!(calls.display_calls, 2, "original display restore count");
    assert_eq!(calls.input_calls, 2, "original input restore count");
    let fresh = replacement.calls.lock().unwrap();
    assert_eq!(fresh.display_calls, 0, "replacement display touched");
    assert_eq!(fresh.input_calls, 0, "replacement input touched");
}

/// STD07: a native restore failure keeps the lease (ownership is not
/// forgotten) and Core keeps reporting the restriction instead of
/// claiming restoration; clearing the fault and narrowing policy retries.
#[test]
fn failed_native_restore_keeps_lease_and_reports_held() {
    let (host, controller, actions) = paired_system();
    *actions.fail_restore.lock().unwrap() = true;
    host.set_system_policy(SystemPolicy {
        allow_privacy: true,
        ..SystemPolicy::default()
    })
    .unwrap();
    let parent = active_parent(&controller, &host);
    let system = open_system(&controller, &parent);
    system.set_privacy(true, true).unwrap();
    host.set_system_actions(None).unwrap();
    // The teardown restore fails: wait for the attempt, then Core must NOT
    // report the default state.
    wait(|| actions.calls.lock().unwrap().restore_attempts >= 1);
    wait(|| host.hosted_services().unwrap().is_empty());
    let held = host.system_privacy().unwrap();
    assert!(
        held.display_private && held.input_blocked,
        "Core claims restoration after native failure: {held:?}"
    );
    assert!(held.holder_connection.is_some());
    assert!(actions.calls.lock().unwrap().display_private);
    assert!(actions.calls.lock().unwrap().input_blocked);
    // Clearing the fault and narrowing policy retries the retained lease.
    *actions.fail_restore.lock().unwrap() = false;
    host.set_system_policy(SystemPolicy {
        allow_privacy: false,
        ..SystemPolicy::default()
    })
    .unwrap();
    wait(|| host.system_privacy().unwrap() == crate::PrivacyState::default());
    let calls = actions.calls.lock().unwrap();
    assert!(!calls.display_private && !calls.input_blocked);
    assert_eq!(calls.display_calls, 2);
    assert_eq!(calls.input_calls, 2);
}

/// Input blocking that refuses every call, like an unelevated Windows
/// `BlockInput`. Display privacy succeeds so the rollback reaches input.
struct RefusingInput {
    display_private: Mutex<bool>,
    refuse: Mutex<bool>,
}

impl HostSystemActions for RefusingInput {
    fn support(&self) -> SystemSupport {
        SystemSupport {
            lock: true,
            restart: true,
            shutdown: true,
            privacy: true,
            input_block: true,
        }
    }
    fn lock_workstation(&self) -> Result<(), Error> {
        Ok(())
    }
    fn restart(&self) -> Result<(), Error> {
        Ok(())
    }
    fn shutdown(&self) -> Result<(), Error> {
        Ok(())
    }
    fn set_display_private(&self, private: bool) -> Result<(), Error> {
        *self.display_private.lock().unwrap() = private;
        Ok(())
    }
    fn set_input_blocked(&self, _: bool) -> Result<(), Error> {
        if *self.refuse.lock().unwrap() {
            return Err(Error::Unsupported("input blocking was refused"));
        }
        Ok(())
    }
    fn session_state(&self) -> HostSessionState {
        HostSessionState::unknown("fake")
    }
}

#[test]
fn refused_enable_applies_nothing_and_reports_default() {
    let host = start_direct();
    let controller = start_direct();
    let actions = Arc::new(RefusingInput {
        display_private: Mutex::new(false),
        refuse: Mutex::new(true),
    });
    host.set_system_actions(Some(actions.clone())).unwrap();
    pair_for_control(&host, &controller);
    host.set_service_sharing(ServiceKind::System, true).unwrap();
    host.set_system_policy(SystemPolicy {
        allow_privacy: true,
        ..SystemPolicy::default()
    })
    .unwrap();
    let parent = active_parent(&controller, &host);
    let system = open_system(&controller, &parent);
    // Input refuses the apply, so nothing is ever held: display applies
    // and reverts, and Core reports the default state instead of a lease
    // it could not establish.
    assert!(system.set_privacy(true, true).is_err());
    assert_eq!(
        host.system_privacy().unwrap(),
        crate::PrivacyState::default()
    );
    assert!(!*actions.display_private.lock().unwrap());
    // The next enable starts clean through the same backend.
    *actions.refuse.lock().unwrap() = false;
    let state = system.set_privacy(true, true).unwrap();
    assert!(state.display_private && state.input_blocked);
}

/// Display that applies but sticks on restore until cleared; input
/// refuses the apply and restores cleanly.
struct StickyDisplay {
    display_private: Mutex<bool>,
    stuck: Mutex<bool>,
}

impl HostSystemActions for StickyDisplay {
    fn support(&self) -> SystemSupport {
        SystemSupport {
            lock: true,
            restart: true,
            shutdown: true,
            privacy: true,
            input_block: true,
        }
    }
    fn lock_workstation(&self) -> Result<(), Error> {
        Ok(())
    }
    fn restart(&self) -> Result<(), Error> {
        Ok(())
    }
    fn shutdown(&self) -> Result<(), Error> {
        Ok(())
    }
    fn set_display_private(&self, private: bool) -> Result<(), Error> {
        if !private && *self.stuck.lock().unwrap() {
            return Err(Error::Unsupported("display restore stuck"));
        }
        *self.display_private.lock().unwrap() = private;
        Ok(())
    }
    fn set_input_blocked(&self, blocked: bool) -> Result<(), Error> {
        if blocked {
            return Err(Error::Unsupported("input blocking was refused"));
        }
        Ok(())
    }
    fn session_state(&self) -> HostSessionState {
        HostSessionState::unknown("fake")
    }
}

#[test]
fn stuck_fresh_apply_retains_acknowledged_remainder() {
    let host = start_direct();
    let controller = start_direct();
    let actions = Arc::new(StickyDisplay {
        display_private: Mutex::new(false),
        stuck: Mutex::new(true),
    });
    host.set_system_actions(Some(actions.clone())).unwrap();
    pair_for_control(&host, &controller);
    host.set_service_sharing(ServiceKind::System, true).unwrap();
    host.set_system_policy(SystemPolicy {
        allow_privacy: true,
        ..SystemPolicy::default()
    })
    .unwrap();
    let parent = active_parent(&controller, &host);
    let system = open_system(&controller, &parent);
    // Display applies, input refuses, and the display revert sticks: the
    // lease keeps exactly the acknowledged remainder (display only),
    // never the requested flags.
    assert!(system.set_privacy(true, true).is_err());
    let held = host.system_privacy().unwrap();
    assert!(
        held.display_private && !held.input_blocked,
        "remainder must be display-only: {held:?}"
    );
    assert!(held.holder_connection.is_some());
    assert!(*actions.display_private.lock().unwrap());
    // Clearing the fault lets an explicit disable converge to default.
    *actions.stuck.lock().unwrap() = false;
    let state = system.set_privacy(false, false).unwrap();
    assert_eq!(state, crate::PrivacyState::default());
    assert!(!*actions.display_private.lock().unwrap());
}

#[test]
fn input_only_owner_close_restores_without_touching_display() {
    let host = start_direct();
    let controller = start_direct();
    let actions = Arc::new(WindowsLikeActions {
        display_calls: Mutex::new(0),
        input_calls: Mutex::new(Vec::new()),
    });
    host.set_system_actions(Some(actions.clone())).unwrap();
    pair_for_control(&host, &controller);
    host.set_service_sharing(ServiceKind::System, true).unwrap();
    host.set_system_policy(SystemPolicy {
        allow_privacy: true,
        ..SystemPolicy::default()
    })
    .unwrap();
    let parent = active_parent(&controller, &host);
    let system = open_system(&controller, &parent);
    system.set_privacy(false, true).unwrap();
    system.close();
    // Teardown restores the held input without ever calling the
    // unsupported display facility it never restricted.
    wait(|| actions.input_calls.lock().unwrap().len() > 1);
    assert_eq!(*actions.display_calls.lock().unwrap(), 0);
    assert_eq!(*actions.input_calls.lock().unwrap(), vec![true, false]);
    assert_eq!(
        host.system_privacy().unwrap(),
        crate::PrivacyState::default()
    );
}

/// Display restore sticks until cleared while input always succeeds,
/// with per-call counts for the retry assertions.
struct FlakyDisplay {
    display_private: Mutex<bool>,
    input_blocked: Mutex<bool>,
    display_calls: Mutex<u32>,
    input_calls: Mutex<u32>,
    stuck: Mutex<bool>,
}

impl HostSystemActions for FlakyDisplay {
    fn support(&self) -> SystemSupport {
        SystemSupport {
            lock: true,
            restart: true,
            shutdown: true,
            privacy: true,
            input_block: true,
        }
    }
    fn lock_workstation(&self) -> Result<(), Error> {
        Ok(())
    }
    fn restart(&self) -> Result<(), Error> {
        Ok(())
    }
    fn shutdown(&self) -> Result<(), Error> {
        Ok(())
    }
    fn set_display_private(&self, private: bool) -> Result<(), Error> {
        *self.display_calls.lock().unwrap() += 1;
        if !private && *self.stuck.lock().unwrap() {
            return Err(Error::Unsupported("display restore stuck"));
        }
        *self.display_private.lock().unwrap() = private;
        Ok(())
    }
    fn set_input_blocked(&self, blocked: bool) -> Result<(), Error> {
        *self.input_calls.lock().unwrap() += 1;
        *self.input_blocked.lock().unwrap() = blocked;
        Ok(())
    }
    fn session_state(&self) -> HostSessionState {
        HostSessionState::unknown("fake")
    }
}

#[test]
fn partial_restore_keeps_remainder_and_repeated_narrowing_retries() {
    let host = start_direct();
    let controller = start_direct();
    let actions = Arc::new(FlakyDisplay {
        display_private: Mutex::new(false),
        input_blocked: Mutex::new(false),
        display_calls: Mutex::new(0),
        input_calls: Mutex::new(0),
        stuck: Mutex::new(true),
    });
    host.set_system_actions(Some(actions.clone())).unwrap();
    pair_for_control(&host, &controller);
    host.set_service_sharing(ServiceKind::System, true).unwrap();
    host.set_system_policy(SystemPolicy {
        allow_privacy: true,
        ..SystemPolicy::default()
    })
    .unwrap();
    let parent = active_parent(&controller, &host);
    let system = open_system(&controller, &parent);
    system.set_privacy(true, true).unwrap();
    // Narrowing restores input despite the stuck display and reports
    // the display-only remainder instead of failing the whole restore.
    let policy = SystemPolicy {
        allow_privacy: false,
        ..SystemPolicy::default()
    };
    assert!(host.set_system_policy(policy.clone()).is_err());
    let held = host.system_privacy().unwrap();
    assert!(
        held.display_private && !held.input_blocked,
        "partial remainder must be display-only: {held:?}"
    );
    assert!(!*actions.input_blocked.lock().unwrap());
    assert_eq!(*actions.input_calls.lock().unwrap(), 2);
    // Repeating the same narrowing retries the retained display restore
    // without re-driving the settled input.
    assert!(host.set_system_policy(policy.clone()).is_err());
    assert_eq!(*actions.display_calls.lock().unwrap(), 3);
    assert_eq!(*actions.input_calls.lock().unwrap(), 2);
    // Clearing the fault converges the next identical save to default.
    *actions.stuck.lock().unwrap() = false;
    host.set_system_policy(policy).unwrap();
    assert_eq!(
        host.system_privacy().unwrap(),
        crate::PrivacyState::default()
    );
    assert!(!*actions.display_private.lock().unwrap());
}

#[test]
fn local_privacy_release_is_observed_and_reenable_reapplies_native_restrictions() {
    let (host, controller, actions) = paired_system();
    host.set_system_policy(SystemPolicy {
        allow_privacy: true,
        ..Default::default()
    })
    .unwrap();
    let parent = active_parent(&controller, &host);
    let system = open_system(&controller, &parent);
    system.set_privacy(true, true).unwrap();
    {
        let mut calls = actions.calls.lock().unwrap();
        calls.display_private = false;
        calls.input_blocked = false;
    }
    let observed = host.system_privacy().unwrap();
    assert!(!observed.display_private && !observed.input_blocked);
    assert!(!system.privacy().unwrap().display_private);
    system.set_privacy(true, true).unwrap();
    assert!(actions.calls.lock().unwrap().display_private);
    assert!(host.system_privacy().unwrap().input_blocked);
    system.close();
}
