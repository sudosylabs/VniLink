//! Persistent owner thread for Windows local-input blocking.
//!
//! `BlockInput` binds to its calling thread: only that thread may unblock,
//! and only it may inject while blocked. Every blocking call, restoration
//! and blocked-time injection in this process runs on the single owner
//! thread spawned here, so a migratable worker or an application-thread
//! restore can never split the pair. Each state change is acknowledged;
//! a failed restoration retains ownership — the blocked flag stays set
//! and the same thread retries — instead of reporting healthy.
//!
//! Backends are constructed by the spawner and moved into the thread; the
//! injection backend opens its OS handle lazily on first use so blocking
//! never depends on injection being available. Non-Windows builds never
//! install an owner: [`is_blocked`] reads false and injection stays direct.

use crate::input::InputEvent;
use crate::Error;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{sync_channel, Receiver, Sender, SyncSender};
use std::sync::OnceLock;

/// Native input-blocking backend. Implemented once per platform that
/// blocks: the Windows backend calls `BlockInput` on the owner thread.
pub trait BlockBackend: Send + 'static {
    fn set_blocked(&mut self, blocked: bool) -> Result<(), Error>;
}

/// Injection operation the owner performs while input is blocked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InjectOp {
    Apply(InputEvent),
    TypeText(String),
    TypeChord(String),
}

/// Blocked-time injection backend. Runs on the owner thread, so injected
/// input satisfies the blocking-thread requirement.
pub trait InjectBackend: Send + 'static {
    fn inject(&mut self, op: &InjectOp) -> Result<(), Error>;
    /// Releases anything a blocked period left pressed. Runs after every
    /// acknowledged unblock so abandoned presses never stick.
    fn release_held(&mut self);
}

enum OwnerRequest {
    SetBlocked {
        blocked: bool,
        reply: SyncSender<Result<(), Error>>,
    },
    Inject {
        op: InjectOp,
        reply: SyncSender<Result<(), Error>>,
    },
}

/// One owner thread and its acknowledged blocked flag.
pub(crate) struct Owner {
    tx: Sender<OwnerRequest>,
    blocked: std::sync::Arc<AtomicBool>,
}

impl Owner {
    /// Spawns the owner thread. Backends move in and never leave; the
    /// thread runs until the process ends.
    pub(crate) fn spawn<B, I>(mut block: B, mut inject: I) -> Result<Self, Error>
    where
        B: BlockBackend,
        I: InjectBackend,
    {
        let (tx, rx) = std::sync::mpsc::channel();
        let blocked = std::sync::Arc::new(AtomicBool::new(false));
        let flag = blocked.clone();
        std::thread::Builder::new()
            .name("vnilink-input-owner".into())
            .spawn(move || run_owner(&mut block, &mut inject, rx, &flag))
            .map_err(|_| Error::Unsupported("the input owner did not start"))?;
        Ok(Self { tx, blocked })
    }

    pub(crate) fn is_blocked(&self) -> bool {
        self.blocked.load(Ordering::SeqCst)
    }

    /// Blocks or restores input on the owner thread and waits for its
    /// answer. A redundant call returns without a hop; a failed call
    /// leaves ownership exactly as it was for the same thread to retry.
    pub(crate) fn set_blocked(&self, blocked: bool) -> Result<(), Error> {
        if self.is_blocked() == blocked {
            return Ok(());
        }
        let (reply_tx, reply_rx) = sync_channel(1);
        self.tx
            .send(OwnerRequest::SetBlocked {
                blocked,
                reply: reply_tx,
            })
            .map_err(|_| Error::Unsupported("the input owner is gone"))?;
        reply_rx
            .recv()
            .map_err(|_| Error::Unsupported("the input owner is gone"))?
    }

    pub(crate) fn inject(&self, op: InjectOp) -> Result<(), Error> {
        let (reply_tx, reply_rx) = sync_channel(1);
        self.tx
            .send(OwnerRequest::Inject {
                op,
                reply: reply_tx,
            })
            .map_err(|_| Error::Unsupported("the input owner is gone"))?;
        reply_rx
            .recv()
            .map_err(|_| Error::Unsupported("the input owner is gone"))?
    }
}

fn run_owner<B, I>(block: &mut B, inject: &mut I, rx: Receiver<OwnerRequest>, blocked: &AtomicBool)
where
    B: BlockBackend,
    I: InjectBackend,
{
    for request in rx {
        match request {
            OwnerRequest::SetBlocked {
                blocked: want,
                reply,
            } => {
                let outcome = block.set_blocked(want);
                if outcome.is_ok() {
                    blocked.store(want, Ordering::SeqCst);
                    if !want {
                        inject.release_held();
                    }
                }
                let _ = reply.send(outcome);
            }
            OwnerRequest::Inject { op, reply } => {
                let _ = reply.send(inject.inject(&op));
            }
        }
    }
}

static GLOBAL: OnceLock<Owner> = OnceLock::new();

/// Blocks or restores process input through the shared owner, installing
/// it on first use. Makers run at most once each — concurrent installers
/// race, and only the winner's backends serve — and never for a redundant
/// call: restoring input that was never blocked returns without spawning.
pub fn set_blocked<B, I, MB, MI>(
    make_block: MB,
    make_inject: MI,
    blocked: bool,
) -> Result<(), Error>
where
    B: BlockBackend,
    I: InjectBackend,
    MB: FnOnce() -> B,
    MI: FnOnce() -> I,
{
    if let Some(owner) = GLOBAL.get() {
        return owner.set_blocked(blocked);
    }
    if !blocked {
        return Ok(());
    }
    let spare = Owner::spawn(make_block(), make_inject())?;
    // A lost install race drops the spare owner unused — its thread exits
    // on the disconnected channel having served nothing — and this call
    // joins the winner, so one thread still owns every call.
    let _ = GLOBAL.set(spare);
    GLOBAL.get().expect("owner installed").set_blocked(blocked)
}

/// True once an acknowledged block is in force. False before the first
/// install, so platforms without an owner always inject direct.
pub(crate) fn is_blocked() -> bool {
    GLOBAL.get().is_some_and(Owner::is_blocked)
}

/// Injects through the shared owner. Only called while blocked, which
/// implies an install; the fallback is defensive.
pub(crate) fn inject(op: InjectOp) -> Result<(), Error> {
    GLOBAL
        .get()
        .ok_or(Error::Unsupported("the input owner is gone"))?
        .inject(op)
}

/// Real injection backend: a lazily opened [`crate::os_input::OsInput`].
/// The handle opens inside the owner thread on first use, so blocking
/// works even where injection cannot initialize. Dropping it on unblock
/// releases any presses the blocked period abandoned.
pub struct OsInjectBackend {
    inner: Option<crate::os_input::OsInput>,
}

/// Builds the real injection backend. The OS handle opens lazily.
pub fn make_os_inject_backend() -> impl InjectBackend {
    OsInjectBackend { inner: None }
}

impl InjectBackend for OsInjectBackend {
    fn inject(&mut self, op: &InjectOp) -> Result<(), Error> {
        let input = match self.inner.as_mut() {
            Some(input) => input,
            None => {
                self.inner = Some(crate::os_input::OsInput::open()?);
                self.inner.as_mut().expect("just opened")
            }
        };
        match op {
            InjectOp::Apply(event) => input.apply_direct(*event),
            InjectOp::TypeText(text) => input.type_text_direct(text),
            InjectOp::TypeChord(text) => input.type_chord_direct(text),
        }
    }

    fn release_held(&mut self) {
        self.inner = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[derive(Debug, PartialEq, Eq)]
    enum Call {
        Block(bool),
        Inject(InjectOp),
        ReleaseHeld,
    }

    struct CallLog {
        thread: std::thread::ThreadId,
        name: Option<String>,
        call: Call,
    }

    /// Thread-affine fake: records every backend call with its calling
    /// thread. `fail_unblock_once` refuses one restoration.
    struct Fake {
        log: std::sync::Arc<Mutex<Vec<CallLog>>>,
        fail_unblock_once: AtomicBool,
    }

    impl Fake {
        fn record(&self, call: Call) {
            let current = std::thread::current();
            self.log.lock().unwrap().push(CallLog {
                thread: current.id(),
                name: current.name().map(str::to_owned),
                call,
            });
        }
    }

    impl BlockBackend for Fake {
        fn set_blocked(&mut self, blocked: bool) -> Result<(), Error> {
            self.record(Call::Block(blocked));
            if !blocked && self.fail_unblock_once.swap(false, Ordering::SeqCst) {
                return Err(Error::Unsupported("unblock refused"));
            }
            Ok(())
        }
    }

    impl InjectBackend for Fake {
        fn inject(&mut self, op: &InjectOp) -> Result<(), Error> {
            self.record(Call::Inject(op.clone()));
            Ok(())
        }

        fn release_held(&mut self) {
            self.record(Call::ReleaseHeld);
        }
    }

    fn fake() -> (Fake, std::sync::Arc<Mutex<Vec<CallLog>>>) {
        let log = std::sync::Arc::new(Mutex::new(Vec::new()));
        (
            Fake {
                log: log.clone(),
                fail_unblock_once: AtomicBool::new(false),
            },
            log,
        )
    }

    fn event() -> InputEvent {
        InputEvent::Motion { dx: 1, dy: 2 }
    }

    #[test]
    fn one_thread_owns_blocking_restoration_and_injection() {
        let (block, block_log) = fake();
        let (inject, inject_log) = fake();
        let owner = Owner::spawn(block, inject).unwrap();
        let main = std::thread::current().id();
        assert!(!owner.is_blocked());
        owner.set_blocked(true).unwrap();
        assert!(owner.is_blocked());
        std::thread::scope(|scope| {
            scope.spawn(|| owner.inject(InjectOp::Apply(event())).unwrap());
            scope.spawn(|| owner.inject(InjectOp::TypeText("hi".into())).unwrap());
            scope
                .spawn(|| owner.inject(InjectOp::TypeChord("ß".into())).unwrap())
                .join()
                .unwrap();
        });
        owner.set_blocked(false).unwrap();
        assert!(!owner.is_blocked());

        let blocks = block_log.lock().unwrap();
        assert_eq!(
            blocks.iter().map(|entry| &entry.call).collect::<Vec<_>>(),
            [&Call::Block(true), &Call::Block(false)]
        );
        let owner_thread = blocks[0].thread;
        assert_ne!(owner_thread, main);
        assert_eq!(blocks[0].name.as_deref(), Some("vnilink-input-owner"));
        assert!(blocks.iter().all(|entry| entry.thread == owner_thread));

        let injected = inject_log.lock().unwrap();
        assert_eq!(injected.len(), 4);
        assert!(injected.iter().all(|entry| entry.thread == owner_thread));
        assert!(injected
            .iter()
            .any(|entry| entry.call == Call::Inject(InjectOp::Apply(event()))));
        assert!(injected
            .iter()
            .any(|entry| entry.call == Call::Inject(InjectOp::TypeText("hi".into()))));
        assert!(injected
            .iter()
            .any(|entry| entry.call == Call::Inject(InjectOp::TypeChord("ß".into()))));
        // The acknowledged unblock released held presses exactly once.
        assert_eq!(
            injected
                .iter()
                .filter(|entry| entry.call == Call::ReleaseHeld)
                .count(),
            1
        );
    }

    #[test]
    fn failed_unblock_retains_ownership_for_retry_on_the_same_thread() {
        let (block, block_log) = fake();
        let (inject, inject_log) = fake();
        block.fail_unblock_once.store(true, Ordering::SeqCst);
        let owner = Owner::spawn(block, inject).unwrap();
        owner.set_blocked(true).unwrap();
        assert!(matches!(
            owner.set_blocked(false),
            Err(Error::Unsupported(_))
        ));
        // Ownership retained: still blocked, nothing released.
        assert!(owner.is_blocked());
        assert!(inject_log.lock().unwrap().is_empty());
        owner.set_blocked(false).unwrap();
        assert!(!owner.is_blocked());

        let blocks = block_log.lock().unwrap();
        assert_eq!(
            blocks.iter().map(|entry| &entry.call).collect::<Vec<_>>(),
            [&Call::Block(true), &Call::Block(false), &Call::Block(false)]
        );
        let owner_thread = blocks[0].thread;
        assert!(blocks.iter().all(|entry| entry.thread == owner_thread));
        // Only the acknowledged unblock released held presses.
        assert_eq!(
            inject_log
                .lock()
                .unwrap()
                .iter()
                .filter(|entry| entry.call == Call::ReleaseHeld)
                .count(),
            1
        );
    }

    #[test]
    fn redundant_sets_skip_the_thread() {
        let (block, block_log) = fake();
        let (inject, _) = fake();
        let owner = Owner::spawn(block, inject).unwrap();
        owner.set_blocked(false).unwrap();
        assert!(block_log.lock().unwrap().is_empty());
        owner.set_blocked(true).unwrap();
        owner.set_blocked(true).unwrap();
        assert_eq!(block_log.lock().unwrap().len(), 1);
        owner.set_blocked(false).unwrap();
        assert_eq!(block_log.lock().unwrap().len(), 2);
    }

    #[test]
    fn uninstalled_global_skips_install_and_reports_unavailable() {
        // No test in this binary installs the global owner (instances
        // above cover the protocol; production installers are cfg'd to
        // Windows), so these assertions observe the pristine global.
        static MAKER_CALLS: AtomicBool = AtomicBool::new(false);
        assert!(matches!(
            inject(InjectOp::Apply(event())),
            Err(Error::Unsupported(_))
        ));
        set_blocked(
            || {
                MAKER_CALLS.store(true, Ordering::SeqCst);
                fake().0
            },
            || fake().0,
            false,
        )
        .unwrap();
        assert!(!MAKER_CALLS.load(Ordering::SeqCst));
        assert!(!is_blocked());
    }
}
