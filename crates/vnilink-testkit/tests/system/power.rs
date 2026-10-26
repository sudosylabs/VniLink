//! Power while a desktop is open: the host and its controllers keep their
//! displays and systems awake for as long as live desktops want it, an
//! idle desktop expires alone, and a host locks itself once control it
//! gave was used and ended.

use expect_test::expect;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

use vnilink::power::{
    HostPowerFactory, PowerInhibitor, PowerKind, PowerPhase, PowerPolicy, PowerStatus,
};
use vnilink::{
    usage, BackendStartup, Core, CoreEvent, DesktopControlState, Error, EventSubscription,
    HostSessionState, HostSystemActions, InputEvent, RemoteDesktopService, ServiceKind,
    SessionPhase, SystemSupport,
};
use vnilink_testkit::desktop::{keyframe, next_picture, InputLog, MonitorCapture, SCREEN};
use vnilink_testkit::{holds, traced, wait_for, Engine, Pair, TestResult, DEFAULT_WAIT};

/// Where a kind's counts are kept.
fn slot(kind: PowerKind) -> usize {
    match kind {
        PowerKind::Display => 0,
        PowerKind::System => 1,
    }
}

/// How many leases of each kind an engine's OS gave and took back.
#[derive(Default)]
struct Counts {
    acquired: [AtomicUsize; 2],
    released: [AtomicUsize; 2],
}

impl Counts {
    fn acquired(&self, kind: PowerKind) -> usize {
        self.acquired[slot(kind)].load(Ordering::SeqCst)
    }
    fn released(&self, kind: PowerKind) -> usize {
        self.released[slot(kind)].load(Ordering::SeqCst)
    }
}

/// An OS lease that keeps a display or the system awake while held.
struct Lease(Arc<Counts>, PowerKind);

impl PowerInhibitor for Lease {}

impl Drop for Lease {
    fn drop(&mut self) {
        self.0.released[slot(self.1)].fetch_add(1, Ordering::SeqCst);
    }
}

fn lease(counts: &Arc<Counts>, kind: PowerKind) -> Box<dyn PowerInhibitor> {
    counts.acquired[slot(kind)].fetch_add(1, Ordering::SeqCst);
    Box::new(Lease(Arc::clone(counts), kind))
}

/// An OS that gives each lease at once and counts it.
struct Leases(Arc<Counts>);

impl HostPowerFactory for Leases {
    fn acquire(
        &self,
        kind: PowerKind,
        startup: &BackendStartup,
    ) -> Result<Box<dyn PowerInhibitor>, Error> {
        startup.check()?;
        Ok(lease(&self.0, kind))
    }
}

/// A host that may be seen and controlled, capturing one monitor, and a
/// controller paired with it.
fn seen() -> Result<Pair, Error> {
    let pair = Pair::for_desktop();
    pair.host
        .core
        .set_capture(Box::new(MonitorCapture(SCREEN)))?;
    Ok(pair)
}

/// `controller` with a Desktop service open on a services connection to
/// `host`.
fn open(host: &Engine, controller: &Engine) -> Result<RemoteDesktopService, Error> {
    controller.open_desktop(&controller.connect_services(host)?)
}

fn status(core: &Core, kind: PowerKind) -> Option<PowerStatus> {
    core.power_status()
        .ok()?
        .into_iter()
        .find(|status| status.kind == kind)
}

/// `core` keeps `kind` awake for `owners` desktops.
fn active(core: &Core, kind: PowerKind, owners: u32) -> bool {
    status(core, kind)
        .is_some_and(|status| status.owners == owners && status.phase == PowerPhase::Active)
}

/// A host keeps its display and system awake once for all its live
/// desktops, and a controller its display for its own; each lease goes
/// when its last desktop closes or the policy stops wanting it, while the
/// connections stay.
#[test]
fn inhibitors_are_counted_by_live_desktop_owners_and_roles() -> TestResult {
    let _trace = traced();
    let pair = seen()?;
    let (host, a) = (&pair.host, &pair.controller);
    let b = pair.add_viewer("b");
    let host_counts = Arc::new(Counts::default());
    let a_counts = Arc::new(Counts::default());
    host.core
        .set_power_factory(Some(Arc::new(Leases(Arc::clone(&host_counts)))))?;
    a.core
        .set_power_factory(Some(Arc::new(Leases(Arc::clone(&a_counts)))))?;
    host.core.set_power_policy(PowerPolicy {
        incoming_display: true,
        incoming_system: true,
        ..Default::default()
    })?;
    a.core.set_power_policy(PowerPolicy {
        outgoing_display: true,
        ..Default::default()
    })?;
    let first = open(host, a)?;
    let second = open(host, &b)?;
    wait_for("both engines to keep awake", DEFAULT_WAIT, || {
        (active(&host.core, PowerKind::Display, 2)
            && active(&host.core, PowerKind::System, 2)
            && active(&a.core, PowerKind::Display, 1))
        .then_some(())
    });
    assert_eq!(
        host_counts.acquired(PowerKind::Display),
        1,
        "the host holds one display lease for both desktops"
    );
    assert_eq!(
        host_counts.acquired(PowerKind::System),
        1,
        "the host holds one system lease for both desktops"
    );
    assert_eq!(
        a_counts.acquired(PowerKind::System),
        0,
        "a controller keeps only what its policy wants awake"
    );
    first.close();
    wait_for("the first desktop's lease to go", DEFAULT_WAIT, || {
        (active(&host.core, PowerKind::Display, 1) && a_counts.released(PowerKind::Display) == 1)
            .then_some(())
    });
    assert_eq!(
        host_counts.released(PowerKind::Display),
        0,
        "the host keeps its display lease for the second desktop"
    );
    assert_eq!(
        host_counts.released(PowerKind::System),
        0,
        "the host keeps its system lease for the second desktop"
    );
    host.core.set_power_policy(PowerPolicy {
        incoming_system: true,
        ..Default::default()
    })?;
    wait_for(
        "the display lease the policy no longer wants to go",
        DEFAULT_WAIT,
        || (host_counts.released(PowerKind::Display) == 1).then_some(()),
    );
    assert!(
        active(&host.core, PowerKind::System, 1),
        "the system lease stays for the second desktop"
    );
    second.close();
    wait_for("the system lease to go", DEFAULT_WAIT, || {
        (host_counts.released(PowerKind::System) == 1).then_some(())
    });
    assert!(
        host.core
            .power_status()?
            .iter()
            .all(|status| status.owners == 0 && status.phase == PowerPhase::Idle),
        "a host without desktops keeps nothing awake"
    );
    assert_eq!(
        a.core.controller_sessions()?.len(),
        1,
        "the first connection outlives its desktop"
    );
    assert_eq!(
        b.core.controller_sessions()?.len(),
        1,
        "the second connection outlives its desktop"
    );
    Ok(())
}

/// A lease the OS gives after its desktop closed is dropped at once; a
/// live lease is given back when the engine shuts down.
#[test]
fn cancelled_native_acquisition_drops_late_success_and_shutdown_releases_live_lease() -> TestResult
{
    struct Delayed {
        counts: Arc<Counts>,
        entered: mpsc::Sender<()>,
        release: Mutex<mpsc::Receiver<()>>,
    }
    impl HostPowerFactory for Delayed {
        fn acquire(
            &self,
            kind: PowerKind,
            _: &BackendStartup,
        ) -> Result<Box<dyn PowerInhibitor>, Error> {
            self.entered
                .send(())
                .expect("the test waits for the acquire");
            self.release
                .lock()
                .unwrap_or_else(|poison| poison.into_inner())
                .recv()
                .expect("the test lets the acquire finish");
            Ok(lease(&self.counts, kind))
        }
    }
    let _trace = traced();
    let pair = seen()?;
    let (host, controller) = (&pair.host, &pair.controller);
    let counts = Arc::new(Counts::default());
    let (entered, entering) = mpsc::channel();
    let (release, released) = mpsc::channel();
    controller.core.set_power_factory(Some(Arc::new(Delayed {
        counts: Arc::clone(&counts),
        entered,
        release: Mutex::new(released),
    })))?;
    controller.core.set_power_policy(PowerPolicy {
        outgoing_display: true,
        ..Default::default()
    })?;
    let desktop = open(host, controller)?;
    entering
        .recv_timeout(Duration::from_secs(5))
        .expect("the controller asks its OS for a display lease");
    desktop.close();
    wait_for(
        "the controller to stop wanting a lease",
        DEFAULT_WAIT,
        || (status(&controller.core, PowerKind::Display)?.phase == PowerPhase::Idle).then_some(()),
    );
    release.send(())?;
    wait_for("the late lease to be dropped", DEFAULT_WAIT, || {
        (counts.released(PowerKind::Display) == 1).then_some(())
    });
    assert_eq!(
        counts.acquired(PowerKind::Display),
        1,
        "the late lease was the only one given"
    );
    controller
        .core
        .set_power_factory(Some(Arc::new(Leases(Arc::clone(&counts)))))?;
    let connection = controller
        .core
        .controller_sessions()?
        .into_iter()
        .next()
        .ok_or("the controller's connection")?;
    let owner = controller.core.attach_controller_service(
        connection.id,
        connection.generation,
        ServiceKind::Desktop,
    )?;
    let _desktop = controller
        .core
        .open_desktop_service(owner.id, owner.generation)?;
    wait_for(
        "the controller to keep its display awake",
        DEFAULT_WAIT,
        || active(&controller.core, PowerKind::Display, 1).then_some(()),
    );
    controller.core.shutdown();
    wait_for("shutdown to give the lease back", DEFAULT_WAIT, || {
        (counts.released(PowerKind::Display) == 2).then_some(())
    });
    Ok(())
}

/// A lease the OS refuses is reported as failed and not asked for again
/// on every tick; with no OS backend it is unavailable.
#[test]
fn rejected_power_adapter_is_reported_without_retrying_every_tick() -> TestResult {
    struct Rejected(Arc<AtomicUsize>);
    impl HostPowerFactory for Rejected {
        fn acquire(
            &self,
            _: PowerKind,
            _: &BackendStartup,
        ) -> Result<Box<dyn PowerInhibitor>, Error> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Err(Error::Unsupported("the lease was refused by the OS"))
        }
    }
    let _trace = traced();
    let pair = seen()?;
    let (host, controller) = (&pair.host, &pair.controller);
    let calls = Arc::new(AtomicUsize::new(0));
    controller
        .core
        .set_power_factory(Some(Arc::new(Rejected(Arc::clone(&calls)))))?;
    controller.core.set_power_policy(PowerPolicy {
        outgoing_system: true,
        ..Default::default()
    })?;
    let _desktop = open(host, controller)?;
    wait_for("the refused lease to be reported", DEFAULT_WAIT, || {
        matches!(
            status(&controller.core, PowerKind::System)?.phase,
            PowerPhase::Failed(_)
        )
        .then_some(())
    });
    holds(
        "the refused lease is asked for once",
        Duration::from_millis(150),
        || calls.load(Ordering::SeqCst) == 1,
    );
    controller.core.set_power_factory(None)?;
    wait_for("the lease to be unavailable", DEFAULT_WAIT, || {
        (status(&controller.core, PowerKind::System)?.phase == PowerPhase::Unavailable)
            .then_some(())
    });
    Ok(())
}

/// A hosted desktop with no input expires on its own, however often its
/// viewer asks for pictures; its connection and the connection's other
/// services stay. An idle limit beyond a day is refused.
#[test]
fn desktop_idle_expiry_keeps_parent_and_files_owner_and_does_not_count_media_requests() -> TestResult
{
    let trace = traced();
    let pair = seen()?;
    let (host, controller) = (&pair.host, &pair.controller);
    let desktop = open(host, controller)?;
    host.core.set_service_sharing(ServiceKind::Files, true)?;
    let connection = controller
        .core
        .controller_sessions()?
        .into_iter()
        .next()
        .ok_or("the controller's connection")?;
    let files_owner = controller.core.attach_controller_service(
        connection.id,
        connection.generation,
        ServiceKind::Files,
    )?;
    let files = controller
        .core
        .service_lease(files_owner.id, files_owner.generation)?;
    // Set the incoming policy live; outgoing idle is disabled. Repeated media
    // maintenance must not refresh the host's Desktop user-activity clock.
    host.core.set_power_policy(PowerPolicy {
        incoming_idle_secs: 1,
        ..Default::default()
    })?;
    let mut asked: Option<Instant> = None;
    wait_for("the idle desktop to expire", Duration::from_secs(5), || {
        if desktop.is_cancelled() {
            return Some(());
        }
        if asked.is_none_or(|asked| asked.elapsed() >= Duration::from_millis(30)) {
            let _ = desktop.request_keyframe();
            asked = Some(Instant::now());
        }
        None
    });
    assert!(
        !files.is_cancelled(),
        "the connection's Files service outlives the desktop"
    );
    assert_eq!(
        controller.core.controller_session(connection.id)?.phase,
        SessionPhase::Active,
        "the connection outlives its idle desktop"
    );
    assert_eq!(
        host.core.hosted_sessions()?.len(),
        1,
        "the host keeps the connection"
    );
    assert!(
        matches!(
            host.core.set_power_policy(PowerPolicy {
                incoming_idle_secs: 86401,
                ..Default::default()
            }),
            Err(Error::InvalidSetting(_))
        ),
        "an idle limit beyond a day is refused"
    );
    assert_eq!(
        host.core.power_policy()?.incoming_idle_secs,
        1,
        "a refused policy is not applied"
    );
    trace.wait_for_records("desktop ended", 1);
    trace.wait_for_records("service phase", 2);
    expect![[r#"
        controller:
          session: session phase from=connecting generation=1 role=Controller session=1 to=active
          session: desktop ended hosted=false reason=peerClosed service=1
          service: service phase from=active generation=1 kind=Desktop service=1 to=closed: PeerClose
        host:
          service: service phase from=active generation=1 kind=Desktop service=1 to=closed: IdleExpired
    "#]].assert_eq(&trace.story(&["service", "session"]));
    Ok(())
}

/// Input the host applied keeps only its own desktop from expiring: the
/// other desktop expires, and the first once its input stops, both
/// connections staying.
#[test]
fn actual_input_refreshes_only_its_hosted_desktop_idle_clock() -> TestResult {
    let _trace = traced();
    let pair = seen()?;
    let (host, a) = (&pair.host, &pair.controller);
    let b = pair.add_viewer("b");
    let applied = InputLog::install(host, SCREEN)?;
    let first = open(host, a)?;
    let second = open(host, &b)?;
    first.acquire_control()?;
    wait_for("the first viewer to control", DEFAULT_WAIT, || {
        (first.control_state().ok()? == DesktopControlState::Controlling).then_some(())
    });
    host.core.set_power_policy(PowerPolicy {
        incoming_idle_secs: 1,
        ..Default::default()
    })?;
    let mut typed: Option<Instant> = None;
    holds(
        "the desktop given input stays open",
        Duration::from_millis(1_400),
        || {
            if typed.is_none_or(|typed| typed.elapsed() >= Duration::from_millis(100)) {
                first
                    .submit_input(InputEvent::key_down(usage::A))
                    .expect("the key goes");
                first
                    .submit_input(InputEvent::key_up(usage::A))
                    .expect("the key goes");
                typed = Some(Instant::now());
            }
            !first.is_cancelled()
        },
    );
    assert!(
        !applied.events().is_empty(),
        "the input reached the host's desktop"
    );
    wait_for("the desktop without input to expire", DEFAULT_WAIT, || {
        second.is_cancelled().then_some(())
    });
    wait_for(
        "the first desktop to expire once its input stops",
        DEFAULT_WAIT,
        || first.is_cancelled().then_some(()),
    );
    assert_eq!(
        a.core.controller_sessions()?[0].phase,
        SessionPhase::Active,
        "the first connection outlives its desktop"
    );
    assert_eq!(
        b.core.controller_sessions()?[0].phase,
        SessionPhase::Active,
        "the second connection outlives its desktop"
    );
    Ok(())
}

/// An OS that can lock and counts its locks, or refuses them.
#[derive(Default)]
struct LockCounter {
    locks: AtomicUsize,
    fail: AtomicBool,
}

impl LockCounter {
    fn locks(&self) -> usize {
        self.locks.load(Ordering::SeqCst)
    }
}

impl HostSystemActions for LockCounter {
    fn support(&self) -> SystemSupport {
        SystemSupport {
            lock: true,
            restart: false,
            shutdown: false,
            privacy: false,
            input_block: false,
        }
    }
    fn lock_workstation(&self) -> Result<(), Error> {
        if self.fail.load(Ordering::SeqCst) {
            return Err(Error::Unsupported("the lock was refused by the OS"));
        }
        self.locks.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
    fn restart(&self) -> Result<(), Error> {
        Err(Error::Unsupported("this OS only locks"))
    }
    fn shutdown(&self) -> Result<(), Error> {
        Err(Error::Unsupported("this OS only locks"))
    }
    fn set_display_private(&self, _: bool) -> Result<(), Error> {
        Err(Error::Unsupported("this OS only locks"))
    }
    fn set_input_blocked(&self, _: bool) -> Result<(), Error> {
        Err(Error::Unsupported("this OS only locks"))
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

/// A host that locks itself after control, with an OS that counts its
/// locks and a desktop that records the input it applies.
struct LockHost {
    pair: Pair,
    system: Arc<LockCounter>,
    applied: InputLog,
}

impl LockHost {
    fn start() -> Result<Self, Error> {
        let pair = seen()?;
        let engine = &pair.host;
        let applied = InputLog::install(engine, SCREEN)?;
        let system = Arc::new(LockCounter::default());
        engine
            .core
            .set_system_actions(Some(Arc::clone(&system) as Arc<dyn HostSystemActions>))?;
        engine.core.set_power_policy(PowerPolicy {
            lock_after_control: true,
            ..Default::default()
        })?;
        Ok(Self {
            pair,
            system,
            applied,
        })
    }

    fn host(&self) -> &Engine {
        &self.pair.host
    }

    /// `viewer`, paired with this host, with a Desktop service open.
    fn open(&self, viewer: &Engine) -> Result<RemoteDesktopService, Error> {
        open(&self.pair.host, viewer)
    }

    fn applied(&self) -> usize {
        self.applied.events().len()
    }

    /// Uses control from `desktop`: one key press reaches the host.
    fn use_control(&self, desktop: &RemoteDesktopService) -> Result<(), Error> {
        controlling(desktop);
        let before = self.applied();
        desktop.submit_input(InputEvent::key_down(usage::A))?;
        desktop.submit_input(InputEvent::key_up(usage::A))?;
        wait_for("the key to reach the host", DEFAULT_WAIT, || {
            (self.applied() >= before + 2).then_some(())
        });
        Ok(())
    }
}

/// Waits until `desktop` controls the host.
fn controlling(desktop: &RemoteDesktopService) {
    wait_for("the viewer to control", DEFAULT_WAIT, || {
        (desktop.control_state().ok()? == DesktopControlState::Controlling).then_some(())
    });
}

/// Waits for the event `wanted` picks, as the original's two-second budget.
fn next_event(events: &EventSubscription, what: &str, wanted: impl Fn(&CoreEvent) -> bool) {
    wait_for(what, Duration::from_secs(2), || {
        events
            .recv_timeout(Duration::from_millis(20))
            .filter(|event| wanted(event))
    });
}

/// Control granted and never used ends without locking. Used, an explicit
/// close with a live connection locks at once; relinquished control locks
/// too; a disabled policy never locks; an OS failure reports
/// `locked: false` and consumes the edge.
#[test]
fn lock_after_control_fires_on_close_yield_and_reports_failure() -> TestResult {
    let _trace = traced();
    let host = LockHost::start()?;
    let events = host.host().core.subscribe()?;
    // A viewer that never used control, such as one cancelled while
    // still connecting, leaves this computer unlocked.
    let watcher = host.pair.add_viewer("watcher");
    let unused = host.open(&watcher)?;
    controlling(&unused);
    unused.close();
    holds(
        "control never used does not lock",
        Duration::from_millis(300),
        || host.system.locks() == 0,
    );
    let desktop = host.open(&host.pair.controller)?;
    host.use_control(&desktop)?;
    desktop.close();
    wait_for("the host to lock", DEFAULT_WAIT, || {
        (host.system.locks() == 1).then_some(())
    });
    next_event(&events, "the host to say it locked", |event| {
        matches!(event, CoreEvent::HostLocked { locked: true })
    });
    // Relinquishing control with the owner still viewing ends control.
    let second = host.pair.add_viewer("second");
    let viewing = host.open(&second)?;
    host.use_control(&viewing)?;
    viewing.yield_control()?;
    wait_for(
        "the host to lock when control is given up",
        DEFAULT_WAIT,
        || (host.system.locks() == 2).then_some(()),
    );
    viewing.close();
    // A disabled policy never fires.
    host.host().core.set_power_policy(PowerPolicy::default())?;
    let third = host.pair.add_viewer("third");
    let last = host.open(&third)?;
    host.use_control(&last)?;
    last.close();
    holds(
        "a disabled policy does not lock",
        Duration::from_millis(300),
        || host.system.locks() == 2,
    );
    // An OS failure reports locked:false exactly once per edge.
    host.host().core.set_power_policy(PowerPolicy {
        lock_after_control: true,
        ..Default::default()
    })?;
    host.system.fail.store(true, Ordering::SeqCst);
    let fourth = host.pair.add_viewer("fourth");
    let failed = host.open(&fourth)?;
    host.use_control(&failed)?;
    failed.close();
    next_event(&events, "the host to say it did not lock", |event| {
        matches!(event, CoreEvent::HostLocked { locked: false })
    });
    holds(
        "a refused lock is not tried again",
        Duration::from_millis(200),
        || host.system.locks() == 2,
    );
    Ok(())
}

/// A picture that reached a controlling viewer is use too: closing the
/// desktop afterwards locks, though no input was sent.
#[test]
fn lock_after_control_counts_a_delivered_picture_as_use() -> TestResult {
    let _trace = traced();
    let host = LockHost::start()?;
    let desktop = host.open(&host.pair.controller)?;
    controlling(&desktop);
    let video = desktop.subscribe_video()?;
    let owner = host
        .host()
        .serving(ServiceKind::Desktop, &host.pair.controller);
    host.host().core.publish_desktop_video(
        owner.id,
        owner.generation,
        keyframe(1, vec![42; 64]),
    )?;
    next_picture(&video, "the picture to reach the viewer");
    desktop.close();
    wait_for("the host to lock", DEFAULT_WAIT, || {
        (host.system.locks() == 1).then_some(())
    });
    Ok(())
}

/// Control ending with no live connection (a full disconnect) waits out
/// the reconnect grace instead of locking at once. The 30-second expiry is
/// proven by the machine's own test in `vnilink`; here only the wait is
/// live.
#[test]
fn lock_after_control_waits_when_the_parent_is_gone() -> TestResult {
    let _trace = traced();
    let host = LockHost::start()?;
    let events = host.host().core.subscribe()?;
    let desktop = host.open(&host.pair.controller)?;
    host.use_control(&desktop)?;
    host.pair.controller.core.disconnect()?;
    wait_for("the host to lose the connection", DEFAULT_WAIT, || {
        host.host()
            .core
            .hosted_sessions()
            .ok()?
            .iter()
            .all(|session| session.phase != SessionPhase::Active)
            .then_some(())
    });
    holds(
        "no lock fires inside the grace window",
        Duration::from_secs(2),
        || {
            while let Some(event) = events.try_recv() {
                if matches!(event, CoreEvent::HostLocked { .. }) {
                    return false;
                }
            }
            host.system.locks() == 0
        },
    );
    drop(desktop);
    Ok(())
}
