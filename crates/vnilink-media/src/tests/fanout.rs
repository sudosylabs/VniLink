// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! Shared capture: one reader, independent encoders.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
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

/// A gate a test holds a thread at for as long as it likes, and sees it
/// held there: work held at it proves what goes on without it, on a
/// computer of any speed.
#[derive(Default)]
struct Gate {
    /// Whether the gate is closed, and how many threads it holds.
    state: Mutex<(bool, usize)>,
    changed: Condvar,
}

impl Gate {
    fn close(&self) {
        self.state.lock().unwrap().0 = true;
    }

    fn open(&self) {
        self.state.lock().unwrap().0 = false;
        self.changed.notify_all();
    }

    /// Passes, once the gate is open.
    fn pass(&self) {
        let mut state = self.state.lock().unwrap();
        if !state.0 {
            return;
        }
        state.1 += 1;
        self.changed.notify_all();
        while state.0 {
            state = self.changed.wait(state).unwrap();
        }
        state.1 -= 1;
    }

    /// Waits until the gate holds a thread, and says whether it does.
    fn holds_one(&self, within: Duration) -> bool {
        let state = self.state.lock().unwrap();
        let (state, _) = self
            .changed
            .wait_timeout_while(state, within, |state| state.1 == 0)
            .unwrap();
        state.1 > 0
    }
}

struct Encoder {
    /// Where this encoder is held before each picture, if it is.
    held: Option<Arc<Gate>>,
    encodes: Arc<AtomicUsize>,
    opens: Arc<AtomicUsize>,
}

impl VideoEncoder for Encoder {
    fn encode(&mut self, frame: &RawFrame, params: &EncodeParams) -> Result<EncodedSample, Error> {
        if let Some(gate) = &self.held {
            gate.pass();
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
    EncodedSample::annex_b(keyframe, annex, vnilink::YuvMatrix::Bt709)
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
    let slow_gate = Arc::new(Gate::default());
    let stop = Arc::new(AtomicBool::new(false));
    let worker_host = Arc::clone(&host);
    let worker_stop = Arc::clone(&stop);
    let worker_frames = Arc::clone(&frames);
    let worker_wrong = Arc::clone(&wrong_thread);
    let worker_fast = Arc::clone(&fast_encodes);
    let worker_slow = Arc::clone(&slow_encodes);
    let worker_live = Arc::clone(&live_encoders);
    let worker_gate = Arc::clone(&slow_gate);
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
                    held: (target.id == slow_target).then(|| Arc::clone(&worker_gate)),
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

    // The slow viewer's encoder is held at its next picture for as long as
    // the test likes: the reader and the fast viewer go on without it.
    slow_gate.close();
    assert!(
        slow_gate.holds_one(Duration::from_secs(5)),
        "the slow encoder was not given another picture"
    );
    let slow_count = slow_encodes.load(Ordering::Relaxed);
    let fast_before = fast_encodes.load(Ordering::Relaxed);
    let read_before = frames.load(Ordering::Relaxed);
    let deadline = Instant::now() + Duration::from_secs(10);
    while fast_encodes.load(Ordering::Relaxed) < fast_before + 8
        || frames.load(Ordering::Relaxed) < read_before + 8
    {
        assert!(
            Instant::now() < deadline,
            "held behind the slow encoder: fast {} encodes, reader {} frames",
            fast_encodes.load(Ordering::Relaxed) - fast_before,
            frames.load(Ordering::Relaxed) - read_before
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(
        slow_encodes.load(Ordering::Relaxed),
        slow_count,
        "the slow encoder ran while held"
    );
    assert!(!wrong_thread.load(Ordering::Relaxed));
    slow_gate.open();

    slow_handle.close();
    let deadline = Instant::now() + Duration::from_secs(5);
    while host.video_targets().unwrap().len() != 1 {
        assert!(Instant::now() < deadline, "slow owner did not leave");
        std::thread::sleep(Duration::from_millis(10));
    }
    let after_close = fast_encodes.load(Ordering::Relaxed);
    let deadline = Instant::now() + Duration::from_secs(5);
    while fast_encodes.load(Ordering::Relaxed) <= after_close {
        assert!(
            Instant::now() < deadline,
            "the remaining viewer stopped when its sibling closed"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
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
                        held: None,
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
                    held: None,
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
                    held: None,
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

/// A source that attests its pixels and never changes them, and whose
/// reads a test can hold.
struct Still {
    at: Instant,
    held: Arc<Gate>,
}

impl FrameSource for Still {
    fn size(&self) -> (u16, u16) {
        (64, 64)
    }

    fn poll_frame(&self, timestamp: u64) -> Result<Option<RawFrame>, Error> {
        self.held.pass();
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
    let reads = Arc::new(Gate::default());
    let stop = Arc::new(AtomicBool::new(false));
    let (worker_stop, worker_host, worker_encodes, worker_reads) = (
        Arc::clone(&stop),
        Arc::clone(&host),
        Arc::clone(&encodes),
        Arc::clone(&reads),
    );
    let serving = Instant::now();
    let worker = std::thread::spawn(move || {
        crate::serve_shared_capture(
            &Still {
                at: Instant::now(),
                held: worker_reads,
            },
            &worker_host,
            &worker_stop,
            false,
            7,
            |_| {},
            move |_, _, _, _, _| {
                Ok(Encoder {
                    held: None,
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
    // The screen is read at the frame rate; what is encoded is once a
    // second at the most, so a busy computer that stretched this wait
    // allows more.
    std::thread::sleep(Duration::from_millis(600));
    let encoded = encodes.load(Ordering::Relaxed);
    let seconds = serving.elapsed().as_secs() as usize;
    assert!(
        encoded <= 1 + seconds,
        "an unchanged screen was encoded {encoded} times in {seconds} s"
    );
    // With the screen's reads held, no picture newer than the last and no
    // keepalive can come: the keyframe asked for is made from the last
    // picture, not from the next read.
    reads.close();
    assert!(
        reads.holds_one(Duration::from_secs(5)),
        "the screen was not read again"
    );
    handle.request_keyframe().unwrap();
    let keyframe = loop {
        let frame = frames
            .recv_timeout(Duration::from_secs(5))
            .expect("the keyframe waited for the screen to change");
        if frame.keyframe {
            break frame;
        }
    };
    assert!(keyframe.keyframe);
    reads.open();
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
            &Still {
                at: Instant::now(),
                held: Arc::default(),
            },
            &worker_host,
            &worker_stop,
            false,
            7,
            |_| {},
            move |_, _, _, _, _| {
                Ok(Refining(Encoder {
                    held: None,
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

/// Coloured text, which 4:2:0 blurs: a still of it is worth sending.
const COLOURED: (u16, u16) = (64, 64);

/// A source of unchanged coloured text, attested once.
struct ColouredStill {
    at: Instant,
    page: RawFrame,
}

impl FrameSource for ColouredStill {
    fn size(&self) -> (u16, u16) {
        COLOURED
    }

    fn poll_frame(&self, timestamp: u64) -> Result<Option<RawFrame>, Error> {
        Ok(Some(self.page.clone().with_timestamp(timestamp)))
    }

    fn attested_at(&self) -> Option<Instant> {
        Some(self.at)
    }
}

/// Coloured text where something changes every half second, covering
/// `share` of the screen each time, as its source says.
struct Ticking {
    started: Instant,
    share: f32,
    regions: Option<Vec<vnilink::Region>>,
    page: RawFrame,
    /// The tick the last read saw: a read says a change only after a new one.
    read: Mutex<u128>,
}

impl FrameSource for Ticking {
    fn size(&self) -> (u16, u16) {
        COLOURED
    }

    fn poll_frame(&self, timestamp: u64) -> Result<Option<RawFrame>, Error> {
        let tick = self.started.elapsed().as_millis() / 500;
        let mut read = self.read.lock().unwrap();
        let changed = if *read == tick {
            crate::Changes::none()
        } else {
            crate::Changes {
                share: self.share,
                regions: self.regions.clone(),
            }
        };
        *read = tick;
        Ok(Some(
            self.page
                .clone()
                .with_timestamp(timestamp)
                .with_changed(Some(changed)),
        ))
    }

    fn attested_at(&self) -> Option<Instant> {
        let ticks = self.started.elapsed().as_millis() / 500;
        Some(self.started + Duration::from_millis(ticks as u64 * 500))
    }
}

/// A host whose capture sends still pictures.
struct StillsCapture;

impl vnilink::HostCapture for StillsCapture {
    fn monitor(&self) -> (u16, u16) {
        COLOURED
    }
    fn stills(&self) -> bool {
        true
    }
}

/// The roles of the pictures a viewer receives over `watch` of an
/// unchanged screen, from a host that sends stills, to a window that shows
/// them when `shows`.
fn roles_of_an_unchanged_screen(shows: bool, watch: Duration) -> Vec<vnilink::FrameRole> {
    roles_of(
        ColouredStill {
            at: Instant::now(),
            page: crate::measure::colour_text_page(COLOURED.0, COLOURED.1, 0),
        },
        shows,
        watch,
    )
}

/// The roles of the pictures a viewer receives over `watch` of `source`,
/// from a host that sends stills, to a window that shows them when `shows`.
fn roles_of(
    source: impl FrameSource + Send + 'static,
    shows: bool,
    watch: Duration,
) -> Vec<vnilink::FrameRole> {
    frames_of(source, shows, watch)
        .into_iter()
        .map(|frame| frame.role)
        .collect()
}

/// The frames a viewer receives over `watch` of `source`, as
/// [`roles_of`] serves it.
fn frames_of(
    source: impl FrameSource + Send + 'static,
    shows: bool,
    watch: Duration,
) -> Vec<vnilink::EncodedFrame> {
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
                bt601: false,
            },
            vnilink::WindowDecoder {
                codec: vnilink::VideoCodec::Av1,
                hardware: false,
                chroma444: shows,
                bt601: false,
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
            &source,
            &worker_host,
            &worker_stop,
            false,
            7,
            |_| {},
            move |_, _, _, _, _| {
                Ok(Encoder {
                    held: None,
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
            roles.push(frame);
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

/// A clock or a caret changing leaves the screen still enough for a still;
/// half of it changing as often does not.
#[test]
fn small_changes_still_bring_a_still_and_large_ones_do_not() {
    let watch = crate::STILL_AFTER + Duration::from_millis(2_500);
    let ticking = |share| Ticking {
        started: Instant::now(),
        share,
        regions: None,
        page: crate::measure::colour_text_page(COLOURED.0, COLOURED.1, 0),
        read: Mutex::new(u128::MAX),
    };
    let roles = roles_of(ticking(0.01), true, watch);
    assert!(roles.contains(&vnilink::FrameRole::Still), "{roles:?}");
    let roles = roles_of(ticking(0.5), true, watch);
    assert!(!roles.contains(&vnilink::FrameRole::Still), "{roles:?}");
}

/// A picture says where the screen changed since the picture sent before
/// it, as its source told the host.
#[test]
fn a_changed_picture_says_where_the_screen_changed() {
    let clock = vnilink::Region {
        x: 40,
        y: 56,
        width: 20,
        height: 8,
    };
    let frames = frames_of(
        Ticking {
            started: Instant::now(),
            share: 0.04,
            regions: Some(vec![clock]),
            page: crate::measure::colour_text_page(COLOURED.0, COLOURED.1, 0),
            read: Mutex::new(u128::MAX),
        },
        false,
        Duration::from_millis(1_600),
    );
    // The first picture is the whole screen; the ticks after it say the clock.
    assert_eq!(
        frames.first().map(|frame| frame.regions.clone()),
        Some(None)
    );
    let ticks: Vec<_> = frames
        .iter()
        .skip(1)
        .filter(|frame| frame.role == vnilink::FrameRole::Changed)
        .collect();
    assert!(!ticks.is_empty(), "{frames:?}");
    assert!(
        ticks.iter().all(|frame| frame
            .regions
            .as_ref()
            .is_some_and(|regions| regions.contains(&clock))),
        "{frames:?}"
    );
}

/// A source that is told of each picture as it arrives, as the platform
/// captures are, and records when it is read.
struct Pushing {
    channel: crate::channel::FrameChannel,
    seen: std::sync::atomic::AtomicU64,
    arrivals: Mutex<Vec<Instant>>,
    reads: Mutex<Vec<Instant>>,
    /// Each read's timestamp, on the reader's own clock, and whether the
    /// wait before it ended on a picture rather than on its deadline.
    stamps: Mutex<Vec<(Duration, bool)>>,
    woken: AtomicBool,
    waits: AtomicUsize,
    /// The longest any wait for a picture was asked to last.
    longest_wait: Mutex<Duration>,
}

impl Pushing {
    fn new() -> Self {
        Self {
            channel: crate::channel::FrameChannel::new(),
            seen: std::sync::atomic::AtomicU64::new(0),
            arrivals: Mutex::new(Vec::new()),
            reads: Mutex::new(Vec::new()),
            stamps: Mutex::new(Vec::new()),
            woken: AtomicBool::new(false),
            waits: AtomicUsize::new(0),
            longest_wait: Mutex::new(Duration::ZERO),
        }
    }

    fn arrive(&self) {
        self.arrivals.lock().unwrap().push(Instant::now());
        self.channel.push(crate::channel::ChannelSlot {
            width: 64,
            height: 64,
            stride: 256,
            bytes: crate::Pixels::from(vec![0; 64 * 64 * 4]),
            captured_at: None,
            changed: None,
        });
    }
}

impl FrameSource for Pushing {
    fn size(&self) -> (u16, u16) {
        (64, 64)
    }

    fn poll_frame(&self, timestamp: u64) -> Result<Option<RawFrame>, Error> {
        self.seen
            .store(self.channel.generation(), Ordering::Release);
        self.reads.lock().unwrap().push(Instant::now());
        self.stamps.lock().unwrap().push((
            Duration::from_micros(timestamp),
            self.woken.swap(false, Ordering::AcqRel),
        ));
        Ok(self.channel.try_latest()?.map(|_| solid(64, 64, timestamp)))
    }

    fn wait_frame(&self, deadline: Instant) -> crate::FrameWait {
        self.waits.fetch_add(1, Ordering::Relaxed);
        {
            let mut longest = self.longest_wait.lock().unwrap();
            *longest = (*longest).max(deadline.saturating_duration_since(Instant::now()));
        }
        let seen = self.seen.load(Ordering::Acquire);
        let arrived = self.channel.wait_newer(seen, deadline) != seen;
        self.woken.store(arrived, Ordering::Release);
        crate::FrameWait::from_arrived(arrived)
    }

    fn attested_at(&self) -> Option<Instant> {
        self.channel.pushed_at()
    }
}

/// Serves `source` to one viewer and runs `body` once its first picture has
/// arrived, with the stop flag, the frame period and the count of pictures
/// encoded so far. Returns the number of pictures encoded while `body` ran
/// and how long the worker took to end after `stop`.
fn serve_pushing(
    source: &Arc<Pushing>,
    body: impl FnOnce(&AtomicBool, Duration, &AtomicUsize),
) -> (usize, Duration) {
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
    let period = Arc::new(Mutex::new(Duration::from_millis(33)));
    let stop = Arc::new(AtomicBool::new(false));
    let worker = {
        let (stop, host, encodes, period, source) = (
            Arc::clone(&stop),
            Arc::clone(&host),
            Arc::clone(&encodes),
            Arc::clone(&period),
            Arc::clone(source),
        );
        std::thread::spawn(move || {
            crate::serve_shared_capture(
                &*source,
                &host,
                &stop,
                false,
                7,
                |targets| {
                    if let Some(target) = targets.first() {
                        *period.lock().unwrap() = Duration::from_micros(
                            1_000_000 / u64::from(target.quality.max_frame_rate.max(1)),
                        );
                    }
                },
                move |_, _, _, _, _| {
                    Ok(Encoder {
                        held: None,
                        encodes: Arc::clone(&encodes),
                        opens: Arc::new(AtomicUsize::new(0)),
                    })
                },
            )
        })
    };
    // The first picture opens the stream; the viewer's request for one is
    // answered from what the source pushes.
    source.arrive();
    frames
        .recv_timeout(Duration::from_secs(5))
        .expect("first picture");
    let period = *period.lock().unwrap();
    let before = encodes.load(Ordering::Relaxed);
    body(&stop, period, &encodes);
    let during = encodes.load(Ordering::Relaxed) - before;
    let asked = Instant::now();
    stop.store(true, Ordering::Release);
    worker.join().unwrap().unwrap();
    (during, asked.elapsed())
}

/// Arrival times that are not on the frame rate's grid.
fn irregular(period: Duration, count: usize) -> Vec<Duration> {
    (0..count)
        .map(|index| period.mul_f64([1.7, 2.3, 3.1, 1.2, 2.9, 4.4][index % 6]))
        .collect()
}

#[test]
fn a_picture_is_read_when_it_arrives_not_on_a_timer() {
    let source = Arc::new(Pushing::new());
    serve_pushing(&source, |_, period, _| {
        source.stamps.lock().unwrap().clear();
        source.arrivals.lock().unwrap().clear();
        for gap in irregular(period, 14) {
            std::thread::sleep(gap);
            source.arrive();
        }
        // The last picture is read before the stream stops.
        let deadline = Instant::now() + Duration::from_secs(5);
        while source.channel.generation() != source.seen.load(Ordering::Acquire) {
            assert!(Instant::now() < deadline, "the last picture was not read");
            std::thread::sleep(Duration::from_millis(1));
        }
    });
    let stamps = source.stamps.lock().unwrap().clone();
    let arrivals = source.arrivals.lock().unwrap().len();
    // What woke each read says what drove it: a reader on a timer reads
    // when its deadline passes, picture or not, and a busy computer only
    // makes such reads rarer. A keepalive may read a still screen once a
    // second.
    assert!(!stamps.is_empty() && stamps.len() <= arrivals, "{stamps:?}");
    for (index, (at, woken)) in stamps.iter().enumerate() {
        let after_a_second = index
            .checked_sub(1)
            .is_some_and(|before| *at >= stamps[before].0 + Duration::from_secs(1));
        assert!(
            *woken || after_a_second,
            "read {index} came on a deadline, not a picture: {stamps:?}"
        );
    }
}

#[test]
fn pictures_faster_than_the_frame_rate_are_read_no_faster_than_it() {
    let source = Arc::new(Pushing::new());
    let mut observed = Duration::ZERO;
    serve_pushing(&source, |_, period, _| {
        observed = period;
        source.stamps.lock().unwrap().clear();
        let end = Instant::now() + Duration::from_millis(900);
        while Instant::now() < end {
            source.arrive();
            std::thread::sleep(period / 4);
        }
    });
    // Read on the reader's own clock, which a busy computer can only
    // spread further apart: reads follow a grid a period apart, one at
    // most a quarter of a period before its place.
    let stamps: Vec<Duration> = source
        .stamps
        .lock()
        .unwrap()
        .iter()
        .map(|(at, _)| *at)
        .collect();
    assert!(stamps.len() >= 2, "{stamps:?}");
    let span = *stamps.last().unwrap() - stamps[0];
    let allowed = (span.as_secs_f64() / observed.as_secs_f64()) as usize + 2;
    assert!(
        stamps.len() <= allowed,
        "{} reads in {span:?} at a period of {observed:?}",
        stamps.len()
    );
}

#[test]
fn pictures_just_above_the_frame_rate_do_not_spin_the_reader() {
    let source = Arc::new(Pushing::new());
    let mut observed = Duration::ZERO;
    serve_pushing(&source, |_, period, _| {
        observed = period;
        source.reads.lock().unwrap().clear();
        source.waits.store(0, Ordering::Relaxed);
        let end = Instant::now() + Duration::from_millis(900);
        while Instant::now() < end {
            source.arrive();
            std::thread::sleep(period.mul_f64(0.97));
        }
    });
    let reads = source.reads.lock().unwrap().len();
    let waits = source.waits.load(Ordering::Relaxed);
    let possible = (Duration::from_millis(900).as_secs_f64() / observed.as_secs_f64()) as usize;
    assert!(reads <= possible + 2, "{reads} reads, {possible} periods");
    assert!(waits <= 3 * possible, "{waits} waits, {possible} periods");
}

#[test]
fn a_still_screen_is_kept_alive_once_a_second_without_polling_at_the_frame_rate() {
    let source = Arc::new(Pushing::new());
    let mut still = Duration::ZERO;
    let (encodes, _) = serve_pushing(&source, |_, _, encodes| {
        source.reads.lock().unwrap().clear();
        let before = encodes.load(Ordering::Relaxed);
        let started = Instant::now();
        // Two keepalives, however long a busy computer takes to make them.
        while encodes.load(Ordering::Relaxed) < before + 2 {
            assert!(
                started.elapsed() < Duration::from_secs(20),
                "a still screen was not kept alive"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        still = started.elapsed();
    });
    // Measured against the time the screen actually stayed still, which a
    // busy computer stretches: polling at the frame rate reads dozens of
    // times a second.
    let seconds = still.as_secs_f64();
    assert!(
        encodes <= seconds as usize + 2,
        "{encodes} pictures in {still:?} of a still screen"
    );
    let reads = source.reads.lock().unwrap().len();
    assert!(
        reads as f64 <= 5.0 * seconds,
        "{reads} reads of a still screen in {still:?}"
    );
}

#[test]
fn stop_is_seen_while_waiting_for_a_picture() {
    let source = Arc::new(Pushing::new());
    let (_, ended) = serve_pushing(&source, |_, _, _| {
        // The reader waits on an empty channel meanwhile.
        let waits = source.waits.load(Ordering::Relaxed);
        while source.waits.load(Ordering::Relaxed) < waits + 2 {
            std::thread::sleep(Duration::from_millis(1));
        }
    });
    // A wait that ran on until a picture would never see stop: each is
    // asked to end within a few milliseconds, and a busy computer only
    // leaves less of that when it begins.
    let longest = *source.longest_wait.lock().unwrap();
    assert!(
        longest <= Duration::from_millis(100),
        "a wait for a picture was asked to last {longest:?}"
    );
    assert!(ended < Duration::from_secs(10), "ended after {ended:?}");
}
