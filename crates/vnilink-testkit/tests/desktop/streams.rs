//! The pictures themselves: capture fenced to its consumers, pressure that
//! adapts one owner, twin sources, conditions the viewer learns, why a
//! picture ended, pictures reaching a session's viewer, and the size a
//! host serves.

use expect_test::expect;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use vnilink::{
    AdaptationReason, Condition, CoreEvent, DesktopControlState, DesktopEnd, DisplayInfo,
    EncodedFrame, EncoderImplementation, Error, EventSubscription, HostCapture, HostDesktop,
    InputEvent, RemoteDesktopService, ServiceInfo, ServiceKind, SessionInfo, SessionPhase,
    StreamEnd, StreamFailure, VideoCodecCaps, VideoSubscription,
};
use vnilink_testkit::{holds, traced, wait_for, Engine, Pair, TestResult, DEFAULT_WAIT};

/// The size of the host's screens.
pub(super) const SCREEN: (u16, u16) = (1920, 1080);

/// A host's physical capture, as the engine sees it: one monitor of this
/// size.
pub(super) struct Capture(pub(super) (u16, u16));

impl HostCapture for Capture {
    fn monitor(&self) -> (u16, u16) {
        self.0
    }
}

/// A desktop of a monitor's size that applies input by recording it.
pub(super) struct Applied {
    events: Arc<Mutex<Vec<InputEvent>>>,
    monitor: (u16, u16),
}

impl Applied {
    /// A desktop of [`SCREEN`], and what it applies.
    pub(super) fn new() -> (Self, Arc<Mutex<Vec<InputEvent>>>) {
        Self::sized(SCREEN)
    }

    pub(super) fn sized(monitor: (u16, u16)) -> (Self, Arc<Mutex<Vec<InputEvent>>>) {
        let events = Arc::new(Mutex::new(Vec::new()));
        let desktop = Self {
            events: Arc::clone(&events),
            monitor,
        };
        (desktop, events)
    }
}

impl HostDesktop for Applied {
    fn apply(&mut self, event: InputEvent) -> Result<(), Error> {
        self.events
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .push(event);
        Ok(())
    }

    fn monitor(&self) -> (u16, u16) {
        self.monitor
    }
}

/// What the host's desktop applied so far.
pub(super) fn applied(events: &Mutex<Vec<InputEvent>>) -> Vec<InputEvent> {
    events
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
        .clone()
}

/// A display of the host, at revision `revision`.
pub(super) fn display(id: u64, x: i32, width: u16, revision: u64) -> DisplayInfo {
    DisplayInfo {
        id,
        origin_x: x,
        origin_y: 0,
        width,
        height: SCREEN.1,
        rotation_deg: 0,
        scale_percent: 100,
        revision,
    }
}

/// Another controller, named `name`, paired with the pair's host as the
/// pair's own controller is.
pub(super) fn another_viewer(pair: &Pair, name: &str) -> Result<Engine, Error> {
    let viewer = Engine::start(name);
    let invitation = Duration::from_secs(60);
    let token = pair.host.core.create_invitation(invitation)?;
    viewer.core.accept_invitation(&token)?;
    let back = viewer.core.create_invitation(invitation)?;
    pair.host.core.accept_invitation(&back)?;
    Ok(viewer)
}

/// `viewer`'s services connection to `host`, once the host admitted it.
pub(super) fn connect(viewer: &Engine, host: &Engine) -> Result<SessionInfo, Error> {
    let pending = viewer.core.begin_connect_services(&host.endpoint())?;
    Ok(wait_for(
        "the connection to be admitted",
        DEFAULT_WAIT,
        || {
            let session = viewer.core.controller_session(pending.id).ok()?;
            (session.phase == SessionPhase::Active).then_some(session)
        },
    ))
}

/// A Desktop owner on `connection`, opened as the application opens one.
pub(super) fn open_desktop(
    viewer: &Engine,
    connection: &SessionInfo,
) -> Result<RemoteDesktopService, Error> {
    let owner = viewer.core.attach_controller_service(
        connection.id,
        connection.generation,
        ServiceKind::Desktop,
    )?;
    viewer.core.open_desktop_service(owner.id, owner.generation)
}

/// The host's Desktop services.
pub(super) fn hosted_desktops(host: &Engine) -> Vec<ServiceInfo> {
    host.core
        .hosted_services()
        .unwrap_or_default()
        .into_iter()
        .filter(|service| service.kind == ServiceKind::Desktop)
        .collect()
}

/// The host's Desktop service for `viewer`.
pub(super) fn hosted_for(host: &Engine, viewer: &Engine) -> ServiceInfo {
    let peer = viewer.endpoint();
    wait_for(
        "the host to serve the viewer's desktop",
        DEFAULT_WAIT,
        || {
            hosted_desktops(host)
                .into_iter()
                .find(|service| service.peer_id == peer)
        },
    )
}

/// The host's Desktop service whose applied selection is `display` alone.
pub(super) fn hosted_showing(host: &Engine, display: u64) -> ServiceInfo {
    wait_for(
        &format!("the host to apply a selection of display {display}"),
        DEFAULT_WAIT,
        || {
            hosted_desktops(host).into_iter().find(|service| {
                host.core
                    .hosted_display_selection(service.id, service.generation)
                    .ok()
                    .as_deref()
                    == Some(&[display][..])
            })
        },
    )
}

/// The next input failure the host reports, by its message.
pub(super) fn input_failure(events: &EventSubscription) -> &'static str {
    wait_for(
        "the host to report an input failure",
        DEFAULT_WAIT,
        || match events.recv_timeout(Duration::from_millis(20))? {
            CoreEvent::InputFailed { message, .. } => Some(message),
            _ => None,
        },
    )
}

/// A keyframe of `payload`, `len` bytes of it.
pub(super) fn picture(timestamp: u64, payload: u8, len: usize) -> EncodedFrame {
    EncodedFrame::h264(true, timestamp, vec![payload; len]).expect("an H.264 keyframe")
}

/// The next picture `video` receives.
pub(super) fn received(video: &VideoSubscription, what: &str) -> EncodedFrame {
    video
        .recv_timeout(Duration::from_secs(5))
        .unwrap_or_else(|| panic!("{what} did not arrive"))
}

/// How often a producer publishes while a test waits for the viewer to
/// learn something: a picture every 50 ms, as a 20 fps stream.
const FRAME_INTERVAL: Duration = Duration::from_millis(50);

/// Says when the next picture is due, so a producer stands in for a
/// stream's clock without sleeping.
struct Paced {
    last: Option<Instant>,
}

impl Paced {
    fn new() -> Self {
        Self { last: None }
    }

    fn due(&mut self) -> bool {
        let due = self
            .last
            .is_none_or(|last| last.elapsed() >= FRAME_INTERVAL);
        if due {
            self.last = Some(Instant::now());
        }
        due
    }
}

/// A physical capture that starts serves the desktops open when it began:
/// it lasts while one of them is open, is not adopted by a desktop opened
/// after they all closed, and ends with its engine.
#[test]
fn capture_startup_serves_only_the_desktops_open_when_it_began() -> TestResult {
    let trace = traced();
    let timeout = Duration::from_secs(5);
    let pair = Pair::for_desktop();
    let host = &pair.host.core;
    host.set_capture(Box::new(Capture(SCREEN)))?;
    let stop = Arc::new(AtomicBool::new(false));
    let alone = host.desktop_capture_startup(0, Arc::clone(&stop), timeout);
    assert!(
        matches!(alone, Err(Error::NoSession)),
        "no capture starts with no desktop to serve: {:?}",
        alone.err()
    );
    let second = another_viewer(&pair, "second")?;
    let viewers = [&pair.controller, &second];
    let mut connections = Vec::new();
    let mut desktops = Vec::new();
    for viewer in viewers {
        let connection = connect(viewer, &pair.host)?;
        desktops.push(open_desktop(viewer, &connection)?);
        connections.push(connection);
    }
    wait_for("the host to serve both desktops", DEFAULT_WAIT, || {
        (host.video_targets().ok()?.len() == 2).then_some(())
    });
    let startup = host.desktop_capture_startup(0, Arc::clone(&stop), timeout)?;

    desktops[0].close();
    wait_for("the host to serve one desktop", DEFAULT_WAIT, || {
        (host.video_targets().ok()?.len() == 1).then_some(())
    });
    assert!(
        startup.check().is_ok(),
        "one departing desktop leaves the capture its sibling waits for"
    );
    desktops[1].close();
    wait_for("the host to serve no desktop", DEFAULT_WAIT, || {
        host.video_targets().ok()?.is_empty().then_some(())
    });
    assert!(
        matches!(startup.check(), Err(Error::NoSession)),
        "a capture whose desktops all closed is cancelled"
    );

    let replacement = open_desktop(&pair.controller, &connections[0])?;
    assert!(
        matches!(startup.check(), Err(Error::NoSession)),
        "a desktop opened later does not adopt a cancelled capture"
    );
    let fresh = host.desktop_capture_startup(0, Arc::clone(&stop), timeout)?;
    assert!(
        fresh.check().is_ok(),
        "the new desktop starts its own capture"
    );
    host.shutdown();
    assert!(
        matches!(fresh.check(), Err(Error::NoSession)),
        "a capture ends with its engine"
    );
    drop(replacement);
    // The viewers learn of the shutdown at moments of their own: whether
    // the replacement suspends before it is dropped is theirs to race.
    expect![[r#"
        host:
          service: service phase from=active generation=1 kind=Desktop service=1 to=closed: PeerClose
          service: service phase from=active generation=2 kind=Desktop service=2 to=closed: PeerClose
          service: service phase from=active generation=1 kind=Desktop service=3 to=closed: Shutdown
    "#]].assert_eq(&trace.engine_story("host", &["desktop", "capture", "service"]));
    Ok(())
}

/// A viewer whose pictures wait unread adapts its own frame rate to that
/// pressure, alone: its sibling on another computer keeps streaming at the
/// quality it had and learns the host's and its decoder's timings, which
/// stay its own; once the slow viewer closes, the sibling still streams.
#[test]
fn receiver_pressure_adapts_only_its_owner_and_the_sibling_keeps_streaming() -> TestResult {
    let trace = traced();
    let pair = Pair::for_desktop();
    let (desktop, _) = Applied::new();
    pair.host.core.set_desktop(Box::new(desktop))?;
    let second = another_viewer(&pair, "second")?;
    let slow = open_desktop(&pair.controller, &connect(&pair.controller, &pair.host)?)?;
    let slow_host = hosted_for(&pair.host, &pair.controller);
    let fast = open_desktop(&second, &connect(&second, &pair.host)?)?;
    let fast_host = hosted_for(&pair.host, &second);
    let _slow_video = slow.subscribe_video()?;
    let fast_video = fast.subscribe_video()?;
    let initial = slow.desktop_info()?.quality;
    let host = &pair.host.core;
    // One picture the slow viewer never reads.
    host.publish_desktop_video(slow_host.id, slow_host.generation, picture(1, 31, 1000))?;

    let budget = Duration::from_secs(8);
    let started = Instant::now();
    let mut paced = Paced::new();
    let mut timestamp = 2;
    let mut stream_fast = |payload: u8| {
        if !paced.due() {
            return;
        }
        host.report_desktop_producer_duration(fast_host.id, fast_host.generation, 17, 29)
            .expect("the host reports its producer's timings");
        fast.report_decode_duration(1000)
            .expect("the window reports its decode time");
        host.publish_desktop_video(
            fast_host.id,
            fast_host.generation,
            picture(timestamp, payload, 1000),
        )
        .expect("the host publishes to the fast viewer");
        assert_eq!(
            received(&fast_video, "the fast viewer's picture").payload,
            vec![payload; 1000],
            "the fast viewer receives each picture published to it"
        );
        timestamp += 1;
    };
    wait_for("the slow viewer to adapt to its pressure", budget, || {
        stream_fast(42);
        (slow.desktop_info().ok()?.quality.max_frame_rate != initial.max_frame_rate).then_some(())
    });
    let changed = slow.desktop_info()?.quality;
    assert!(
        changed.max_frame_rate < initial.max_frame_rate,
        "pressure lowers the slow viewer's frame rate: {changed:?}"
    );
    assert_eq!(
        (
            changed.max_width,
            changed.max_height,
            changed.max_bitrate_kbps
        ),
        (
            initial.max_width,
            initial.max_height,
            initial.max_bitrate_kbps
        ),
        "pressure changes the frame rate alone"
    );
    let slowed = slow.media_status()?;
    assert_eq!(
        slowed.reason,
        AdaptationReason::ReceiverPressure,
        "the slow viewer says why it adapted"
    );
    assert!(
        slowed.receiver_queue_ms >= 150,
        "the slow viewer's queue is what pressed: {} ms",
        slowed.receiver_queue_ms
    );
    // Host feedback and receiver sampling have independent one-second
    // ticks: the sibling learns the timings at its own next sample.
    let learned = wait_for(
        "the fast viewer to learn the timings",
        budget.saturating_sub(started.elapsed()),
        || {
            stream_fast(42);
            let status = fast.media_status().ok()?;
            ((status.encode_us, status.capture_us, status.decode_us) == (29, 17, 1000))
                .then_some(status)
        },
    );
    assert!(
        learned.received_fps > 0,
        "the fast viewer measures what it receives"
    );
    assert_eq!(
        fast.desktop_info()?.quality,
        initial,
        "the sibling keeps the quality it had"
    );
    assert_eq!(
        slow.media_status()?.encode_us,
        0,
        "one owner's producer timings are not another's"
    );
    assert!(
        fast.report_decode_duration(5_000_001).is_err(),
        "a decode time beyond five seconds is refused"
    );
    slow.close();
    // The handle closes at once, before the host hears of it.
    assert!(
        slow.media_status().is_err(),
        "a closed desktop has no status"
    );
    assert!(
        slow.report_decode_duration(1000).is_err(),
        "a closed desktop takes no reports"
    );
    wait_for("the host to let the slow viewer go", DEFAULT_WAIT, || {
        (hosted_desktops(&pair.host).len() == 1).then_some(())
    });
    host.publish_desktop_video(
        fast_host.id,
        fast_host.generation,
        picture(timestamp, 77, 1000),
    )?;
    assert_eq!(
        received(&fast_video, "the sibling's picture after the close").payload,
        vec![77; 1000],
        "the sibling streams after the slow viewer closed"
    );
    expect![[r#"
        controller:
          service: service phase from=active generation=1 kind=Desktop service=1 to=closed: OwnerClose
        host:
          service: service phase from=active generation=1 kind=Desktop service=1 to=closed: PeerClose
    "#]].assert_eq(&trace.story(&["desktop", "service"]));
    Ok(())
}

/// Two Desktop owners on one connection showing twin displays, alike in
/// all but their ids, each receive their own display's pictures with its
/// exact stamp; an unstamped picture gains the live revision and no
/// invented display. Pointers name their display exactly, closing one owner
/// spares the other, and switching the survivor's display re-sources it.
#[test]
fn twin_sources_stream_independently_with_exact_stamps() -> TestResult {
    let trace = traced();
    let pair = Pair::for_desktop();
    let host = &pair.host.core;
    let (desktop, applied_events) = Applied::new();
    host.set_desktop(Box::new(desktop))?;
    // Twins: identical geometry, distinct ids.
    host.set_topology(5, &[display(1, 0, 1920, 5), display(2, 1920, 1920, 5)])?;
    let connection = pair.connect_services();
    let desktop_a = open_desktop(&pair.controller, &connection)?;
    let desktop_b = open_desktop(&pair.controller, &connection)?;
    wait_for("both owners to learn the twins", DEFAULT_WAIT, || {
        (desktop_a.topology().ok()?.len() == 2 && desktop_b.topology().ok()?.len() == 2)
            .then_some(())
    });
    desktop_a.select_displays(&[1])?;
    desktop_b.select_displays(&[2])?;
    let remote_a = hosted_showing(&pair.host, 1);
    let remote_b = hosted_showing(&pair.host, 2);
    let mut targets: Vec<_> = host
        .video_targets()?
        .into_iter()
        .filter(|target| target.service)
        .map(|target| (target.display_id, target.revision))
        .collect();
    targets.sort_unstable();
    assert_eq!(
        targets,
        vec![(1, 5), (2, 5)],
        "each owner's stream names its display and revision"
    );
    // Contended input is discarded before acceptance: acquire explicitly.
    desktop_a.acquire_control()?;
    wait_for("the first owner to control", DEFAULT_WAIT, || {
        (desktop_a.control_state().ok()? == DesktopControlState::Controlling).then_some(())
    });

    let video_a = desktop_a.subscribe_video()?;
    let video_b = desktop_b.subscribe_video()?;
    host.publish_desktop_video(
        remote_a.id,
        remote_a.generation,
        picture(1, 31, 1000).with_source(1, 5),
    )?;
    host.publish_desktop_video(
        remote_b.id,
        remote_b.generation,
        picture(1, 42, 1000).with_source(2, 5),
    )?;
    let got_a = received(&video_a, "the first twin's picture");
    assert_eq!(
        (got_a.payload, got_a.display_id, got_a.revision),
        (vec![31; 1000], 1, 5),
        "the first owner receives its twin's picture, stamped exactly"
    );
    let got_b = received(&video_b, "the second twin's picture");
    assert_eq!(
        (got_b.payload, got_b.display_id, got_b.revision),
        (vec![42; 1000], 2, 5),
        "the second owner receives its twin's picture, stamped exactly"
    );
    host.publish_desktop_video(remote_a.id, remote_a.generation, picture(2, 33, 100))?;
    let unstamped = received(&video_a, "the unstamped picture");
    assert_eq!(
        (unstamped.display_id, unstamped.revision),
        (0, 5),
        "an unstamped picture gains the live revision, never an invented display"
    );

    let on_first = InputEvent::pointer(100, 100, 1, 5);
    desktop_a.submit_input(on_first)?;
    wait_for("the pointer on display 1 to apply", DEFAULT_WAIT, || {
        (!applied(&applied_events).is_empty()).then_some(())
    });
    assert_eq!(
        applied(&applied_events),
        [on_first],
        "a pointer on the owner's twin applies"
    );
    let events = host.subscribe()?;
    desktop_a.submit_input(InputEvent::pointer(2000, 100, 2, 5))?;
    assert_eq!(
        input_failure(&events),
        "that display is not selected",
        "a pointer on the other twin is refused for this owner"
    );
    assert_eq!(
        applied(&applied_events).len(),
        1,
        "the refused pointer did not apply"
    );

    desktop_a.close();
    wait_for("the host to drop the first owner", DEFAULT_WAIT, || {
        (hosted_desktops(&pair.host).len() == 1).then_some(())
    });
    assert!(
        host.publish_desktop_video(remote_a.id, remote_a.generation, picture(3, 31, 100))
            .is_err(),
        "a closed owner takes no pictures"
    );
    host.publish_desktop_video(
        remote_b.id,
        remote_b.generation,
        picture(3, 43, 500).with_source(2, 5),
    )?;
    let survivor = received(&video_b, "the survivor's picture");
    assert_eq!(
        (survivor.payload, survivor.display_id, survivor.revision),
        (vec![43; 500], 2, 5),
        "closing one owner spares the other's stamped stream"
    );
    desktop_b.select_displays(&[1])?;
    wait_for("the survivor to switch to display 1", DEFAULT_WAIT, || {
        (host
            .hosted_display_selection(remote_b.id, remote_b.generation)
            .ok()?
            == [1])
        .then_some(())
    });
    host.publish_desktop_video(
        remote_b.id,
        remote_b.generation,
        picture(4, 44, 500).with_source(1, 5),
    )?;
    let switched = received(&video_b, "the switched picture");
    assert_eq!(
        (switched.payload, switched.display_id, switched.revision),
        (vec![44; 500], 1, 5),
        "the survivor's stream is re-sourced to display 1"
    );
    assert_eq!(
        host.capture_displays()?,
        vec![1],
        "the host captures only the display still shown"
    );
    expect![[r#"
        host:
          input: control asked for epoch=1 service=1 state=Controlling
          warn input: input failed reason=that display is not selected session=1
    "#]]
    .assert_eq(&trace.story(&["desktop", "topology", "input"]));
    Ok(())
}

/// What the host says of itself, and what this window's decoder reports,
/// become the viewer's conditions, with their evidence; a clean loopback
/// adds no link strain, and a decoder that restarted shows as recovering.
#[test]
fn host_and_decoder_reports_become_the_viewers_conditions() -> TestResult {
    let trace = traced();
    let pair = Pair::for_desktop();
    let host = &pair.host.core;
    let (desktop, _) = Applied::new();
    host.set_desktop(Box::new(desktop))?;
    let connection = pair.connect_services();
    let viewer = open_desktop(&pair.controller, &connection)?;
    let remote = hosted_for(&pair.host, &pair.controller);
    let video = viewer.subscribe_video()?;
    host.report_desktop_encoder(
        remote.id,
        remote.generation,
        EncoderImplementation {
            name: "test",
            hardware: Some(false),
            input: None,
        },
    )?;
    host.report_secure_desktop(true)?;
    viewer.report_decoder(true, 0, 0)?;

    // Conditions arrive with feedback, which needs pictures to flow.
    let mut paced = Paced::new();
    let mut timestamp = 1;
    let mut stream_until = |what: &str, done: &dyn Fn(&[Condition]) -> bool| {
        wait_for(what, DEFAULT_WAIT, || {
            if paced.due() {
                host.publish_desktop_video(
                    remote.id,
                    remote.generation,
                    picture(timestamp, 9, 500),
                )
                .expect("the host publishes a picture");
                received(&video, "the viewer's picture");
                timestamp += 1;
            }
            let set: Vec<_> = viewer.media_status().ok()?.conditions.iter().collect();
            done(&set).then_some(())
        });
    };
    stream_until("the host's reports to reach the viewer", &|set| {
        set.contains(&Condition::EncoderSoftware) && set.contains(&Condition::HostSecureDesktop)
    });
    let set: Vec<_> = viewer.media_status()?.conditions.iter().collect();
    assert_eq!(
        set,
        [
            Condition::HostSecureDesktop,
            Condition::EncoderSoftware,
            Condition::DecoderSoftware
        ],
        "the viewer's conditions are the reports, most severe first, with no link strain"
    );
    let signals: Vec<_> = viewer
        .conditions()?
        .iter()
        .map(|entry| entry.evidence.signal)
        .collect();
    assert_eq!(
        signals,
        [
            "host_secure_desktop",
            "host_encoder_hardware",
            "decoder_hardware"
        ],
        "each condition names the signal it rests on"
    );
    host.report_secure_desktop(false)?;
    viewer.report_decoder(false, 0, 0)?;
    stream_until("the cleared reports to reach the viewer", &|set| {
        set == [Condition::EncoderSoftware]
    });
    // The window's decoder restarted twice: it shows as recovering.
    viewer.report_decoder(false, 2, 1)?;
    stream_until("the decoder's restarts to reach the viewer", &|set| {
        set.contains(&Condition::DecoderRecovering)
    });
    assert!(
        viewer.report_decoder(false, 1, 1).is_err(),
        "a restart count that goes back belongs to another decoder"
    );
    viewer.close();
    expect![[r#"
        host:
          stream: encoder opened encoder=EncoderImplementation { name: "test", hardware: Some(false), input: None } generation=1 service=1
    "#]].assert_eq(&trace.story(&["stream"]));
    Ok(())
}

/// How a picture ends in [`picture_ended_by`].
#[derive(Clone, Copy, Debug)]
enum Ending {
    HostCloses,
    HostFails(StreamFailure),
    HostStopsSharing,
    ViewerCloses,
}

/// Opens a picture, ends it by `ending`, and says what end its viewer
/// learned.
fn picture_ended_by(pair: &Pair, ending: Ending) -> Result<Option<DesktopEnd>, Error> {
    let host = &pair.host.core;
    let (desktop, _) = Applied::new();
    host.set_desktop(Box::new(desktop))?;
    let connection = pair.connect_services();
    let viewer = open_desktop(&pair.controller, &connection)?;
    let remote = hosted_for(&pair.host, &pair.controller);
    assert_eq!(viewer.end(), None, "a live picture has no end");
    match ending {
        Ending::HostCloses => host.close_hosted_service(remote.id, remote.generation)?,
        Ending::HostFails(failure) => {
            host.fail_hosted_service(remote.id, remote.generation, failure)?
        }
        Ending::HostStopsSharing => host.set_service_sharing(ServiceKind::Desktop, false)?,
        Ending::ViewerCloses => viewer.close(),
    }
    wait_for("the viewer's picture to end", DEFAULT_WAIT, || {
        viewer.is_cancelled().then_some(())
    });
    match ending {
        // The host has let the service go, and an end it sent by mistake
        // has the original's 300 ms to reach the viewer.
        Ending::ViewerCloses => {
            wait_for("the host to let the picture go", DEFAULT_WAIT, || {
                hosted_desktops(&pair.host).is_empty().then_some(())
            });
            holds(
                "the viewer's closed picture having no end",
                Duration::from_millis(300),
                || viewer.end().is_none(),
            );
        }
        _ => wait_for("the viewer to learn why", DEFAULT_WAIT, || {
            viewer.end().is_some().then_some(())
        }),
    }
    Ok(viewer.end())
}

/// A host that closes the picture tells its viewer it stopped.
#[test]
fn the_viewer_learns_the_host_stopped_its_picture() -> TestResult {
    let trace = traced();
    let pair = Pair::for_desktop();
    assert_eq!(
        picture_ended_by(&pair, Ending::HostCloses)?,
        Some(DesktopEnd::Host(StreamEnd::Stopped)),
        "a host's close reaches the viewer as stopped"
    );
    expect![[r#"
        controller:
          service: service phase from=active generation=1 kind=Desktop service=1 to=closed: PeerClose
        host:
          service: service phase from=active generation=1 kind=Desktop service=1 to=closed: OwnerClose
    "#]].assert_eq(&trace.story(&["service"]));
    Ok(())
}

/// A host whose encoder failed tells its viewer so.
#[test]
fn the_viewer_learns_the_hosts_encoder_failed() -> TestResult {
    let trace = traced();
    let pair = Pair::for_desktop();
    assert_eq!(
        picture_ended_by(&pair, Ending::HostFails(StreamFailure::Encoder))?,
        Some(DesktopEnd::Host(StreamEnd::Failed(StreamFailure::Encoder))),
        "an encoder failure reaches the viewer as itself"
    );
    expect![[r#"
        controller:
          service: service phase from=active generation=1 kind=Desktop service=1 to=closed: PeerClose
        host:
          service: service phase from=active generation=1 kind=Desktop service=1 to=closed: Failed(Encoder)
    "#]].assert_eq(&trace.story(&["service"]));
    Ok(())
}

/// A host refused permission to capture tells its viewer so.
#[test]
fn the_viewer_learns_the_host_lacks_permission() -> TestResult {
    let trace = traced();
    let pair = Pair::for_desktop();
    assert_eq!(
        picture_ended_by(&pair, Ending::HostFails(StreamFailure::Permission))?,
        Some(DesktopEnd::Host(StreamEnd::Failed(
            StreamFailure::Permission
        ))),
        "a permission failure reaches the viewer as itself"
    );
    expect![[r#"
        controller:
          service: service phase from=active generation=1 kind=Desktop service=1 to=closed: PeerClose
        host:
          service: service phase from=active generation=1 kind=Desktop service=1 to=closed: Failed(Permission)
    "#]].assert_eq(&trace.story(&["service"]));
    Ok(())
}

/// A host that stops sharing its desktop tells its viewer it was revoked.
#[test]
fn the_viewer_learns_the_host_stopped_sharing() -> TestResult {
    let trace = traced();
    let pair = Pair::for_desktop();
    assert_eq!(
        picture_ended_by(&pair, Ending::HostStopsSharing)?,
        Some(DesktopEnd::Host(StreamEnd::Revoked)),
        "a withdrawn share reaches the viewer as revoked"
    );
    expect![[r#"
        controller:
          service: service phase from=active generation=1 kind=Desktop service=1 to=closed: PeerClose
        host:
          service: service phase from=active generation=1 kind=Desktop service=1 to=closed: Revoked
    "#]].assert_eq(&trace.story(&["service"]));
    Ok(())
}

/// A picture this computer closed has no end from the host.
#[test]
fn a_picture_the_viewer_closed_has_no_host_end() -> TestResult {
    let trace = traced();
    let pair = Pair::for_desktop();
    assert_eq!(
        picture_ended_by(&pair, Ending::ViewerCloses)?,
        None,
        "the viewer's own close carries no reason from the host"
    );
    expect![[r#"
        controller:
          service: service phase from=active generation=1 kind=Desktop service=1 to=closed: OwnerClose
        host:
          service: service phase from=active generation=1 kind=Desktop service=1 to=closed: PeerClose
    "#]].assert_eq(&trace.story(&["service", "session"]));
    Ok(())
}

/// Pictures a host publishes to a viewer's session reach it in order,
/// keyframe and delta as published, a burst of them too, and the input the
/// viewer sends meanwhile still applies. What the host advertises follows
/// its adapters: a capture alone offers no input, a desktop adds it. Empty
/// pictures and pictures for no session are refused, as are pictures once
/// the viewer left.
#[test]
fn encoded_frames_reach_the_controller_without_blocking_input() -> TestResult {
    let trace = traced();
    let pair = Pair::for_desktop();
    let (host, controller) = (&pair.host.core, &pair.controller.core);
    // The kit's engines share a folder; this host offers only its screen.
    host.set_file_roots(Vec::new())?;
    host.set_capture(Box::new(Capture((1280, 720))))?;
    host.set_video_codecs(VideoCodecCaps::encode_only(4096, 4096, "testkit"))?;
    let capture_only = host.capabilities();
    assert!(
        capture_only.desktop_capture
            && !capture_only.keyboard
            && !capture_only.pointer
            && !capture_only.clipboard
            && !capture_only.file_transfer
            && !capture_only.audio
            && capture_only.monitors == 1,
        "a capture alone offers one monitor and nothing else: {capture_only:?}"
    );
    let (desktop, applied_events) = Applied::sized((100, 100));
    host.set_desktop(Box::new(desktop))?;
    let with_desktop = host.capabilities();
    assert!(
        with_desktop.desktop_capture
            && with_desktop.keyboard
            && with_desktop.pointer
            && with_desktop.monitors == 1,
        "a desktop adds keyboard and pointer: {with_desktop:?}"
    );

    let video = controller.subscribe_video()?;
    let session = controller.connect(&pair.host.endpoint())?;
    assert_eq!(session.phase, SessionPhase::Active, "the session opens");
    assert_eq!(
        (session.monitor_width, session.monitor_height),
        (1280, 720),
        "the viewer learns the capture's size, not the desktop's"
    );
    let incoming = host.snapshot()?.incoming.ok_or("a hosted session")?;
    assert_eq!(
        (incoming.monitor_width, incoming.monitor_height),
        (1280, 720),
        "the host serves the capture's size"
    );
    assert!(
        matches!(
            host.set_capture(Box::new(Capture((1, 1)))),
            Err(Error::Busy)
        ),
        "the capture is not swapped during a session"
    );

    host.publish_video(session.id, EncodedFrame::h264(true, 1, vec![1])?)?;
    host.publish_video(session.id, EncodedFrame::h264(false, 2, vec![2])?)?;
    let first = received(&video, "the keyframe");
    assert_eq!(
        (first.keyframe, first.timestamp_us, first.payload),
        (true, 1, vec![1]),
        "the keyframe arrives first, as published"
    );
    let second = received(&video, "the delta");
    assert_eq!(
        (second.keyframe, second.timestamp_us, second.payload),
        (false, 2, vec![2]),
        "the delta follows, as published"
    );
    for index in 0..30u64 {
        host.publish_video(session.id, EncodedFrame::h264(false, 100 + index, vec![3])?)?;
    }
    host.publish_video(session.id, EncodedFrame::h264(true, 999, vec![9, 9])?)?;
    // The sender may drain the burst or drop part of it; the core's
    // bounded-queue tests own overflow. The burst's keyframe arrives last.
    let last = wait_for("the burst's keyframe", Duration::from_secs(5), || {
        video
            .recv_timeout(Duration::from_millis(20))
            .filter(|frame| frame.timestamp_us == 999)
    });
    assert_eq!(
        (last.keyframe, last.payload),
        (true, vec![9, 9]),
        "the burst's keyframe arrives as published"
    );
    assert!(
        video.try_recv().is_none(),
        "nothing follows the last picture"
    );

    let key = InputEvent::key_down(vnilink::usage::A);
    controller.submit_input(session.id, session.generation, key)?;
    wait_for("the key to apply", Duration::from_secs(2), || {
        (!applied(&applied_events).is_empty()).then_some(())
    });
    assert_eq!(
        applied(&applied_events),
        [key],
        "input applies while pictures stream"
    );

    let mut empty = EncodedFrame::h264(true, 1, vec![1])?;
    empty.payload.clear();
    assert!(
        matches!(
            host.publish_video(session.id, empty),
            Err(Error::InvalidSetting("encoded frame is empty"))
        ),
        "an empty picture is refused"
    );
    assert!(
        matches!(
            host.publish_video(session.id + 1, EncodedFrame::h264(true, 1, vec![1])?),
            Err(Error::NoSession)
        ),
        "a picture for no session is refused"
    );
    controller.disconnect()?;
    wait_for("the host's session to close", DEFAULT_WAIT, || {
        host.snapshot()
            .ok()?
            .incoming
            .is_some_and(|session| session.phase == SessionPhase::Closed)
            .then_some(())
    });
    assert!(
        matches!(
            host.publish_video(session.id, EncodedFrame::h264(true, 1, vec![1])?),
            Err(Error::NoSession)
        ),
        "a picture after the viewer left is refused"
    );
    expect![[r#"
        host:
          input: the session's control stream ended error=network session=1
    "#]]
    .assert_eq(&trace.story(&["session", "input"]));
    Ok(())
}

/// A capture that reports no size leaves the desktop's size to advertise
/// and serve.
#[test]
fn a_zero_capture_size_falls_back_to_the_desktop() -> TestResult {
    let _trace = traced();
    let pair = Pair::for_desktop();
    let host = &pair.host.core;
    host.set_capture(Box::new(Capture((0, 0))))?;
    host.set_video_codecs(VideoCodecCaps::encode_only(4096, 4096, "testkit"))?;
    let (desktop, _) = Applied::sized((100, 80));
    host.set_desktop(Box::new(desktop))?;
    let capabilities = host.capabilities();
    assert!(
        capabilities.desktop_capture && capabilities.keyboard && capabilities.monitors == 1,
        "the host still offers its one monitor: {capabilities:?}"
    );
    let session = pair.controller.core.connect(&pair.host.endpoint())?;
    assert_eq!(
        (session.monitor_width, session.monitor_height),
        (100, 80),
        "the viewer learns the desktop's size"
    );
    Ok(())
}
