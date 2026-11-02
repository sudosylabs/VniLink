// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! What the system actions suites share: host operating systems that
//! record what they are asked to do instead of doing it, one of them shaped
//! as a scenario needs, a system service opened as the application opens
//! one, and the checks that run a controller's actions ([`check_actions`])
//! and give privacy back ([`check_giving_back`]).

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::Duration;

use expect_test::Expect;
use vnilink::{
    Error, HostSessionState, HostSystemActions, OperationState, PrivacyState, RemoteSystemService,
    ServiceKind, SessionInfo, SystemAction, SystemOperation, SystemPolicy, SystemSupport,
};

use crate::{assert_story, wait_for, Engine, Pair, Traced, DEFAULT_WAIT};

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

/// A host sharing its system actions, and a controller with the System
/// service open on its services connection.
pub struct SystemSession {
    pub pair: Pair,
    pub connection: SessionInfo,
    pub service: RemoteSystemService,
}

impl SystemSession {
    /// The host's operating system is `system`, and its policy `policy`
    /// when given, else the engine's own.
    pub fn open(
        system: Arc<dyn HostSystemActions>,
        policy: Option<SystemPolicy>,
    ) -> Result<Self, Error> {
        let pair = sharing(system)?;
        if let Some(policy) = policy {
            pair.host.core.set_system_policy(policy)?;
        }
        let connection = pair.controller.connect_services(&pair.host)?;
        let service = pair.controller.open_system(&connection)?;
        Ok(Self {
            pair,
            connection,
            service,
        })
    }
}

/// A host sharing its system actions, with `system` as its operating
/// system, and a controller paired with it.
pub fn sharing(system: Arc<dyn HostSystemActions>) -> Result<Pair, Error> {
    let pair = Pair::for_services(&[ServiceKind::System]);
    pair.host.core.set_system_actions(Some(system))?;
    Ok(pair)
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

/// One step of [`check_actions`].
#[derive(Clone, Copy, Debug)]
pub enum Step {
    /// The controller asks for the action, which ends in this state; one
    /// left `Scheduled` is still counting down its notice.
    Ask(SystemAction, OperationState),
    /// The controller cancels the action it asked for last.
    Cancel,
    /// The host's policy becomes this.
    Policy(SystemPolicy),
    /// The notice the host gives before a restart or shutdown becomes this.
    Notice(Duration),
}

/// What a controller asks of a host's System service, in order, and how
/// the host's operating system answers a lock.
pub struct Asking<'a> {
    /// The host's policy when the service opens, else the engine's own.
    pub policy: Option<SystemPolicy>,
    /// The host's OS refuses every lock.
    pub fail_lock: bool,
    pub steps: &'a [Step],
}

/// Opens a controller's System service on a host whose OS records each
/// action, takes `asking`'s steps in order and waits until each action
/// ends as its step says; asserts that an admitted lock runs at once and
/// observably while a restart or shutdown waits out its notice, that each
/// operation is kept as it ended, that the host's OS was asked exactly the
/// actions that ran, and that the host told the story `expect` of its
/// system actions. Returns each action's operation as it ended.
#[track_caller]
pub fn check_actions(
    trace: &Traced,
    asking: Asking<'_>,
    expect: Expect,
) -> Result<Vec<SystemOperation>, Box<dyn std::error::Error>> {
    let system = Arc::new(FakeSystem::full());
    system.calls().fail_lock = asking.fail_lock;
    let session = SystemSession::open(system.clone(), asking.policy)?;
    let (host, service) = (&session.pair.host.core, &session.service);
    let mut ended: Vec<SystemOperation> = Vec::new();
    for step in asking.steps {
        match *step {
            Step::Ask(action, ends) => {
                let answer = service.execute(action)?;
                if matches!(
                    answer.state,
                    OperationState::Scheduled | OperationState::Executed
                ) {
                    assert_eq!(
                        answer.state == OperationState::Scheduled,
                        action != SystemAction::Lock,
                        "a lock runs at once, a restart or shutdown waits out its notice: {answer:?}"
                    );
                    assert_eq!(
                        answer.completion_observable,
                        action == SystemAction::Lock,
                        "only a lock's completion is observable: {answer:?}"
                    );
                }
                let operation = if answer.state == ends {
                    answer
                } else {
                    wait_for(
                        &format!("the {action:?} to end {ends:?}"),
                        DEFAULT_WAIT,
                        || {
                            let operation = service.operation(answer.id).ok()?;
                            (operation.state == ends).then_some(operation)
                        },
                    )
                };
                ended.push(operation);
            }
            Step::Cancel => {
                let last = ended.last_mut().ok_or("an action to cancel")?;
                let cancelled = service.cancel(last.id)?;
                assert_eq!(
                    cancelled.state,
                    OperationState::Cancelled,
                    "an action cancelled during its notice is cancelled"
                );
                *last = cancelled;
            }
            Step::Policy(policy) => host.set_system_policy(policy)?,
            Step::Notice(notice) => host.set_system_action_notice(notice),
        }
    }
    for operation in &ended {
        assert_eq!(
            &service.operation(operation.id)?,
            operation,
            "an operation is kept as it ended"
        );
    }
    let ran = |action| {
        ended
            .iter()
            .filter(|operation| {
                operation.action == action && operation.state == OperationState::Executed
            })
            .count()
    };
    {
        let calls = system.calls();
        assert_eq!(
            (
                calls.locks as usize,
                calls.restarts as usize,
                calls.shutdowns as usize
            ),
            (
                ran(SystemAction::Lock),
                ran(SystemAction::Restart),
                ran(SystemAction::Shutdown)
            ),
            "the host's OS is asked exactly the actions that ran"
        );
    }
    let finished = ended
        .iter()
        .filter(|operation| {
            matches!(
                operation.state,
                OperationState::Executed | OperationState::Failed
            )
        })
        .count();
    trace.wait_for_records("system action ended", finished);
    assert_story(expect, || trace.story(&["system"]));
    Ok(ended)
}

/// How the host gives privacy back in [`check_giving_back`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GivingBack {
    /// The controller closes its System service.
    Close,
    /// The host stops sharing its system actions.
    Withdraw,
    /// The host's policy stops allowing privacy, which is given back before
    /// the change returns.
    Narrow,
    /// The host's OS backend is removed.
    RemoveBackend,
    /// The host's OS backend is replaced by another, which is never asked.
    ReplaceBackend,
}

/// Privacy a controller takes on a host, and how the host gives it back.
pub struct Giving {
    /// The host's operating system.
    pub system: FakeSystem,
    /// Whether the controller hides the host's screen.
    pub display: bool,
    /// Whether the controller blocks the host's input.
    pub input: bool,
    pub back: GivingBack,
}

/// Opens a controller's System service on a host whose policy allows
/// privacy, takes the privacy `giving` asks for and gives it back as
/// `giving.back` says; asserts that the host reports privacy given back,
/// that the OS that applied it restricted and restored each facility asked
/// exactly once and never touched the others, that a replacement backend
/// is never asked, and that the host told the story `expect` of its
/// privacy. Returns the session, for what a test asserts after.
#[track_caller]
pub fn check_giving_back(
    trace: &Traced,
    giving: Giving,
    expect: Expect,
) -> Result<SystemSession, Box<dyn std::error::Error>> {
    let system = Arc::new(giving.system);
    let session = SystemSession::open(
        system.clone(),
        Some(SystemPolicy {
            allow_privacy: true,
            ..SystemPolicy::default()
        }),
    )?;
    let host = &session.pair.host.core;
    let held = session.service.set_privacy(giving.display, giving.input)?;
    assert_eq!(
        (held.display_private, held.input_blocked),
        (giving.display, giving.input),
        "privacy holds what the controller asked"
    );
    let replacement = Arc::new(FakeSystem::full());
    match giving.back {
        GivingBack::Close => session.service.close(),
        GivingBack::Withdraw => host.set_service_sharing(ServiceKind::System, false)?,
        GivingBack::Narrow => host.set_system_policy(SystemPolicy::default())?,
        GivingBack::RemoveBackend => host.set_system_actions(None)?,
        GivingBack::ReplaceBackend => {
            host.set_system_actions(Some(Arc::clone(&replacement) as Arc<dyn HostSystemActions>))?
        }
    }
    let asked = usize::from(giving.display) + usize::from(giving.input);
    if giving.back == GivingBack::Narrow {
        assert_eq!(
            host.system_privacy()?,
            PrivacyState::default(),
            "a narrowed policy gives privacy back before the change returns"
        );
    } else {
        wait_for("privacy to be given back", DEFAULT_WAIT, || {
            let restores = {
                let calls = system.calls();
                calls.display.restores_asked() + calls.input.restores_asked()
            };
            (restores >= asked && host.system_privacy().ok()? == PrivacyState::default())
                .then_some(())
        });
    }
    {
        let calls = system.calls();
        for (name, facility, asked) in [
            ("display", &calls.display, giving.display),
            ("input", &calls.input, giving.input),
        ] {
            let expected = if asked {
                vec![(true, true), (false, true)]
            } else {
                Vec::new()
            };
            assert_eq!(
                facility.calls, expected,
                "the {name} is restricted and restored once by the backend that applied it, \
                 and never asked unless privacy wanted it"
            );
        }
    }
    {
        let fresh = replacement.calls();
        assert!(
            fresh.display.calls.is_empty() && fresh.input.calls.is_empty(),
            "a replacement backend is never asked to give back privacy it did not apply"
        );
    }
    trace.wait_for_records("privacy restored", 1);
    assert_story(expect, || trace.story(&["system"]));
    Ok(session)
}
