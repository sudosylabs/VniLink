// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

use std::sync::{Arc, Mutex};

use crate::annexb::{to_annex_b, AvccAccessUnit};
use crate::error::{Error, PublishError};
use crate::frame::RawFrame;
use crate::host::HostSession;
use crate::pump::{EncodeParams, EncodedSample, FrameSink, PumpOutput, VideoEncoder};
use crate::{parse_annex_b, Quality};

struct Fake {
    width: u16,
    height: u16,
    /// Whether this encoder takes a new rate while running; what it took.
    retunes: Option<Arc<Mutex<Vec<(u32, u8)>>>>,
}

impl Fake {
    fn open(
        opens: &Arc<Mutex<usize>>,
        width: u16,
        height: u16,
        frame_rate: u8,
        bitrate_bps: u32,
    ) -> Result<Self, Error> {
        *opens.lock().unwrap() += 1;
        if width == 0 || height == 0 || width % 2 != 0 || height % 2 != 0 {
            return Err(Error::InvalidFrame(
                "frame width and height must be non-zero",
            ));
        }
        if frame_rate == 0 || bitrate_bps == 0 {
            return Err(Error::InvalidSetting("frame rate must be non-zero"));
        }
        Ok(Self {
            width,
            height,
            retunes: None,
        })
    }
}

impl VideoEncoder for Fake {
    fn encode(&mut self, frame: &RawFrame, params: &EncodeParams) -> Result<EncodedSample, Error> {
        if frame.width() != self.width || frame.height() != self.height {
            return Err(Error::InvalidFrame(
                "the frame size does not match the encoder",
            ));
        }
        sample(params.force_keyframe)
    }

    fn retune(&mut self, bitrate_bps: u32, frame_rate: u8) -> Result<bool, Error> {
        Ok(self.retunes.as_ref().is_some_and(|taken| {
            taken.lock().unwrap().push((bitrate_bps, frame_rate));
            true
        }))
    }
}

fn sample(keyframe: bool) -> Result<EncodedSample, Error> {
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

fn solid(width: u16, height: u16, timestamp_us: u64) -> RawFrame {
    RawFrame::bgra(
        width,
        height,
        timestamp_us,
        vec![0; usize::from(width) * usize::from(height) * 4],
    )
    .unwrap()
}

#[test]
fn neutral_capture_worker_stops_when_its_desktop_owner_closes() {
    capture_worker_exit(false, CaptureExit::Owner);
}

#[derive(Clone, Copy)]
enum CaptureExit {
    Owner,
    Parent,
    Revoke,
    Stop,
    Shutdown,
    DuringRead,
}

#[test]
fn idle_capture_stops_on_owner_close_before_the_first_picture() {
    capture_worker_exit(true, CaptureExit::Owner);
}
#[test]
fn idle_capture_stops_on_parent_loss_before_the_first_picture() {
    capture_worker_exit(true, CaptureExit::Parent);
}
#[test]
fn idle_capture_stops_on_revoke_before_the_first_picture() {
    capture_worker_exit(true, CaptureExit::Revoke);
}
#[test]
fn idle_capture_stops_on_caller_stop_before_the_first_picture() {
    capture_worker_exit(true, CaptureExit::Stop);
}
#[test]
fn idle_capture_stops_on_core_shutdown_before_the_first_picture() {
    capture_worker_exit(true, CaptureExit::Shutdown);
}

#[test]
fn shutdown_during_capture_read_does_not_publish_or_report_a_capture_fault() {
    capture_worker_exit(false, CaptureExit::DuringRead);
}

fn capture_worker_exit(idle: bool, exit: CaptureExit) {
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::time::{Duration, Instant};
    use vnilink::{
        Core, CoreConfig, CustomRelay, Grants, Listen, RelaySettings, ServiceKind, SessionPhase,
    };
    struct Source {
        idle: bool,
        polled: Arc<AtomicBool>,
        shutdown: Option<std::sync::Weak<Core>>,
    }
    impl crate::FrameSource for Source {
        fn size(&self) -> (u16, u16) {
            (64, 64)
        }
        fn poll_frame(&self, timestamp: u64) -> Result<Option<RawFrame>, Error> {
            self.polled.store(true, Ordering::Release);
            if let Some(core) = self.shutdown.as_ref().and_then(std::sync::Weak::upgrade) {
                core.shutdown();
            }
            if self.idle {
                Ok(None)
            } else {
                Ok(Some(solid(64, 64, timestamp)))
            }
        }
    }
    impl vnilink::HostCapture for Source {
        fn monitor(&self) -> (u16, u16) {
            (64, 64)
        }
    }
    struct Encoder {
        fake: Fake,
        drops: Arc<AtomicUsize>,
    }
    impl VideoEncoder for Encoder {
        fn encode(
            &mut self,
            frame: &RawFrame,
            params: &EncodeParams,
        ) -> Result<EncodedSample, Error> {
            self.fake.encode(frame, params)
        }
    }
    impl Drop for Encoder {
        fn drop(&mut self) {
            self.drops.fetch_add(1, Ordering::Relaxed);
        }
    }
    let boot = || {
        Core::start(CoreConfig {
            listen: Listen::Loopback,
            relay: RelaySettings::custom(vec![CustomRelay::new("http://127.0.0.1:9").unwrap()])
                .unwrap(),
            ..CoreConfig::default()
        })
        .unwrap()
    };
    let host = Arc::new(boot());
    let controller = boot();
    let polled = Arc::new(AtomicBool::new(false));
    host.set_capture(Box::new(Source {
        idle,
        polled: polled.clone(),
        shutdown: None,
    }))
    .unwrap();
    host.set_video_codecs(vnilink::VideoCodecCaps::h264_full(4096, 4096, "test"))
        .unwrap();
    controller
        .set_video_codecs(vnilink::VideoCodecCaps::h264_full(4096, 4096, "test"))
        .unwrap();
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
    host.start_hosting().unwrap();
    let parent = controller
        .begin_connect_services(&host.endpoint_id())
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while controller.controller_session(parent.id).unwrap().phase != SessionPhase::Active {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(10));
    }
    let parent = controller.controller_session(parent.id).unwrap();
    let owner = controller
        .attach_controller_service(parent.id, parent.generation, ServiceKind::Desktop)
        .unwrap();
    let handle = controller
        .open_desktop_service(owner.id, owner.generation)
        .unwrap();
    let remote = host
        .hosted_services()
        .unwrap()
        .into_iter()
        .find(|s| s.kind == ServiceKind::Desktop)
        .unwrap();
    let frames = handle.subscribe_video().unwrap();
    let stop = Arc::new(AtomicBool::new(false));
    let drops = Arc::new(AtomicUsize::new(0));
    let opens = Arc::new(Mutex::new(0));
    let (done_tx, done_rx) = std::sync::mpsc::channel();
    let worker_host = host.clone();
    let worker_stop = stop.clone();
    let worker_drops = drops.clone();
    let worker_opens = opens.clone();
    let worker_polled = polled.clone();
    let worker = std::thread::spawn(move || {
        let result = crate::serve_desktop_capture(
            &Source {
                idle,
                polled: worker_polled,
                shutdown: matches!(exit, CaptureExit::DuringRead)
                    .then(|| Arc::downgrade(&worker_host)),
            },
            &worker_host,
            remote.id,
            remote.generation,
            &worker_stop,
            move |w, h, fps, bps| {
                Ok(Encoder {
                    fake: Fake::open(&worker_opens, w, h, fps, bps)?,
                    drops: worker_drops.clone(),
                })
            },
        );
        done_tx.send(result).unwrap();
    });
    if idle || matches!(exit, CaptureExit::DuringRead) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !polled.load(Ordering::Acquire) {
            assert!(Instant::now() < deadline, "capture never polled");
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(frames.recv_timeout(Duration::from_millis(20)).is_none());
    } else {
        let picture = frames
            .recv_timeout(Duration::from_secs(5))
            .expect("scoped capture frame");
        assert!(parse_annex_b(&picture.payload).unwrap().has_idr());
    }
    match exit {
        CaptureExit::Owner => handle.close(),
        CaptureExit::Parent => controller.disconnect_session(parent.id).unwrap(),
        CaptureExit::Revoke => host
            .set_service_sharing(ServiceKind::Desktop, false)
            .unwrap(),
        CaptureExit::Stop => stop.store(true, Ordering::Release),
        CaptureExit::Shutdown => host.shutdown(),
        CaptureExit::DuringRead => {}
    }
    let result = done_rx.recv_timeout(Duration::from_secs(5));
    stop.store(true, Ordering::Relaxed);
    worker.join().unwrap();
    result
        .expect("owner close must stop capture without the caller stop flag")
        .unwrap();
    assert_eq!(drops.load(Ordering::Relaxed), *opens.lock().unwrap());
    if !matches!(
        exit,
        CaptureExit::Parent | CaptureExit::Shutdown | CaptureExit::DuringRead
    ) {
        assert_eq!(
            controller.controller_session(parent.id).unwrap().phase,
            SessionPhase::Active
        );
    }
}

fn session(capture: (u16, u16), quality: Quality, opens: &Arc<Mutex<usize>>) -> HostSession<Fake> {
    let opener = Arc::clone(opens);
    HostSession::open(capture.0, capture.1, quality, move |w, h, fps, bps| {
        Fake::open(&opener, w, h, fps, bps)
    })
    .unwrap()
}

#[test]
fn a_paced_picture_is_published_and_a_fast_one_is_skipped() {
    let opens = Arc::new(Mutex::new(0));
    let quality = Quality::new(500, 30, 32, 32).unwrap();
    let mut session = session((64, 64), quality, &opens);
    assert_eq!(session.capture_size(), (64, 64));
    assert_eq!(session.encoded_size(), (32, 32));
    assert_eq!(*opens.lock().unwrap(), 1);
    let mut sink = MemorySink::default();
    let first = session.publish(&solid(64, 64, 0), &mut sink, 7).unwrap();
    assert!(matches!(first, PumpOutput::Encoded(_)));
    assert_eq!(sink.ids, vec![7]);
    let parsed = parse_annex_b(&sink.frames[0].payload).unwrap();
    assert!(sink.frames[0].keyframe && parsed.has_sps() && parsed.has_pps() && parsed.has_idr());

    let skipped = session
        .publish(&solid(64, 64, 1_000), &mut sink, 7)
        .unwrap();
    assert_eq!(skipped, PumpOutput::Skipped);
    assert_eq!(sink.ids.len(), 1);

    session.request_recovery_keyframe();
    let forced = session
        .publish(&solid(64, 64, 2_000), &mut sink, 7)
        .unwrap();
    assert!(matches!(forced, PumpOutput::Encoded(_)));
    assert!(sink.frames[1].keyframe);
    assert!(matches!(
        session.publish(&solid(32, 32, 50_000), &mut sink, 7),
        Err(PublishError::Media(Error::InvalidFrame(
            "the frame size does not match the encoder"
        )))
    ));
}

#[test]
fn a_new_rate_retunes_the_running_encoder_without_a_keyframe() {
    let opens = Arc::new(Mutex::new(0));
    let taken = Arc::new(Mutex::new(Vec::new()));
    let opener = Arc::clone(&opens);
    let retunes = Arc::clone(&taken);
    let mut session = HostSession::open(
        64,
        64,
        Quality::new(500, 30, 32, 32).unwrap(),
        move |w, h, fps, bps| {
            let mut encoder = Fake::open(&opener, w, h, fps, bps)?;
            encoder.retunes = Some(Arc::clone(&retunes));
            Ok(encoder)
        },
    )
    .unwrap();
    let mut sink = MemorySink::default();
    session.publish(&solid(64, 64, 0), &mut sink, 7).unwrap();
    session
        .set_quality(Quality::new(900, 20, 32, 32).unwrap())
        .unwrap();
    assert_eq!(*taken.lock().unwrap(), vec![(900_000, 20)]);
    assert_eq!((*opens.lock().unwrap(), session.reopens()), (1, 0));
    session
        .publish(&solid(64, 64, 100_000), &mut sink, 7)
        .unwrap();
    assert!(!sink.frames[1].keyframe, "a retune forced a keyframe");
}

/// An encoder whose next picture comes out larger than a frame may be.
struct Swelling {
    too_large: Arc<Mutex<bool>>,
    retunes: Arc<Mutex<Vec<u32>>>,
    keyframes: Arc<Mutex<Vec<bool>>>,
}

impl VideoEncoder for Swelling {
    fn encode(&mut self, _frame: &RawFrame, params: &EncodeParams) -> Result<EncodedSample, Error> {
        if std::mem::take(&mut *self.too_large.lock().unwrap()) {
            return Err(Error::InvalidFrame(crate::error::FRAME_TOO_LARGE));
        }
        self.keyframes.lock().unwrap().push(params.force_keyframe);
        sample(params.force_keyframe)
    }

    fn retune(&mut self, bitrate_bps: u32, _frame_rate: u8) -> Result<bool, Error> {
        self.retunes.lock().unwrap().push(bitrate_bps);
        Ok(true)
    }
}

#[test]
fn a_frame_too_large_is_dropped_and_the_encoder_aims_lower() {
    let too_large = Arc::new(Mutex::new(false));
    let retunes = Arc::new(Mutex::new(Vec::new()));
    let keyframes = Arc::new(Mutex::new(Vec::new()));
    let (swell, taken, made) = (
        Arc::clone(&too_large),
        Arc::clone(&retunes),
        Arc::clone(&keyframes),
    );
    let mut session = HostSession::open(
        64,
        64,
        Quality::new(500, 30, 64, 64).unwrap(),
        move |_, _, _, _| {
            Ok(Swelling {
                too_large: Arc::clone(&swell),
                retunes: Arc::clone(&taken),
                keyframes: Arc::clone(&made),
            })
        },
    )
    .unwrap();
    let mut sink = MemorySink::default();
    session.publish(&solid(64, 64, 0), &mut sink, 7).unwrap();
    *too_large.lock().unwrap() = true;
    assert!(matches!(
        session.publish(&solid(64, 64, 100_000), &mut sink, 7),
        Ok(PumpOutput::Skipped)
    ));
    assert_eq!(*retunes.lock().unwrap(), vec![375_000]);
    session
        .publish(&solid(64, 64, 200_000), &mut sink, 7)
        .unwrap();
    assert_eq!(
        *keyframes.lock().unwrap(),
        vec![true, true],
        "the next picture was not a keyframe"
    );
    assert_eq!(
        sink.frames.len(),
        2,
        "the oversize frame reached the viewer"
    );
    // A second oversize frame aims lower again; a quality update restores it.
    *too_large.lock().unwrap() = true;
    session
        .publish(&solid(64, 64, 300_000), &mut sink, 7)
        .unwrap();
    assert_eq!(*retunes.lock().unwrap(), vec![375_000, 281_250]);
    session
        .set_quality(Quality::new(600, 30, 64, 64).unwrap())
        .unwrap();
    assert_eq!(retunes.lock().unwrap().last(), Some(&600_000));
}

#[test]
fn an_encoder_that_cannot_retune_is_opened_again() {
    let opens = Arc::new(Mutex::new(0));
    let mut session = session((64, 64), Quality::new(500, 30, 32, 32).unwrap(), &opens);
    session
        .set_quality(Quality::new(900, 30, 32, 32).unwrap())
        .unwrap();
    assert_eq!((*opens.lock().unwrap(), session.reopens()), (2, 1));
}

#[test]
fn a_size_change_reopens_the_encoder_through_the_session() {
    let opens = Arc::new(Mutex::new(0));
    let mut session = session((64, 64), Quality::new(500, 30, 32, 32).unwrap(), &opens);
    session
        .set_quality(Quality::new(500, 30, 16, 16).unwrap())
        .unwrap();
    assert_eq!(*opens.lock().unwrap(), 2);
    assert_eq!(session.encoded_size(), (16, 16));
    assert_eq!(session.reopens(), 1, "a reopen is counted for diagnostics");
    // The same settings again open nothing.
    session
        .set_quality(Quality::new(500, 30, 16, 16).unwrap())
        .unwrap();
    assert_eq!(session.reopens(), 1);
}

#[derive(Default)]
struct MemorySink {
    ids: Vec<u64>,
    frames: Vec<vnilink::EncodedFrame>,
}

impl FrameSink for MemorySink {
    fn publish_frame(
        &mut self,
        session_id: u64,
        frame: vnilink::EncodedFrame,
    ) -> Result<(), vnilink::Error> {
        self.ids.push(session_id);
        self.frames.push(frame);
        Ok(())
    }
}
