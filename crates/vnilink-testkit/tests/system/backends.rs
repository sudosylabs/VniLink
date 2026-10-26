//! Operating systems for the system suites that do what a scenario needs
//! of the host's: answer, fail, never answer a lock, or lack a facility,
//! and record every call they were asked.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};

use vnilink::{Error, HostSessionState, HostSystemActions, SystemSupport};

/// One privacy facility of the host, the display or input: whether it is
/// restricted, whether the OS refuses to restrict or to restore it, and
/// each call asked of it.
#[derive(Default)]
pub(super) struct Facility {
    /// Restricted, as the last call that worked left it.
    pub(super) restricted: bool,
    pub(super) refuse_apply: bool,
    pub(super) refuse_restore: bool,
    /// Each call in order: what it asked, and whether the OS did it.
    pub(super) calls: Vec<(bool, bool)>,
}

impl Facility {
    /// What each call asked, worked or not.
    pub(super) fn asked(&self) -> Vec<bool> {
        self.calls.iter().map(|(asked, _)| *asked).collect()
    }

    /// The calls the OS did.
    pub(super) fn done(&self) -> usize {
        self.calls.iter().filter(|(_, done)| *done).count()
    }

    /// The calls that asked to restore, worked or not.
    pub(super) fn restores_asked(&self) -> usize {
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
pub(super) struct Calls {
    pub(super) locks: u32,
    pub(super) restarts: u32,
    pub(super) shutdowns: u32,
    pub(super) fail_lock: bool,
    pub(super) display: Facility,
    pub(super) input: Facility,
}

/// An operating system that records each action instead of taking it.
/// [`FakeSystem::full`] supports everything and reports what it restricts,
/// as a desktop that can lose its restrictions locally does; the other
/// constructors give it the shapes the platforms' backends have.
pub(super) struct FakeSystem {
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
    pub(super) fn full() -> Self {
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
    pub(super) fn supporting(support: SystemSupport) -> Self {
        Self {
            support,
            ..Self::full()
        }
    }

    /// Windows's shape: no display privacy, which refuses every call, and
    /// working input blocking; its session state unknown.
    pub(super) fn windows_like() -> Self {
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
    pub(super) fn unobserved(faults: impl FnOnce(&mut Calls)) -> Self {
        let system = Self {
            session: HostSessionState::unknown("fake"),
            observes: false,
            ..Self::full()
        };
        faults(&mut system.calls());
        system
    }

    pub(super) fn calls(&self) -> MutexGuard<'_, Calls> {
        self.calls
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
    }

    /// How many times the engine asked what the OS supports.
    pub(super) fn support_asked(&self) -> u32 {
        self.support_asked.load(Ordering::SeqCst)
    }

    /// Locks stop answering until the returned guard drops, so a failed
    /// assertion fails the test rather than waits on the lock forever.
    pub(super) fn stall_locks(self: &Arc<Self>) -> Stalled {
        *self
            .stalled
            .0
            .lock()
            .unwrap_or_else(|poison| poison.into_inner()) = true;
        Stalled(Arc::clone(self))
    }
}

/// Lets stalled locks answer when dropped.
pub(super) struct Stalled(Arc<FakeSystem>);

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
