use std::sync::{Arc, Mutex};

use crate::annexb::{to_annex_b, AvccAccessUnit};
use crate::error::{Error, PublishError};
use crate::frame::RawFrame;
use crate::host::HostSession;
use crate::pump::{EncodeParams, EncodedSample, FrameSink, PumpOutput, VideoEncoder};
use crate::{parse_annex_b, Quality};
use vnilink::CoreEvent;

struct Fake {
    width: u16,
    height: u16,
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
        Ok(Self { width, height })
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
}

fn sample(keyframe: bool) -> Result<EncodedSample, Error> {
    let picture: &[u8] = if keyframe {
        &[0x65, 0x10]
    } else {
        &[0x41, 0x10]
    };
    let mut avcc = (picture.len() as u32).to_be_bytes().to_vec();
    avcc.extend_from_slice(picture);
    let sets = [vec![0x67, 0x42], vec![0x68, 0xCE]];
    let annex = to_annex_b(&AvccAccessUnit {
        nal_length_size: 4,
        avcc: &avcc,
        parameter_sets: &sets,
        keyframe,
    })?;
    EncodedSample::annex_b(keyframe, annex)
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
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::time::{Duration, Instant};
    use vnilink::{
        Core, CoreConfig, CustomRelay, Grants, Listen, RelaySettings, ServiceKind, SessionPhase,
    };
    struct Source;
    impl crate::FrameSource for Source {
        fn size(&self) -> (u16, u16) {
            (64, 64)
        }
        fn next_frame(&self, timestamp: u64) -> Result<RawFrame, Error> {
            Ok(solid(64, 64, timestamp))
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
    host.set_capture(Box::new(Source)).unwrap();
    host.set_h264_support(true, true).unwrap();
    controller.set_h264_support(true, true).unwrap();
    controller
        .accept_invitation(&host.create_invitation(Duration::from_secs(60)).unwrap())
        .unwrap();
    host.accept_invitation(
        &controller
            .create_invitation(Duration::from_secs(60))
            .unwrap(),
    )
    .unwrap();
    host.authorize(
        &controller.endpoint_id(),
        Grants {
            control: true,
            clipboard: false,
            transfer: false,
        },
    )
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
    let worker = std::thread::spawn(move || {
        let result = crate::serve_desktop_capture(
            &Source,
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
    let picture = frames
        .recv_timeout(Duration::from_secs(5))
        .expect("scoped capture frame");
    assert!(parse_annex_b(&picture.payload).unwrap().has_idr());
    handle.close();
    let result = done_rx.recv_timeout(Duration::from_secs(5));
    stop.store(true, Ordering::Relaxed);
    worker.join().unwrap();
    result
        .expect("owner close must stop capture without the caller stop flag")
        .unwrap();
    assert_eq!(drops.load(Ordering::Relaxed), *opens.lock().unwrap());
    assert_eq!(
        controller.controller_session(parent.id).unwrap().phase,
        SessionPhase::Active
    );
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

    session
        .observe(&CoreEvent::KeyframeRequested { id: 7 })
        .unwrap();
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
fn a_size_change_reopens_the_encoder_through_the_session() {
    let opens = Arc::new(Mutex::new(0));
    let mut session = session((64, 64), Quality::new(500, 30, 32, 32).unwrap(), &opens);
    session
        .observe(&CoreEvent::QualityUpdated {
            id: 7,
            quality: Quality::new(500, 30, 16, 16).unwrap(),
        })
        .unwrap();
    assert_eq!(*opens.lock().unwrap(), 2);
    assert_eq!(session.encoded_size(), (16, 16));
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

#[test]
fn only_matching_sessions_reach_the_serve_loop() {
    use crate::host::event_session;

    // Ownerless events are observed by every serve loop.
    assert_eq!(
        event_session(&CoreEvent::HostingChanged { hosting: true }),
        None
    );
    assert_eq!(
        event_session(&CoreEvent::DevicePaired {
            endpoint_id: "peer".to_string(),
        }),
        None
    );
    // Owned events carry their session.
    let quality = Quality::new(500, 30, 32, 32).unwrap();
    assert_eq!(
        event_session(&CoreEvent::QualityUpdated { id: 7, quality }),
        Some(7)
    );
    assert_eq!(
        event_session(&CoreEvent::KeyframeRequested { id: 9 }),
        Some(9)
    );
    assert_eq!(
        event_session(&CoreEvent::SessionChanged {
            id: 3,
            generation: 1,
            role: vnilink::SessionRole::Host,
            phase: vnilink::SessionPhase::Active,
            peer_id: "peer".to_string(),
        }),
        Some(3)
    );
}
