//! What one host engine advertises of its system actions.

use std::sync::Arc;

use vnilink::{
    Capability, Error, HostSessionState, HostSystemActions, SystemPolicy, SystemSupport,
};
use vnilink_testkit::system::RecordingSystem;
use vnilink_testkit::{traced, Engine, TestResult};

/// An operating system that supports every action but restarting.
#[derive(Default)]
struct NoRestart(RecordingSystem);

impl HostSystemActions for NoRestart {
    fn support(&self) -> SystemSupport {
        SystemSupport {
            restart: false,
            ..self.0.support()
        }
    }
    fn lock_workstation(&self) -> Result<(), Error> {
        self.0.lock_workstation()
    }
    fn restart(&self) -> Result<(), Error> {
        self.0.restart()
    }
    fn shutdown(&self) -> Result<(), Error> {
        self.0.shutdown()
    }
    fn set_display_private(&self, private: bool) -> Result<(), Error> {
        self.0.set_display_private(private)
    }
    fn set_input_blocked(&self, blocked: bool) -> Result<(), Error> {
        self.0.set_input_blocked(blocked)
    }
    fn observed_privacy(&self) -> Option<(bool, bool)> {
        self.0.observed_privacy()
    }
    fn session_state(&self) -> HostSessionState {
        self.0.session_state()
    }
}

/// A restart the policy allows but the operating system cannot do is not
/// offered.
#[test]
fn a_restart_the_system_cannot_do_is_unavailable_though_allowed() -> TestResult {
    let _trace = traced();
    let host = Engine::start("host");
    host.core
        .set_system_actions(Some(Arc::new(NoRestart::default())))?;
    host.core.set_system_policy(SystemPolicy {
        allow_restart: true,
        ..SystemPolicy::default()
    })?;
    assert!(
        !host
            .core
            .capability_availability(None, Capability::SystemRestart)?
            .available,
        "an unsupported restart is not advertised available"
    );
    Ok(())
}
