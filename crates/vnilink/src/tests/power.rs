use super::support::{
    pair_for_control, start_direct, take_matching, RecordingDesktop, ShutdownCapture,
};
use crate::power::*;
use crate::{
    BackendStartup, Core, CoreEvent, DesktopControlState, Error, HostSessionState,
    HostSystemActions, InputEvent, RemoteDesktopService, ServiceKind, SessionPhase, SystemSupport,
};
use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Arc, Mutex,
};
use std::time::{Duration, Instant};

fn wait(mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(8);
    while !condition() {
        assert!(Instant::now() < deadline, "power reconciliation timed out");
        std::thread::sleep(Duration::from_millis(10));
    }
}
#[derive(Default)]
struct Counts {
    acquired: [AtomicUsize; 2],
    released: [AtomicUsize; 2],
}
struct Lease(Arc<Counts>, usize);
impl PowerInhibitor for Lease {}
impl Drop for Lease {
    fn drop(&mut self) {
        self.0.released[self.1].fetch_add(1, Ordering::SeqCst);
    }
}
struct Factory(Arc<Counts>);
impl HostPowerFactory for Factory {
    fn acquire(
        &self,
        kind: PowerKind,
        startup: &BackendStartup,
    ) -> Result<Box<dyn PowerInhibitor>, Error> {
        startup.check()?;
        let index = kind.index();
        self.0.acquired[index].fetch_add(1, Ordering::SeqCst);
        Ok(Box::new(Lease(self.0.clone(), index)))
    }
}
fn open(host: &Core, controller: &Core) -> RemoteDesktopService {
    pair_for_control(host, controller);
    let parent = controller
        .begin_connect_services(&host.endpoint_id())
        .unwrap();
    wait(|| controller.controller_session(parent.id).unwrap().phase == SessionPhase::Active);
    let parent = controller.controller_session(parent.id).unwrap();
    let owner = controller
        .attach_controller_service(parent.id, parent.generation, ServiceKind::Desktop)
        .unwrap();
    controller
        .open_desktop_service(owner.id, owner.generation)
        .unwrap()
}
fn active(core: &Core, kind: PowerKind, owners: u32) -> bool {
    core.power_status()
        .unwrap()
        .iter()
        .any(|s| s.kind == kind && s.owners == owners && s.phase == PowerPhase::Active)
}

#[test]
fn inhibitors_are_counted_by_live_desktop_owners_and_roles() {
    let host = start_direct();
    host.set_capture(Box::new(ShutdownCapture)).unwrap();
    let a = start_direct();
    let b = start_direct();
    let host_counts = Arc::new(Counts::default());
    let a_counts = Arc::new(Counts::default());
    host.set_power_factory(Some(Arc::new(Factory(host_counts.clone()))))
        .unwrap();
    a.set_power_factory(Some(Arc::new(Factory(a_counts.clone()))))
        .unwrap();
    host.set_power_policy(PowerPolicy {
        incoming_display: true,
        incoming_system: true,
        ..Default::default()
    })
    .unwrap();
    a.set_power_policy(PowerPolicy {
        outgoing_display: true,
        ..Default::default()
    })
    .unwrap();
    let first = open(&host, &a);
    let second = open(&host, &b);
    wait(|| {
        active(&host, PowerKind::Display, 2)
            && active(&host, PowerKind::System, 2)
            && active(&a, PowerKind::Display, 1)
    });
    assert_eq!(host_counts.acquired[0].load(Ordering::SeqCst), 1);
    assert_eq!(host_counts.acquired[1].load(Ordering::SeqCst), 1);
    assert_eq!(a_counts.acquired[1].load(Ordering::SeqCst), 0);
    first.close();
    wait(|| {
        active(&host, PowerKind::Display, 1) && a_counts.released[0].load(Ordering::SeqCst) == 1
    });
    assert_eq!(host_counts.released[0].load(Ordering::SeqCst), 0);
    assert_eq!(host_counts.released[1].load(Ordering::SeqCst), 0);
    host.set_power_policy(PowerPolicy {
        incoming_system: true,
        ..Default::default()
    })
    .unwrap();
    wait(|| host_counts.released[0].load(Ordering::SeqCst) == 1);
    assert!(active(&host, PowerKind::System, 1));
    second.close();
    wait(|| host_counts.released[1].load(Ordering::SeqCst) == 1);
    assert!(host
        .power_status()
        .unwrap()
        .iter()
        .all(|s| s.owners == 0 && s.phase == PowerPhase::Idle));
    // Parent transports still exist, but no Desktop lease keeps the OS awake.
    assert_eq!(a.controller_sessions().unwrap().len(), 1);
    assert_eq!(b.controller_sessions().unwrap().len(), 1);
}

#[test]
fn cancelled_native_acquisition_drops_late_success_and_shutdown_releases_live_lease() {
    struct Delayed {
        counts: Arc<Counts>,
        entered: std::sync::mpsc::Sender<()>,
        release: Mutex<std::sync::mpsc::Receiver<()>>,
    }
    impl HostPowerFactory for Delayed {
        fn acquire(
            &self,
            kind: PowerKind,
            _: &BackendStartup,
        ) -> Result<Box<dyn PowerInhibitor>, Error> {
            self.entered.send(()).unwrap();
            self.release.lock().unwrap().recv().unwrap();
            self.counts.acquired[kind.index()].fetch_add(1, Ordering::SeqCst);
            Ok(Box::new(Lease(self.counts.clone(), kind.index())))
        }
    }
    let host = start_direct();
    let controller = start_direct();
    host.set_capture(Box::new(ShutdownCapture)).unwrap();
    let counts = Arc::new(Counts::default());
    let (entered, entering) = std::sync::mpsc::channel();
    let (release, released) = std::sync::mpsc::channel();
    controller
        .set_power_factory(Some(Arc::new(Delayed {
            counts: counts.clone(),
            entered,
            release: Mutex::new(released),
        })))
        .unwrap();
    controller
        .set_power_policy(PowerPolicy {
            outgoing_display: true,
            ..Default::default()
        })
        .unwrap();
    let desktop = open(&host, &controller);
    entering.recv_timeout(Duration::from_secs(5)).unwrap();
    desktop.close();
    wait(|| controller.power_status().unwrap()[0].phase == PowerPhase::Idle);
    release.send(()).unwrap();
    wait(|| counts.released[0].load(Ordering::SeqCst) == 1);
    assert_eq!(counts.acquired[0].load(Ordering::SeqCst), 1);
    controller
        .set_power_factory(Some(Arc::new(Factory(counts.clone()))))
        .unwrap();
    let parent = controller.controller_sessions().unwrap().remove(0);
    let owner = controller
        .attach_controller_service(parent.id, parent.generation, ServiceKind::Desktop)
        .unwrap();
    let _desktop = controller
        .open_desktop_service(owner.id, owner.generation)
        .unwrap();
    wait(|| active(&controller, PowerKind::Display, 1));
    controller.shutdown();
    wait(|| counts.released[0].load(Ordering::SeqCst) == 2);
}

#[test]
fn rejected_power_adapter_is_reported_without_retrying_every_tick() {
    struct Rejected(Arc<AtomicUsize>);
    impl HostPowerFactory for Rejected {
        fn acquire(
            &self,
            _: PowerKind,
            _: &BackendStartup,
        ) -> Result<Box<dyn PowerInhibitor>, Error> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Err(Error::Unsupported("test native rejection"))
        }
    }
    let host = start_direct();
    host.set_capture(Box::new(ShutdownCapture)).unwrap();
    let controller = start_direct();
    let calls = Arc::new(AtomicUsize::new(0));
    controller
        .set_power_factory(Some(Arc::new(Rejected(calls.clone()))))
        .unwrap();
    controller
        .set_power_policy(PowerPolicy {
            outgoing_system: true,
            ..Default::default()
        })
        .unwrap();
    let _desktop = open(&host, &controller);
    wait(|| {
        matches!(
            controller.power_status().unwrap()[1].phase,
            PowerPhase::Failed(_)
        )
    });
    std::thread::sleep(Duration::from_millis(150));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    controller.set_power_factory(None).unwrap();
    wait(|| controller.power_status().unwrap()[1].phase == PowerPhase::Unavailable);
}

#[test]
fn desktop_idle_expiry_keeps_parent_and_files_owner_and_does_not_count_media_requests() {
    let host = start_direct();
    let controller = start_direct();
    host.set_capture(Box::new(ShutdownCapture)).unwrap();
    let desktop = open(&host, &controller);
    host.set_service_sharing(ServiceKind::Files, true).unwrap();
    let parent = controller.controller_sessions().unwrap().remove(0);
    let files_owner = controller
        .attach_controller_service(parent.id, parent.generation, ServiceKind::Files)
        .unwrap();
    let files = controller
        .service_lease(files_owner.id, files_owner.generation)
        .unwrap();
    // Set the incoming policy live; outgoing idle is disabled. Repeated media
    // maintenance must not refresh the host's Desktop user-activity clock.
    host.set_power_policy(PowerPolicy {
        incoming_idle_secs: 1,
        ..Default::default()
    })
    .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while !desktop.is_cancelled() {
        assert!(Instant::now() < deadline);
        let _ = desktop.request_keyframe();
        std::thread::sleep(Duration::from_millis(30));
    }
    assert!(!files.is_cancelled());
    assert_eq!(
        controller.controller_session(parent.id).unwrap().phase,
        SessionPhase::Active
    );
    assert_eq!(host.hosted_sessions().unwrap().len(), 1);
    assert!(matches!(
        host.set_power_policy(PowerPolicy {
            incoming_idle_secs: 86401,
            ..Default::default()
        }),
        Err(Error::InvalidSetting(_))
    ));
    assert_eq!(host.power_policy().unwrap().incoming_idle_secs, 1);
}

#[test]
fn actual_input_refreshes_only_its_hosted_desktop_idle_clock() {
    use super::support::RecordingDesktop;
    let host = start_direct();
    let a = start_direct();
    let b = start_direct();
    let applied = Arc::new(Mutex::new(Vec::new()));
    host.set_capture(Box::new(ShutdownCapture)).unwrap();
    host.set_desktop(Box::new(RecordingDesktop {
        events: applied.clone(),
    }))
    .unwrap();
    let first = open(&host, &a);
    let second = open(&host, &b);
    first.acquire_control().unwrap();
    wait(|| first.control_state().unwrap() == crate::DesktopControlState::Controlling);
    host.set_power_policy(PowerPolicy {
        incoming_idle_secs: 1,
        ..Default::default()
    })
    .unwrap();
    let finish = Instant::now() + Duration::from_millis(1400);
    while Instant::now() < finish {
        first
            .submit_input(crate::InputEvent::key_down(crate::usage::A))
            .unwrap();
        first
            .submit_input(crate::InputEvent::key_up(crate::usage::A))
            .unwrap();
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(
        !applied.lock().unwrap().is_empty(),
        "must reach host native adapter"
    );
    assert!(!first.is_cancelled());
    wait(|| second.is_cancelled());
    wait(|| first.is_cancelled());
    assert_eq!(
        a.controller_sessions().unwrap()[0].phase,
        SessionPhase::Active
    );
    assert_eq!(
        b.controller_sessions().unwrap()[0].phase,
        SessionPhase::Active
    );
}

/// Pure policy, no network: the machine never fires unused, fires once on
/// an explicit end with a live parent, waits out the grace without one, and
/// a redial inside the window resumes instead of locking.
#[test]
fn lock_machine_fires_once_and_graces_parent_loss() {
    let start = Instant::now();
    let mut machine = LockAfterControl::default();
    // tick(now, enabled, controlling, used, parent_live)
    assert_eq!(
        machine.tick(start, true, false, false, false),
        LockDecision::Idle
    );
    assert_eq!(
        machine.tick(start, true, true, true, true),
        LockDecision::Idle
    );
    assert_eq!(
        machine.tick(start, false, false, false, true),
        LockDecision::Idle
    );
    assert_eq!(
        machine.tick(start, true, true, true, true),
        LockDecision::Idle
    );
    assert_eq!(
        machine.tick(start, true, false, false, true),
        LockDecision::Fire
    );
    assert_eq!(
        machine.tick(start, true, false, false, true),
        LockDecision::Idle
    );
    assert_eq!(
        machine.tick(start, true, true, true, false),
        LockDecision::Idle
    );
    assert_eq!(
        machine.tick(start, true, false, false, false),
        LockDecision::Wait
    );
    assert_eq!(
        machine.tick(start + Duration::from_secs(29), true, false, false, false),
        LockDecision::Wait
    );
    assert_eq!(
        machine.tick(start + Duration::from_secs(30), true, false, false, false),
        LockDecision::Fire
    );
    assert_eq!(
        machine.tick(start, true, true, true, false),
        LockDecision::Idle
    );
    assert_eq!(
        machine.tick(start, true, false, false, false),
        LockDecision::Wait
    );
    assert_eq!(
        machine.tick(start + Duration::from_secs(5), true, true, false, true),
        LockDecision::Idle
    );
    assert_eq!(
        machine.tick(start + Duration::from_secs(31), true, false, false, true),
        LockDecision::Fire
    );
    // A use and an end within one tick still fire on the next sample.
    let mut quick = LockAfterControl::default();
    assert_eq!(
        quick.tick(start, true, false, true, true),
        LockDecision::Fire
    );
}

/// Control granted but never used, such as a viewer cancelled while still
/// connecting, ends without locking; using it once arms the lock until
/// control ends.
#[test]
fn lock_machine_ignores_control_that_was_never_used() {
    let start = Instant::now();
    let mut machine = LockAfterControl::default();
    assert_eq!(
        machine.tick(start, true, true, false, true),
        LockDecision::Idle
    );
    assert_eq!(
        machine.tick(start, true, false, false, true),
        LockDecision::Idle
    );
    assert_eq!(
        machine.tick(start, true, false, false, false),
        LockDecision::Idle
    );
    assert_eq!(
        machine.tick(start, true, true, true, true),
        LockDecision::Idle
    );
    assert_eq!(
        machine.tick(start, true, true, false, true),
        LockDecision::Idle
    );
    assert_eq!(
        machine.tick(start, true, false, false, true),
        LockDecision::Fire
    );
}

#[derive(Default)]
struct LockCounter {
    locks: AtomicUsize,
    fail: AtomicBool,
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
            return Err(Error::Unsupported("fake lock refused by the OS"));
        }
        self.locks.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
    fn restart(&self) -> Result<(), Error> {
        Err(Error::Unsupported("lock counter"))
    }
    fn shutdown(&self) -> Result<(), Error> {
        Err(Error::Unsupported("lock counter"))
    }
    fn set_display_private(&self, _private: bool) -> Result<(), Error> {
        Err(Error::Unsupported("lock counter"))
    }
    fn set_input_blocked(&self, _blocked: bool) -> Result<(), Error> {
        Err(Error::Unsupported("lock counter"))
    }
    fn session_state(&self) -> HostSessionState {
        HostSessionState {
            locked: Some(false),
            login_screen: Some(false),
            headless: false,
            session: "test".into(),
        }
    }
}

fn controlling(desktop: &RemoteDesktopService) -> bool {
    desktop.control_state().unwrap() == DesktopControlState::Controlling
}

/// Lock-after-control hosts with a desktop that records the input it
/// applies, so a test can use control and know it arrived.
fn lock_host(actions: &Arc<LockCounter>) -> (Core, Arc<Mutex<Vec<InputEvent>>>) {
    let host = start_direct();
    host.set_capture(Box::new(ShutdownCapture)).unwrap();
    let applied = Arc::new(Mutex::new(Vec::new()));
    host.set_desktop(Box::new(RecordingDesktop {
        events: applied.clone(),
    }))
    .unwrap();
    host.set_system_actions(Some(actions.clone())).unwrap();
    host.set_power_policy(PowerPolicy {
        lock_after_control: true,
        ..Default::default()
    })
    .unwrap();
    (host, applied)
}

/// Uses control: one key press reaches the host's desktop.
fn use_control(desktop: &RemoteDesktopService, applied: &Mutex<Vec<InputEvent>>) {
    wait(|| controlling(desktop));
    let before = applied.lock().unwrap().len();
    desktop
        .submit_input(InputEvent::key_down(crate::input::usage::A))
        .unwrap();
    desktop
        .submit_input(InputEvent::key_up(crate::input::usage::A))
        .unwrap();
    wait(|| applied.lock().unwrap().len() >= before + 2);
}

/// Control granted and never used ends without locking. Used, an explicit
/// owner close with a live parent locks at once; relinquished control locks
/// too; a disabled policy locks never; an OS failure reports
/// `locked: false` and consumes the edge.
#[test]
fn lock_after_control_fires_on_close_yield_and_reports_failure() {
    let actions = Arc::new(LockCounter::default());
    let (host, applied) = lock_host(&actions);
    let events = host.subscribe().unwrap();
    // A viewer that never used control, such as one cancelled while
    // still connecting, leaves this computer unlocked.
    let watcher = start_direct();
    let unused = open(&host, &watcher);
    wait(|| controlling(&unused));
    unused.close();
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(actions.locks.load(Ordering::SeqCst), 0);
    let controller = start_direct();
    let desktop = open(&host, &controller);
    use_control(&desktop, &applied);
    desktop.close();
    wait(|| actions.locks.load(Ordering::SeqCst) == 1);
    take_matching(&events, |event| {
        matches!(event, CoreEvent::HostLocked { locked: true })
    });
    // Relinquishing control with the owner still viewing ends control.
    let second = start_direct();
    let viewing = open(&host, &second);
    use_control(&viewing, &applied);
    viewing.yield_control().unwrap();
    wait(|| actions.locks.load(Ordering::SeqCst) == 2);
    viewing.close();
    // A disabled policy never fires.
    host.set_power_policy(PowerPolicy::default()).unwrap();
    let third = start_direct();
    let last = open(&host, &third);
    use_control(&last, &applied);
    last.close();
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(actions.locks.load(Ordering::SeqCst), 2);
    // An OS failure reports locked:false exactly once per edge.
    host.set_power_policy(PowerPolicy {
        lock_after_control: true,
        ..Default::default()
    })
    .unwrap();
    actions.fail.store(true, Ordering::SeqCst);
    let fourth = start_direct();
    let failed = open(&host, &fourth);
    use_control(&failed, &applied);
    failed.close();
    take_matching(&events, |event| {
        matches!(event, CoreEvent::HostLocked { locked: false })
    });
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(actions.locks.load(Ordering::SeqCst), 2);
}

/// A picture that reached a controlling viewer is use too: closing the
/// owner afterwards locks, though no input was sent.
#[test]
fn lock_after_control_counts_a_delivered_picture_as_use() {
    let actions = Arc::new(LockCounter::default());
    let (host, _applied) = lock_host(&actions);
    let controller = start_direct();
    let desktop = open(&host, &controller);
    wait(|| controlling(&desktop));
    let video = desktop.subscribe_video().unwrap();
    let owner = host
        .hosted_services()
        .unwrap()
        .into_iter()
        .find(|service| service.kind == ServiceKind::Desktop)
        .unwrap();
    host.publish_desktop_video(
        owner.id,
        owner.generation,
        crate::EncodedFrame::h264(true, 1, vec![42; 64]).unwrap(),
    )
    .unwrap();
    video.recv_timeout(Duration::from_secs(5)).unwrap();
    desktop.close();
    wait(|| actions.locks.load(Ordering::SeqCst) == 1);
}

/// Control ending with no live parent (full disconnect) waits out the
/// reconnect grace instead of locking at once. The 30-second expiry is
/// proven by the pure machine test; here only the wait is live.
#[test]
fn lock_after_control_waits_when_the_parent_is_gone() {
    let actions = Arc::new(LockCounter::default());
    let (host, applied) = lock_host(&actions);
    let events = host.subscribe().unwrap();
    let controller = start_direct();
    let desktop = open(&host, &controller);
    use_control(&desktop, &applied);
    controller.disconnect().unwrap();
    wait(|| {
        host.hosted_sessions()
            .unwrap()
            .iter()
            .all(|s| s.phase != SessionPhase::Active)
    });
    std::thread::sleep(Duration::from_secs(2));
    assert_eq!(actions.locks.load(Ordering::SeqCst), 0);
    assert!(
        events
            .recv_timeout(Duration::from_millis(100))
            .is_none_or(|event| !matches!(event, CoreEvent::HostLocked { .. })),
        "no lock may fire inside the grace window"
    );
    drop(desktop);
}
