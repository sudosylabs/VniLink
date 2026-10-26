use super::support::start_direct;
use crate::system::truncate_detail;
use crate::{Error, HostSessionState, HostSystemActions, SystemPolicy, SystemSupport};
use std::sync::{Arc, Condvar, Mutex};

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
    /// While true, a lock never answers, as GNOME's does on a locked screen.
    stall_lock: (Mutex<bool>, Condvar),
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
            stall_lock: (Mutex::new(false), Condvar::new()),
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
        let (stalled, released) = &self.stall_lock;
        let _released = released
            .wait_while(stalled.lock().unwrap(), |stalled| *stalled)
            .unwrap();
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
