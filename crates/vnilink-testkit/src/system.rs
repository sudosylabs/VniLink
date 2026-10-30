// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! What the system actions suites share: host operating systems that
//! record what they are asked to do instead of doing it, one of them shaped
//! as a scenario needs, and a system service opened as the application
//! opens one.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};

use vnilink::{
    Error, HostSessionState, HostSystemActions, RemoteSystemService, ServiceKind, SessionInfo,
    SystemSupport,
};

use crate::{Engine, Pair};

/// Opening services: the System service, beside [`Engine::open_desktop`].
impl Engine {
    /// A System service on `connection`, opened as the application opens
    /// one.
    pub fn open_system(&self, connection: &SessionInfo) -> Result<RemoteSystemService, Error> {
        let owner = self.core.attach_controller_service(
            connection.id,
            connection.generation,
            ServiceKind::System,
        )?;
        self.core.open_system_service(owner.id, owner.generation)
    }
}

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
        let service = pair
            .controller
            .open_system(&connection)
            .expect("the system service");
        Self {
            pair,
            service,
            system,
        }
    }
}

/// One privacy facility of the host, the display or input: whether it is
/// restricted, whether the OS refuses to restrict or to restore it, and
/// each call asked of it.
#[derive(Default)]
pub struct Facility {
    /// Restricted, as the last call that worked left it.
    pub restricted: bool,
    pub refuse_apply: bool,
    pub refuse_restore: bool,
    /// Each call in order: what it asked, and whether the OS did it.
    pub calls: Vec<(bool, bool)>,
}

impl Facility {
    /// What each call asked, worked or not.
    pub fn asked(&self) -> Vec<bool> {
        self.calls.iter().map(|(asked, _)| *asked).collect()
    }

    /// The calls the OS did.
    pub fn done(&self) -> usize {
        self.calls.iter().filter(|(_, done)| *done).count()
    }

    /// The calls that asked to restore, worked or not.
    pub fn restores_asked(&self) -> usize {
        self.calls.iter().filter(|(asked, _)| !asked).count()
    }

    fn set(&mut self, restrict: bool, refused: &'static str) -> Result<(), Error> {
        let refuse = if restrict {
            self.refuse_apply
        } else {
            self.refuse_restore
        };
        self.calls.push((restrict, !refuse));
        if refuse {
            return Err(Error::Unsupported(refused));
        }
        self.restricted = restrict;
        Ok(())
    }
}

/// What the host's OS was asked, and the faults it has now.
#[derive(Default)]
pub struct Calls {
    pub locks: u32,
    pub restarts: u32,
    pub shutdowns: u32,
    pub fail_lock: bool,
    pub display: Facility,
    pub input: Facility,
}

/// An operating system that records each action instead of taking it.
/// [`FakeSystem::full`] supports everything and reports what it restricts,
/// as a desktop that can lose its restrictions locally does; the other
/// constructors give it the shapes the platforms' backends have.
pub struct FakeSystem {
    support: SystemSupport,
    session: HostSessionState,
    observes: bool,
    support_asked: AtomicU32,
    calls: Mutex<Calls>,
    /// While true, a lock never answers, as GNOME's does on a locked screen.
    stalled: (Mutex<bool>, Condvar),
}

impl FakeSystem {
    /// Every action and both privacy facilities, its restrictions observed.
    pub fn full() -> Self {
        Self {
            support: SystemSupport {
                lock: true,
                restart: true,
                shutdown: true,
                privacy: true,
                input_block: true,
            },
            session: HostSessionState {
                locked: Some(false),
                login_screen: Some(false),
                headless: false,
                session: "fake".into(),
            },
            observes: true,
            support_asked: AtomicU32::new(0),
            calls: Mutex::new(Calls::default()),
            stalled: (Mutex::new(false), Condvar::new()),
        }
    }

    /// As [`Self::full`], supporting only what `support` says.
    pub fn supporting(support: SystemSupport) -> Self {
        Self {
            support,
            ..Self::full()
        }
    }

    /// Windows's shape: no display privacy, which refuses every call, and
    /// working input blocking; its session state unknown.
    pub fn windows_like() -> Self {
        let system = Self {
            support: SystemSupport {
                privacy: false,
                ..Self::full().support
            },
            session: HostSessionState::unknown("win32"),
            observes: false,
            ..Self::full()
        };
        {
            let mut calls = system.calls();
            calls.display.refuse_apply = true;
            calls.display.refuse_restore = true;
        }
        system
    }

    /// Every facility supported, nothing observed, and `faults` set on the
    /// facilities before it is used.
    pub fn unobserved(faults: impl FnOnce(&mut Calls)) -> Self {
        let system = Self {
            session: HostSessionState::unknown("fake"),
            observes: false,
            ..Self::full()
        };
        faults(&mut system.calls());
        system
    }

    pub fn calls(&self) -> MutexGuard<'_, Calls> {
        self.calls
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
    }

    /// How many times the engine asked what the OS supports.
    pub fn support_asked(&self) -> u32 {
        self.support_asked.load(Ordering::SeqCst)
    }

    /// Locks stop answering until the returned guard drops, so a failed
    /// assertion fails the test rather than waits on the lock forever.
    pub fn stall_locks(self: &Arc<Self>) -> Stalled {
        *self
            .stalled
            .0
            .lock()
            .unwrap_or_else(|poison| poison.into_inner()) = true;
        Stalled(Arc::clone(self))
    }
}

/// Lets stalled locks answer when dropped.
pub struct Stalled(Arc<FakeSystem>);

impl Drop for Stalled {
    fn drop(&mut self) {
        let (stalled, released) = &self.0.stalled;
        *stalled.lock().unwrap_or_else(|poison| poison.into_inner()) = false;
        released.notify_all();
    }
}

impl HostSystemActions for FakeSystem {
    fn support(&self) -> SystemSupport {
        self.support_asked.fetch_add(1, Ordering::SeqCst);
        self.support
    }
    fn lock_workstation(&self) -> Result<(), Error> {
        if self.calls().fail_lock {
            return Err(Error::Unsupported("the lock was refused by the OS"));
        }
        let (stalled, released) = &self.stalled;
        drop(
            released
                .wait_while(
                    stalled.lock().unwrap_or_else(|poison| poison.into_inner()),
                    |stalled| *stalled,
                )
                .unwrap_or_else(|poison| poison.into_inner()),
        );
        self.calls().locks += 1;
        Ok(())
    }
    fn restart(&self) -> Result<(), Error> {
        self.calls().restarts += 1;
        Ok(())
    }
    fn shutdown(&self) -> Result<(), Error> {
        self.calls().shutdowns += 1;
        Ok(())
    }
    fn set_display_private(&self, private: bool) -> Result<(), Error> {
        self.calls()
            .display
            .set(private, "display privacy was refused by the OS")
    }
    fn set_input_blocked(&self, blocked: bool) -> Result<(), Error> {
        self.calls()
            .input
            .set(blocked, "input blocking was refused by the OS")
    }
    fn observed_privacy(&self) -> Option<(bool, bool)> {
        let calls = self.calls();
        self.observes
            .then_some((calls.display.restricted, calls.input.restricted))
    }
    fn session_state(&self) -> HostSessionState {
        self.session.clone()
    }
}
