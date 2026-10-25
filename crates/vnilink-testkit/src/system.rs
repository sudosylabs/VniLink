//! What the system actions suites share: a host operating system that
//! records what it is asked to do instead of doing it, and a system
//! service opened as the application opens one.

use std::sync::{Arc, Mutex};

use vnilink::{
    Error, HostSessionState, HostSystemActions, RemoteSystemService, ServiceKind, SystemSupport,
};

use crate::Pair;

/// What the host's operating system was asked.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Asked {
    pub locks: u32,
    pub restarts: u32,
    pub shutdowns: u32,
    pub display_private: bool,
    pub input_blocked: bool,
}

/// An operating system that supports every action and records each one.
#[derive(Default)]
pub struct RecordingSystem {
    pub asked: Mutex<Asked>,
}

impl RecordingSystem {
    pub fn asked(&self) -> Asked {
        self.asked
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .clone()
    }

    fn record(&self, change: impl FnOnce(&mut Asked)) -> Result<(), Error> {
        change(
            &mut self
                .asked
                .lock()
                .unwrap_or_else(|poison| poison.into_inner()),
        );
        Ok(())
    }
}

impl HostSystemActions for RecordingSystem {
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
        self.record(|asked| asked.locks += 1)
    }
    fn restart(&self) -> Result<(), Error> {
        self.record(|asked| asked.restarts += 1)
    }
    fn shutdown(&self) -> Result<(), Error> {
        self.record(|asked| asked.shutdowns += 1)
    }
    fn set_display_private(&self, private: bool) -> Result<(), Error> {
        self.record(|asked| asked.display_private = private)
    }
    fn set_input_blocked(&self, blocked: bool) -> Result<(), Error> {
        self.record(|asked| asked.input_blocked = blocked)
    }
    fn observed_privacy(&self) -> Option<(bool, bool)> {
        let asked = self.asked();
        Some((asked.display_private, asked.input_blocked))
    }
    fn session_state(&self) -> HostSessionState {
        HostSessionState {
            locked: Some(false),
            login_screen: Some(false),
            headless: false,
            session: "testkit".into(),
        }
    }
}

/// A host sharing its system actions, with `system` as its operating
/// system when given, and a controller with the system service open.
pub struct SystemSession {
    pub pair: Pair,
    pub service: RemoteSystemService,
    pub system: Option<Arc<RecordingSystem>>,
}

impl SystemSession {
    pub fn open(system: Option<Arc<RecordingSystem>>) -> Self {
        let pair = Pair::for_services(&[ServiceKind::System]);
        if let Some(system) = &system {
            pair.host
                .core
                .set_system_actions(Some(Arc::clone(system) as Arc<dyn HostSystemActions>))
                .expect("system actions");
        }
        let connection = pair.connect_services();
        let owner = pair.attach(&connection, ServiceKind::System);
        let service = pair
            .controller
            .core
            .open_system_service(owner.id, owner.generation)
            .expect("the system service");
        Self {
            pair,
            service,
            system,
        }
    }
}
