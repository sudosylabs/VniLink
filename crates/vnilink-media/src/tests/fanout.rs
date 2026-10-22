//! Shared capture: one reader, independent encoders.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use vnilink::{
    Core, CoreConfig, CustomRelay, Grants, Listen, RelaySettings, ServiceKind, SessionPhase,
};

use crate::error::Error;
use crate::frame::RawFrame;
use crate::pump::{EncodeParams, EncodedSample, VideoEncoder};
use crate::{parse_annex_b, FrameSource};

fn solid(width: u16, height: u16, timestamp_us: u64) -> RawFrame {
    RawFrame::bgra(
        width,
        height,
        timestamp_us,
        vec![0; usize::from(width) * usize::from(height) * 4],
    )
    .unwrap()
}

struct Source {
    idle: bool,
    frames: Arc<AtomicUsize>,
    thread: Mutex<Option<std::thread::ThreadId>>,
    wrong_thread: Arc<AtomicBool>,
}

impl FrameSource for Source {
    fn size(&self) -> (u16, u16) {
        (64, 64)
    }

    fn poll_frame(&self, timestamp: u64) -> Result<Option<RawFrame>, Error> {
        let current = std::thread::current().id();
        let mut owner = self.thread.lock().unwrap();
        match *owner {
            None => *owner = Some(current),
            Some(owner) if owner != current => {
                self.wrong_thread.store(true, Ordering::Relaxed);
            }
            Some(_) => {}
        }
        drop(owner);
        self.frames.fetch_add(1, Ordering::Relaxed);
        Ok((!self.idle).then(|| solid(64, 64, timestamp)))
    }
}

impl vnilink::HostCapture for Source {
    fn monitor(&self) -> (u16, u16) {
        (64, 64)
    }
}

struct Encoder {
    slow: bool,
    encodes: Arc<AtomicUsize>,
    opens: Arc<AtomicUsize>,
}

impl VideoEncoder for Encoder {
    fn encode(&mut self, frame: &RawFrame, params: &EncodeParams) -> Result<EncodedSample, Error> {
        // Far slower than a shared CI runner's capture loop, so a fast viewer
        // held behind it would show as a count near its own.
        if self.slow {
            std::thread::sleep(Duration::from_millis(400));
        }
        self.encodes.fetch_add(1, Ordering::Relaxed);
        if frame.width() != 64 || frame.height() != 64 {
            return Err(Error::InvalidFrame(
                "the frame size does not match the encoder",
            ));
        }
        sample(params.force_keyframe)
    }
}

impl Drop for Encoder {
    fn drop(&mut self) {
        self.opens.fetch_sub(1, Ordering::Relaxed);
    }
}

fn sample(keyframe: bool) -> Result<EncodedSample, Error> {
    use crate::annexb::{to_annex_b, AvccAccessUnit};
    let picture: &[u8] = if keyframe {
        &[0x65, 0x10]
    } else {
        &[0x41, 0x10]
    };
    let mut avcc = (picture.len() as u32).to_be_bytes().to_vec();
    avcc.extend_from_slice(picture);
    // A real SPS: hosts give every SPS the stream's VUI.
    let sets = [
        vec![0x67, 0x42, 0x00, 0x1e, 0xda, 0x10, 0x99],
        vec![0x68, 0xCE],
    ];
    let annex = to_annex_b(&AvccAccessUnit {
        nal_length_size: 4,
        avcc: &avcc,
        parameter_sets: &sets,
        keyframe,
    })?;
    EncodedSample::annex_b(keyframe, annex)
}

fn boot() -> Core {
    Core::start(CoreConfig {
        listen: Listen::Loopback,
        relay: RelaySettings::custom(vec![CustomRelay::new("http://127.0.0.1:9").unwrap()])
            .unwrap(),
        ..CoreConfig::default()
    })
    .unwrap()
}

fn pair(host: &Core, controller: &Core) {
    controller
        .accept_invitation(&host.create_invitation(Duration::from_secs(60)).unwrap())
        .unwrap();
    host.accept_invitation(
        &controller
            .create_invitation(Duration::from_secs(60))
            .unwrap(),
    )
    .unwrap();
    host.set_desktop_sharing(Grants {
        view: true,
        control: true,
        clipboard: false,
        transfer: false,
    })
    .unwrap();
}

fn wait_active(controller: &Core, id: u64) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while controller.controller_session(id).unwrap().phase != SessionPhase::Active {
        assert!(Instant::now() < deadline, "connection was not admitted");
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn a_slow_viewer_does_not_stall_or_queue_its_sibling() {
    shared_capture_workload(false);
}

#[test]
fn idle_shared_capture_releases_one_owner_without_stopping_its_sibling() {
    shared_capture_workload(true);
}

fn shared_capture_workload(idle: bool) {
    let host = Arc::new(boot());
    let fast = boot();
    let slow = boot();
    host.set_capture(Box::new(Source {
        idle,
        frames: Arc::new(AtomicUsize::new(0)),
        thread: Mutex::new(None),
        wrong_thread: Arc::new(AtomicBool::new(false)),
    }))
    .unwrap();
    for core in [&host, &fast, &slow] {
        core.set_video_codecs(vnilink::VideoCodecCaps::h264_full(4096, 4096, "test"))
            .unwrap();
    }
    pair(&host, &fast);
    pair(&host, &slow);
    host.start_hosting().unwrap();
    let fast_parent = fast.begin_connect_services(&host.endpoint_id()).unwrap();
    let slow_parent = slow.begin_connect_services(&host.endpoint_id()).unwrap();
    wait_active(&fast, fast_parent.id);
    wait_active(&slow, slow_parent.id);
    let fast_parent = fast.controller_session(fast_parent.id).unwrap();
    let slow_parent = slow.controller_session(slow_parent.id).unwrap();
    let fast_owner = fast
        .attach_controller_service(fast_parent.id, fast_parent.generation, ServiceKind::Desktop)
        .unwrap();
    let slow_owner = slow
        .attach_controller_service(slow_parent.id, slow_parent.generation, ServiceKind::Desktop)
        .unwrap();
    let fast_handle = fast
        .open_desktop_service(fast_owner.id, fast_owner.generation)
        .unwrap();
    let slow_handle = slow
        .open_desktop_service(slow_owner.id, slow_owner.generation)
        .unwrap();
    let targets = host.video_targets().unwrap();
    assert_eq!(targets.len(), 2);
    let slow_target = targets.iter().map(|target| target.id).max().unwrap();
    let fast_frames = fast_handle.subscribe_video().unwrap();
    let slow_frames = slow_handle.subscribe_video().unwrap();

    let frames = Arc::new(AtomicUsize::new(0));
    let wrong_thread = Arc::new(AtomicBool::new(false));
    let fast_encodes = Arc::new(AtomicUsize::new(0));
    let slow_encodes = Arc::new(AtomicUsize::new(0));
    let live_encoders = Arc::new(AtomicUsize::new(0));
    let stop = Arc::new(AtomicBool::new(false));
    let worker_host = Arc::clone(&host);
    let worker_stop = Arc::clone(&stop);
    let worker_frames = Arc::clone(&frames);
    let worker_wrong = Arc::clone(&wrong_thread);
    let worker_fast = Arc::clone(&fast_encodes);
    let worker_slow = Arc::clone(&slow_encodes);
    let worker_live = Arc::clone(&live_encoders);
    let (done_tx, done_rx) = std::sync::mpsc::channel();
    let worker = std::thread::spawn(move || {
        let result = crate::serve_shared_capture(
            &Source {
                idle,
                frames: worker_frames,
                thread: Mutex::new(None),
                wrong_thread: worker_wrong,
            },
            &worker_host,
            &worker_stop,
            false,
            0,
            |_| {},
            move |target, width, height, fps, bps| {
                if width != 64 || height != 64 || fps == 0 || bps == 0 {
                    return Err(Error::InvalidSetting("encoder parameters are invalid"));
                }
                worker_live.fetch_add(1, Ordering::Relaxed);
                Ok(Encoder {
                    slow: target.id == slow_target,
                    encodes: if target.id == slow_target {
                        Arc::clone(&worker_slow)
                    } else {
                        Arc::clone(&worker_fast)
                    },
                    opens: Arc::clone(&worker_live),
                })
            },
        );
        let _ = done_tx.send(result);
    });

    if idle {
        let deadline = Instant::now() + Duration::from_secs(5);
        while frames.load(Ordering::Relaxed) < 3 || live_encoders.load(Ordering::Relaxed) != 2 {
            assert!(
                Instant::now() < deadline,
                "idle capture did not start both encoders"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(fast_frames
            .recv_timeout(Duration::from_millis(10))
            .is_none());
        assert!(slow_frames
            .recv_timeout(Duration::from_millis(10))
            .is_none());
        assert_eq!(fast_encodes.load(Ordering::Relaxed), 0);
        assert_eq!(slow_encodes.load(Ordering::Relaxed), 0);
        slow_handle.close();
        let deadline = Instant::now() + Duration::from_secs(5);
        while host.video_targets().unwrap().len() != 1 || live_encoders.load(Ordering::Relaxed) != 1
        {
            assert!(
                Instant::now() < deadline,
                "idle departed encoder was not released"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(
            fast.controller_session(fast_parent.id).unwrap().phase,
            SessionPhase::Active
        );
        assert!(!wrong_thread.load(Ordering::Relaxed));
        stop.store(true, Ordering::Release);
        let result = done_rx.recv_timeout(Duration::from_secs(5));
        worker.join().unwrap();
        result.expect("idle shared capture did not stop").unwrap();
        assert_eq!(live_encoders.load(Ordering::Relaxed), 0);
        return;
    }

    let first = fast_frames
        .recv_timeout(Duration::from_secs(5))
        .expect("fast viewer frame");
    assert!(parse_annex_b(&first.payload).unwrap().has_idr());
    let first_slow = slow_frames
        .recv_timeout(Duration::from_secs(5))
        .expect("slow viewer frame");
    assert!(parse_annex_b(&first_slow.payload).unwrap().has_idr());

    let started = Instant::now();
    while started.elapsed() < Duration::from_millis(1200) {
        std::thread::sleep(Duration::from_millis(30));
    }
    let fast_count = fast_encodes.load(Ordering::Relaxed);
    let slow_count = slow_encodes.load(Ordering::Relaxed);
    let read_count = frames.load(Ordering::Relaxed);
    assert!(read_count >= 8, "reader produced {read_count} frames");
    assert!(slow_count >= 1, "slow encoder did not run");
    assert!(
        fast_count > slow_count * 2,
        "fast {fast_count} encodes, slow {slow_count}"
    );
    assert!(
        read_count > slow_count * 2,
        "reader {read_count} was held behind the slow encoder {slow_count}"
    );
    assert!(!wrong_thread.load(Ordering::Relaxed));

    slow_handle.close();
    let deadline = Instant::now() + Duration::from_secs(5);
    while host.video_targets().unwrap().len() != 1 {
        assert!(Instant::now() < deadline, "slow owner did not leave");
        std::thread::sleep(Duration::from_millis(10));
    }
    let after_close = fast_encodes.load(Ordering::Relaxed);
    let resume = Instant::now();
    while resume.elapsed() < Duration::from_millis(400) {
        std::thread::sleep(Duration::from_millis(30));
    }
    assert!(
        fast_encodes.load(Ordering::Relaxed) > after_close,
        "the remaining viewer stopped when its sibling closed"
    );
    assert_eq!(host.video_targets().unwrap().len(), 1);
    assert_eq!(
        fast.controller_session(fast_parent.id).unwrap().phase,
        SessionPhase::Active
    );

    stop.store(true, Ordering::Relaxed);
    let result = done_rx.recv_timeout(Duration::from_secs(5));
    worker.join().unwrap();
    result.expect("shared capture did not stop").unwrap();
    assert_eq!(live_encoders.load(Ordering::Relaxed), 0);
}

fn twin(id: u64, x: i32) -> vnilink::DisplayInfo {
    vnilink::DisplayInfo {
        id,
        origin_x: x,
        origin_y: 0,
        width: 1920,
        height: 1080,
        rotation_deg: 0,
        scale_percent: 100,
        revision: 9,
    }
}

/// Two same-sized captures must not both serve one unselected viewer: its
/// picture would flap between sources and its pointer stamp with it. The
/// unselected viewer follows the selected default (lowest requested id),
/// each selected viewer its own source, and stopping one capture leaves the
/// sibling's stream untouched.
#[test]
fn twin_captures_serve_selected_and_default_targets_without_flapping() {
    let host = Arc::new(boot());
    let first = boot();
    let second = boot();
    host.set_capture(Box::new(Source {
        idle: false,
        frames: Arc::new(AtomicUsize::new(0)),
        thread: Mutex::new(None),
        wrong_thread: Arc::new(AtomicBool::new(false)),
    }))
    .unwrap();
    for core in [&host, &first, &second] {
        core.set_video_codecs(vnilink::VideoCodecCaps::h264_full(4096, 4096, "test"))
            .unwrap();
    }
    host.set_topology(9, &[twin(1, 0), twin(2, 1920)]).unwrap();
    pair(&host, &first);
    pair(&host, &second);
    host.start_hosting().unwrap();
    let first_parent = first.begin_connect_services(&host.endpoint_id()).unwrap();
    let second_parent = second.begin_connect_services(&host.endpoint_id()).unwrap();
    wait_active(&first, first_parent.id);
    wait_active(&second, second_parent.id);
    let first_parent = first.controller_session(first_parent.id).unwrap();
    let second_parent = second.controller_session(second_parent.id).unwrap();
    let first_owner = first
        .attach_controller_service(
            first_parent.id,
            first_parent.generation,
            ServiceKind::Desktop,
        )
        .unwrap();
    let second_owner = second
        .attach_controller_service(
            second_parent.id,
            second_parent.generation,
            ServiceKind::Desktop,
        )
        .unwrap();
    let first_handle = first
        .open_desktop_service(first_owner.id, first_owner.generation)
        .unwrap();
    let second_handle = second
        .open_desktop_service(second_owner.id, second_owner.generation)
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while first_handle.topology().ok().map(|d| d.len()) != Some(2)
        || second_handle.topology().ok().map(|d| d.len()) != Some(2)
    {
        assert!(Instant::now() < deadline, "topology did not propagate");
        std::thread::sleep(Duration::from_millis(10));
    }
    // One viewer selects display 2; the other stays unselected and must
    // follow that same default rather than flapping between captures.
    first_handle.select_displays(&[2]).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while host.capture_displays().unwrap() != vec![2] {
        assert!(Instant::now() < deadline, "selection did not propagate");
        std::thread::sleep(Duration::from_millis(10));
    }
    let first_frames = first_handle.subscribe_video().unwrap();
    let second_frames = second_handle.subscribe_video().unwrap();

    let opens_left = Arc::new(AtomicUsize::new(0));
    let opens_right = Arc::new(AtomicUsize::new(0));
    let stop_left = Arc::new(AtomicBool::new(false));
    let stop_right = Arc::new(AtomicBool::new(false));
    let serve = |display: u64, stop: Arc<AtomicBool>, opens: Arc<AtomicUsize>, host: Arc<Core>| {
        std::thread::spawn(move || {
            crate::serve_shared_capture(
                &Source {
                    idle: false,
                    frames: Arc::new(AtomicUsize::new(0)),
                    thread: Mutex::new(None),
                    wrong_thread: Arc::new(AtomicBool::new(false)),
                },
                &host,
                &stop,
                false,
                display,
                |_| {},
                move |_target, width, height, fps, bps| {
                    if width != 64 || height != 64 || fps == 0 || bps == 0 {
                        return Err(Error::InvalidSetting("encoder parameters are invalid"));
                    }
                    opens.fetch_add(1, Ordering::Relaxed);
                    Ok(Encoder {
                        slow: false,
                        encodes: Arc::new(AtomicUsize::new(0)),
                        opens: Arc::new(AtomicUsize::new(0)),
                    })
                },
            )
        })
    };
    let left = serve(
        1,
        Arc::clone(&stop_left),
        Arc::clone(&opens_left),
        Arc::clone(&host),
    );
    let right = serve(
        2,
        Arc::clone(&stop_right),
        Arc::clone(&opens_right),
        Arc::clone(&host),
    );

    // Both viewers receive display 2 only; display 1 never opens an encoder.
    for (frames, name) in [&first_frames, &second_frames]
        .into_iter()
        .zip(["first", "second"])
    {
        for _ in 0..3 {
            let frame = frames
                .recv_timeout(Duration::from_secs(5))
                .unwrap_or_else(|| panic!("{name} viewer frame"));
            assert_eq!(frame.display_id, 2, "{name} viewer flapped sources");
            assert_eq!(frame.revision, 9, "{name} viewer lost its revision");
        }
    }
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(
        opens_left.load(Ordering::Relaxed),
        0,
        "unselected capture encoded for the default viewer"
    );
    assert!(
        opens_right.load(Ordering::Relaxed) >= 2,
        "default capture did not serve both viewers"
    );

    // The unselected viewer selects display 1 and moves to its own source.
    second_handle.select_displays(&[1]).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while host.capture_displays().unwrap() != vec![1, 2] {
        assert!(
            Instant::now() < deadline,
            "second selection did not propagate"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    let deadline = Instant::now() + Duration::from_secs(5);
    let moved = loop {
        let frame = second_frames
            .recv_timeout(Duration::from_secs(5))
            .expect("reselected viewer frame");
        if frame.display_id == 1 {
            break frame;
        }
        assert!(Instant::now() < deadline, "viewer never moved to display 1");
    };
    assert_eq!(moved.revision, 9);
    assert_eq!(opens_left.load(Ordering::Relaxed), 1);

    // Closing display 1 stops its stream without touching display 2.
    stop_left.store(true, Ordering::Relaxed);
    left.join().unwrap().unwrap();
    // Drain the sibling's backlog so the frames asserted below prove its
    // worker is still publishing rather than replaying its queue.
    let drain_until = Instant::now() + Duration::from_millis(500);
    while Instant::now() < drain_until {
        let _ = first_frames.recv_timeout(Duration::from_millis(100));
    }
    for _ in 0..2 {
        let frame = first_frames
            .recv_timeout(Duration::from_secs(5))
            .expect("sibling stream after close");
        assert_eq!(frame.display_id, 2);
    }
    stop_right.store(true, Ordering::Relaxed);
    right.join().unwrap().unwrap();
}

/// SPEC14: a capture pinned to an older revision keeps stamping it after
/// the topology advanced: its pixels show the old crop, so relabeling
/// them current would lie to the viewers' stale-input fence. A live
/// re-attestation (the supervisor's bump) updates the stamp without
/// restarting the worker.
#[test]
fn pinned_capture_revision_survives_topology_advance() {
    use std::sync::atomic::AtomicU64;

    struct Pinned {
        revision: Arc<AtomicU64>,
    }
    impl FrameSource for Pinned {
        fn size(&self) -> (u16, u16) {
            (64, 64)
        }
        fn poll_frame(&self, timestamp: u64) -> Result<Option<RawFrame>, Error> {
            Ok(Some(solid(64, 64, timestamp)))
        }
        fn capture_revision(&self) -> Arc<AtomicU64> {
            Arc::clone(&self.revision)
        }
    }

    let host = Arc::new(boot());
    let viewer = boot();
    host.set_capture(Box::new(Source {
        idle: false,
        frames: Arc::new(AtomicUsize::new(0)),
        thread: Mutex::new(None),
        wrong_thread: Arc::new(AtomicBool::new(false)),
    }))
    .unwrap();
    for core in [&host, &viewer] {
        core.set_video_codecs(vnilink::VideoCodecCaps::h264_full(4096, 4096, "test"))
            .unwrap();
    }
    // Topology already advanced past the capture's attestation: the
    // supervisor has not reopened or re-attested this owner yet.
    host.set_topology(10, &[twin(2, 0)]).unwrap();
    pair(&host, &viewer);
    host.start_hosting().unwrap();
    let parent = viewer.begin_connect_services(&host.endpoint_id()).unwrap();
    wait_active(&viewer, parent.id);
    let parent = viewer.controller_session(parent.id).unwrap();
    let owner = viewer
        .attach_controller_service(parent.id, parent.generation, ServiceKind::Desktop)
        .unwrap();
    let handle = viewer
        .open_desktop_service(owner.id, owner.generation)
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while handle.topology().ok().map(|d| d.len()) != Some(1) {
        assert!(Instant::now() < deadline, "topology did not propagate");
        std::thread::sleep(Duration::from_millis(10));
    }
    handle.select_displays(&[2]).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while host.capture_displays().unwrap() != vec![2] {
        assert!(Instant::now() < deadline, "selection did not propagate");
        std::thread::sleep(Duration::from_millis(10));
    }
    let frames = handle.subscribe_video().unwrap();

    let pinned = Arc::new(AtomicU64::new(7));
    let stop = Arc::new(AtomicBool::new(false));
    let worker_pinned = Arc::clone(&pinned);
    let worker_stop = Arc::clone(&stop);
    let worker_host = Arc::clone(&host);
    let worker = std::thread::spawn(move || {
        crate::serve_shared_capture(
            &Pinned {
                revision: worker_pinned,
            },
            &worker_host,
            &worker_stop,
            false,
            2,
            |_| {},
            move |_target, width, height, fps, bps| {
                if width != 64 || height != 64 || fps == 0 || bps == 0 {
                    return Err(Error::InvalidSetting("encoder parameters are invalid"));
                }
                Ok(Encoder {
                    slow: false,
                    encodes: Arc::new(AtomicUsize::new(0)),
                    opens: Arc::new(AtomicUsize::new(0)),
                })
            },
        )
    });
    for _ in 0..3 {
        let frame = frames
            .recv_timeout(Duration::from_secs(5))
            .expect("pinned viewer frame");
        assert_eq!(frame.display_id, 2);
        assert_eq!(frame.revision, 7, "stale crop relabeled current");
    }
    // The supervisor re-attests the unchanged capture: the stamp follows
    // without restarting the worker.
    pinned.store(10, Ordering::Release);
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let frame = frames
            .recv_timeout(Duration::from_secs(5))
            .expect("re-attested viewer frame");
        if frame.revision == 10 {
            assert_eq!(frame.display_id, 2);
            break;
        }
        assert_eq!(frame.revision, 7, "stamp skipped the attested revision");
        assert!(Instant::now() < deadline, "stamp never followed the bump");
    }
    stop.store(true, Ordering::Relaxed);
    worker.join().unwrap().unwrap();
}

/// A lone capture still serves viewers before anyone selects: the default
/// rule must not strand the single-source bootstrap.
#[test]
fn lone_capture_serves_unselected_viewers_before_any_selection() {
    let host = Arc::new(boot());
    let viewer = boot();
    host.set_capture(Box::new(Source {
        idle: false,
        frames: Arc::new(AtomicUsize::new(0)),
        thread: Mutex::new(None),
        wrong_thread: Arc::new(AtomicBool::new(false)),
    }))
    .unwrap();
    for core in [&host, &viewer] {
        core.set_video_codecs(vnilink::VideoCodecCaps::h264_full(4096, 4096, "test"))
            .unwrap();
    }
    host.set_topology(9, &[twin(7, 0)]).unwrap();
    pair(&host, &viewer);
    host.start_hosting().unwrap();
    let parent = viewer.begin_connect_services(&host.endpoint_id()).unwrap();
    wait_active(&viewer, parent.id);
    let parent = viewer.controller_session(parent.id).unwrap();
    let owner = viewer
        .attach_controller_service(parent.id, parent.generation, ServiceKind::Desktop)
        .unwrap();
    let handle = viewer
        .open_desktop_service(owner.id, owner.generation)
        .unwrap();
    let frames = handle.subscribe_video().unwrap();
    let stop = Arc::new(AtomicBool::new(false));
    let worker_stop = Arc::clone(&stop);
    let worker_host = Arc::clone(&host);
    let worker = std::thread::spawn(move || {
        crate::serve_shared_capture(
            &Source {
                idle: false,
                frames: Arc::new(AtomicUsize::new(0)),
                thread: Mutex::new(None),
                wrong_thread: Arc::new(AtomicBool::new(false)),
            },
            &worker_host,
            &worker_stop,
            false,
            7,
            |_| {},
            move |_, width, height, fps, bps| {
                if width != 64 || height != 64 || fps == 0 || bps == 0 {
                    return Err(Error::InvalidSetting("encoder parameters are invalid"));
                }
                Ok(Encoder {
                    slow: false,
                    encodes: Arc::new(AtomicUsize::new(0)),
                    opens: Arc::new(AtomicUsize::new(0)),
                })
            },
        )
    });
    let frame = frames
        .recv_timeout(Duration::from_secs(5))
        .expect("bootstrap viewer frame");
    assert_eq!(frame.display_id, 7);
    assert_eq!(frame.revision, 9);
    stop.store(true, Ordering::Relaxed);
    worker.join().unwrap().unwrap();
}

/// A source that attests its pixels and never changes them.
struct Still {
    at: Instant,
}

impl FrameSource for Still {
    fn size(&self) -> (u16, u16) {
        (64, 64)
    }

    fn poll_frame(&self, timestamp: u64) -> Result<Option<RawFrame>, Error> {
        Ok(Some(solid(64, 64, timestamp)))
    }

    fn attested_at(&self) -> Option<Instant> {
        Some(self.at)
    }
}

/// An unchanged screen is encoded once a second, not at the frame rate;
/// a keyframe asked for meanwhile is made from the last picture at once.
#[test]
fn an_unchanged_screen_is_sent_once_a_second_and_a_keyframe_still_comes() {
    let host = Arc::new(boot());
    let viewer = boot();
    host.set_capture(Box::new(Source {
        idle: false,
        frames: Arc::new(AtomicUsize::new(0)),
        thread: Mutex::new(None),
        wrong_thread: Arc::new(AtomicBool::new(false)),
    }))
    .unwrap();
    for core in [&host, &viewer] {
        core.set_video_codecs(vnilink::VideoCodecCaps::h264_full(4096, 4096, "test"))
            .unwrap();
    }
    host.set_topology(9, &[twin(7, 0)]).unwrap();
    pair(&host, &viewer);
    host.start_hosting().unwrap();
    let parent = viewer.begin_connect_services(&host.endpoint_id()).unwrap();
    wait_active(&viewer, parent.id);
    let parent = viewer.controller_session(parent.id).unwrap();
    let owner = viewer
        .attach_controller_service(parent.id, parent.generation, ServiceKind::Desktop)
        .unwrap();
    let handle = viewer
        .open_desktop_service(owner.id, owner.generation)
        .unwrap();
    let frames = handle.subscribe_video().unwrap();
    let encodes = Arc::new(AtomicUsize::new(0));
    let stop = Arc::new(AtomicBool::new(false));
    let (worker_stop, worker_host, worker_encodes) =
        (Arc::clone(&stop), Arc::clone(&host), Arc::clone(&encodes));
    let worker = std::thread::spawn(move || {
        crate::serve_shared_capture(
            &Still { at: Instant::now() },
            &worker_host,
            &worker_stop,
            false,
            7,
            |_| {},
            move |_, _, _, _, _| {
                Ok(Encoder {
                    slow: false,
                    encodes: Arc::clone(&worker_encodes),
                    opens: Arc::new(AtomicUsize::new(0)),
                })
            },
        )
    });
    let first = frames
        .recv_timeout(Duration::from_secs(5))
        .expect("first picture");
    assert!(first.keyframe);
    std::thread::sleep(Duration::from_millis(600));
    assert_eq!(
        encodes.load(Ordering::Relaxed),
        1,
        "an unchanged screen was encoded again within a second"
    );
    handle.request_keyframe().unwrap();
    let asked = Instant::now();
    let keyframe = loop {
        let frame = frames
            .recv_timeout(Duration::from_secs(5))
            .expect("the keyframe asked for");
        if frame.keyframe {
            break frame;
        }
    };
    assert!(keyframe.keyframe);
    assert!(
        asked.elapsed() < Duration::from_millis(500),
        "the keyframe waited for the screen to change"
    );
    stop.store(true, Ordering::Relaxed);
    worker.join().unwrap().unwrap();
}

/// An [`Encoder`] that refines still pictures.
struct Refining(Encoder);

impl VideoEncoder for Refining {
    fn encode(&mut self, frame: &RawFrame, params: &EncodeParams) -> Result<EncodedSample, Error> {
        self.0.encode(frame, params)
    }

    fn refines_still_pictures(&self) -> bool {
        true
    }
}

/// After a keyframe an encoder that refines is given the unchanged picture
/// at the frame rate, for the refinement window only; then once a second
/// again.
#[test]
fn an_unchanged_screen_is_encoded_again_while_a_keyframe_is_refined() {
    let host = Arc::new(boot());
    let viewer = boot();
    host.set_capture(Box::new(Source {
        idle: false,
        frames: Arc::new(AtomicUsize::new(0)),
        thread: Mutex::new(None),
        wrong_thread: Arc::new(AtomicBool::new(false)),
    }))
    .unwrap();
    for core in [&host, &viewer] {
        core.set_video_codecs(vnilink::VideoCodecCaps::h264_full(4096, 4096, "test"))
            .unwrap();
    }
    host.set_topology(9, &[twin(7, 0)]).unwrap();
    pair(&host, &viewer);
    host.start_hosting().unwrap();
    let parent = viewer.begin_connect_services(&host.endpoint_id()).unwrap();
    wait_active(&viewer, parent.id);
    let parent = viewer.controller_session(parent.id).unwrap();
    let owner = viewer
        .attach_controller_service(parent.id, parent.generation, ServiceKind::Desktop)
        .unwrap();
    let handle = viewer
        .open_desktop_service(owner.id, owner.generation)
        .unwrap();
    let frames = handle.subscribe_video().unwrap();
    let encodes = Arc::new(AtomicUsize::new(0));
    let stop = Arc::new(AtomicBool::new(false));
    let (worker_stop, worker_host, worker_encodes) =
        (Arc::clone(&stop), Arc::clone(&host), Arc::clone(&encodes));
    let worker = std::thread::spawn(move || {
        crate::serve_shared_capture(
            &Still { at: Instant::now() },
            &worker_host,
            &worker_stop,
            false,
            7,
            |_| {},
            move |_, _, _, _, _| {
                Ok(Refining(Encoder {
                    slow: false,
                    encodes: Arc::clone(&worker_encodes),
                    opens: Arc::new(AtomicUsize::new(0)),
                }))
            },
        )
    });
    let first = frames
        .recv_timeout(Duration::from_secs(5))
        .expect("first picture");
    assert!(first.keyframe);
    // A viewer reads what arrives: one that does not falls behind and asks
    // for keyframes, each starting a new window.
    let keyframes = Arc::new(AtomicUsize::new(0));
    let reader = {
        let (keyframes, stop) = (Arc::clone(&keyframes), Arc::clone(&stop));
        std::thread::spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                if let Some(frame) = frames.recv_timeout(Duration::from_millis(20)) {
                    keyframes.fetch_add(usize::from(frame.keyframe), Ordering::Relaxed);
                }
            }
        })
    };
    std::thread::sleep(Duration::from_millis(600));
    let refined = encodes.load(Ordering::Relaxed);
    assert!(
        refined >= 5,
        "only {refined} pictures were encoded while refining"
    );
    // Past the window: back to a picture a second.
    std::thread::sleep(Duration::from_millis(
        crate::pump::REFINE_FOR_US / 1_000 - 600 + 300,
    ));
    let settled = encodes.load(Ordering::Relaxed);
    std::thread::sleep(Duration::from_millis(600));
    let after = encodes.load(Ordering::Relaxed);
    assert_eq!(
        keyframes.load(Ordering::Relaxed),
        0,
        "no keyframe was asked for"
    );
    assert!(
        after <= settled + 1,
        "an unchanged screen was still encoded at the frame rate after the window: {settled} then {after}"
    );
    stop.store(true, Ordering::Relaxed);
    reader.join().unwrap();
    worker.join().unwrap().unwrap();
}

/// An encoder that works for `good` pictures, then fails every one.
struct FailsAfter {
    good: usize,
}

impl VideoEncoder for FailsAfter {
    fn encode(&mut self, _frame: &RawFrame, params: &EncodeParams) -> Result<EncodedSample, Error> {
        if self.good == 0 {
            return Err(Error::EncodeRejected("the test encoder stopped"));
        }
        self.good -= 1;
        sample(params.force_keyframe)
    }
}

/// An encoder that fails mid-stream ends that viewer's picture, and the
/// viewer learns it was the host's encoder, not the link.
#[test]
fn an_encoder_failing_mid_stream_tells_the_viewer_why() {
    let host = Arc::new(boot());
    let viewer = boot();
    host.set_capture(Box::new(Source {
        idle: false,
        frames: Arc::new(AtomicUsize::new(0)),
        thread: Mutex::new(None),
        wrong_thread: Arc::new(AtomicBool::new(false)),
    }))
    .unwrap();
    for core in [&host, &viewer] {
        core.set_video_codecs(vnilink::VideoCodecCaps::h264_full(4096, 4096, "test"))
            .unwrap();
    }
    pair(&host, &viewer);
    host.start_hosting().unwrap();
    let parent = viewer.begin_connect_services(&host.endpoint_id()).unwrap();
    wait_active(&viewer, parent.id);
    let parent = viewer.controller_session(parent.id).unwrap();
    let owner = viewer
        .attach_controller_service(parent.id, parent.generation, ServiceKind::Desktop)
        .unwrap();
    let handle = viewer
        .open_desktop_service(owner.id, owner.generation)
        .unwrap();
    let frames = handle.subscribe_video().unwrap();
    let stop = Arc::new(AtomicBool::new(false));
    let worker_stop = Arc::clone(&stop);
    let worker_host = Arc::clone(&host);
    let worker = std::thread::spawn(move || {
        crate::serve_shared_capture(
            &Source {
                idle: false,
                frames: Arc::new(AtomicUsize::new(0)),
                thread: Mutex::new(None),
                wrong_thread: Arc::new(AtomicBool::new(false)),
            },
            &worker_host,
            &worker_stop,
            false,
            0,
            |_| {},
            |_, _, _, _, _| Ok(FailsAfter { good: 5 }),
        )
    });
    frames
        .recv_timeout(Duration::from_secs(5))
        .expect("a picture before the encoder fails");
    let deadline = Instant::now() + Duration::from_secs(10);
    while handle.end().is_none() {
        assert!(
            Instant::now() < deadline,
            "the viewer never learned why the picture stopped"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(
        handle.end(),
        Some(vnilink::DesktopEnd::Host(vnilink::StreamEnd::Failed(
            vnilink::StreamFailure::Encoder
        )))
    );
    let hosted = host
        .hosted_services()
        .unwrap()
        .into_iter()
        .find(|service| service.kind == ServiceKind::Desktop)
        .map(|service| service.phase);
    // The record may already be gone; while it lasts it names the failure.
    if let Some(phase) = hosted {
        assert_eq!(
            phase,
            vnilink::ServicePhase::Closed(vnilink::CloseCause::Failed(
                vnilink::StreamFailure::Encoder
            ))
        );
    }
    stop.store(true, Ordering::Relaxed);
    worker.join().unwrap().unwrap();
}

/// A host whose capture sends still pictures.
struct StillsCapture;

impl vnilink::HostCapture for StillsCapture {
    fn monitor(&self) -> (u16, u16) {
        (64, 64)
    }
    fn stills(&self) -> bool {
        true
    }
}

/// The roles of the pictures a viewer receives over `watch` of an
/// unchanged screen, from a host that sends stills, to a window that shows
/// them when `shows`.
fn roles_of_an_unchanged_screen(shows: bool, watch: Duration) -> Vec<vnilink::FrameRole> {
    let host = Arc::new(boot());
    let viewer = boot();
    host.set_capture(Box::new(StillsCapture)).unwrap();
    for core in [&host, &viewer] {
        core.set_video_codecs(vnilink::VideoCodecCaps::h264_full(4096, 4096, "test"))
            .unwrap();
    }
    viewer
        .set_window_decoders(vec![
            vnilink::WindowDecoder {
                codec: vnilink::VideoCodec::H264,
                hardware: false,
                chroma444: false,
            },
            vnilink::WindowDecoder {
                codec: vnilink::VideoCodec::Av1,
                hardware: false,
                chroma444: shows,
            },
        ])
        .unwrap();
    host.set_topology(9, &[twin(7, 0)]).unwrap();
    pair(&host, &viewer);
    host.start_hosting().unwrap();
    let parent = viewer.begin_connect_services(&host.endpoint_id()).unwrap();
    wait_active(&viewer, parent.id);
    let parent = viewer.controller_session(parent.id).unwrap();
    let owner = viewer
        .attach_controller_service(parent.id, parent.generation, ServiceKind::Desktop)
        .unwrap();
    let handle = viewer
        .open_desktop_service(owner.id, owner.generation)
        .unwrap();
    let frames = handle.subscribe_video().unwrap();
    let stop = Arc::new(AtomicBool::new(false));
    let (worker_stop, worker_host) = (Arc::clone(&stop), Arc::clone(&host));
    let worker = std::thread::spawn(move || {
        crate::serve_shared_capture(
            &Still { at: Instant::now() },
            &worker_host,
            &worker_stop,
            false,
            7,
            |_| {},
            move |_, _, _, _, _| {
                Ok(Encoder {
                    slow: false,
                    encodes: Arc::new(AtomicUsize::new(0)),
                    opens: Arc::new(AtomicUsize::new(0)),
                })
            },
        )
    });
    let until = Instant::now() + watch;
    let mut roles = Vec::new();
    while Instant::now() < until {
        if let Some(frame) = frames.recv_timeout(Duration::from_millis(50)) {
            if frame.role == vnilink::FrameRole::Still {
                assert_eq!(frame.display_id, 7, "a still is stamped like any picture");
            }
            roles.push(frame.role);
        }
    }
    stop.store(true, Ordering::Relaxed);
    worker.join().unwrap().unwrap();
    roles
}

/// An unchanged screen brings one still picture once it stayed so for
/// [`crate::STILL_AFTER`], and no other; the pictures of it the stream
/// sends meanwhile say the screen is unchanged.
#[test]
fn an_unchanged_screen_brings_one_still_picture() {
    use vnilink::FrameRole::{Changed, Still, Unchanged};
    let watch = crate::STILL_AFTER + Duration::from_millis(2_500);
    let roles = roles_of_an_unchanged_screen(true, watch);
    assert_eq!(roles.first(), Some(&Changed), "{roles:?}");
    assert_eq!(
        roles.iter().filter(|role| **role == Still).count(),
        1,
        "{roles:?}"
    );
    assert!(roles[1..].iter().all(|role| *role != Changed), "{roles:?}");
    assert!(roles.contains(&Unchanged), "{roles:?}");
    // A window that cannot show them is sent none.
    let roles = roles_of_an_unchanged_screen(false, watch);
    assert!(!roles.contains(&Still), "{roles:?}");
}
