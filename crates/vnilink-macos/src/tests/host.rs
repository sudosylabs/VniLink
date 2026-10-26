use vnilink_media::Quality;

#[cfg(not(target_os = "macos"))]
use vnilink_media::Error;
#[cfg(target_os = "macos")]
use vnilink_media::{parse_annex_b, FrameSink, PumpOutput, RawFrame};

use crate::encode::MacEncoder;
use crate::host::HostSession;

#[cfg(not(target_os = "macos"))]
#[test]
fn the_host_encoder_is_unavailable() {
    let quality = Quality::new(500, 30, 64, 64).unwrap();
    assert!(matches!(
        HostSession::open(64, 64, quality, |w, h, fps, bps| MacEncoder::open(
            w, h, fps, bps, false
        )),
        Err(Error::Unsupported("VideoToolbox H.264"))
    ));
}

#[cfg(target_os = "macos")]
#[test]
fn the_platform_encoder_plugs_into_the_generic_session() {
    let quality = Quality::new(500, 30, 32, 32).unwrap();
    let mut session = HostSession::open(64, 64, quality, |w, h, fps, bps| {
        MacEncoder::open(w, h, fps, bps, false, vnilink::YuvMatrix::Bt709)
    })
    .unwrap();
    assert_eq!(session.capture_size(), (64, 64));
    assert_eq!(session.encoded_size(), (32, 32));
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
}

#[cfg(target_os = "macos")]
#[derive(Default)]
struct MemorySink {
    ids: Vec<u64>,
    frames: Vec<vnilink::EncodedFrame>,
}

#[cfg(target_os = "macos")]
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

#[cfg(target_os = "macos")]
fn solid(width: u16, height: u16, timestamp_us: u64) -> RawFrame {
    let mut pixels = Vec::with_capacity(usize::from(width) * usize::from(height) * 4);
    for _ in 0..usize::from(width) * usize::from(height) {
        pixels.extend_from_slice(&[0, 0, 255, 255]);
    }
    RawFrame::bgra(width, height, timestamp_us, pixels).unwrap()
}

#[cfg(target_os = "macos")]
mod twin {
    use super::MacEncoder;
    use crate::decode::MacDecoder;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use std::time::{Duration, Instant};
    use vnilink::{Core, CoreConfig, DisplayInfo, EncodedFrame, Listen, ServiceKind, SessionPhase};
    use vnilink_media::{serve_shared_capture, FrameSource, RawFrame, VideoDecoder};

    struct Tint {
        bgra: [u8; 4],
    }

    impl FrameSource for Tint {
        fn size(&self) -> (u16, u16) {
            (64, 64)
        }

        fn poll_frame(&self, timestamp_us: u64) -> Result<Option<RawFrame>, vnilink_media::Error> {
            let mut pixels = Vec::with_capacity(64 * 64 * 4);
            for _ in 0..64 * 64 {
                pixels.extend_from_slice(&self.bgra);
            }
            Ok(Some(RawFrame::bgra(64, 64, timestamp_us, pixels).unwrap()))
        }
    }

    fn boot() -> Core {
        Core::start(CoreConfig {
            listen: Listen::Loopback,
            relay: vnilink::RelaySettings::custom(vec![vnilink::CustomRelay::new(
                "http://127.0.0.1:9",
            )
            .unwrap()])
            .unwrap(),
            ..CoreConfig::default()
        })
        .unwrap()
    }

    fn pair(host: &Core, viewer: &Core) {
        viewer
            .accept_invitation(&host.create_invitation(Duration::from_secs(60)).unwrap())
            .unwrap();
        host.accept_invitation(&viewer.create_invitation(Duration::from_secs(60)).unwrap())
            .unwrap();
        let grants = vnilink::Grants {
            view: true,
            control: true,
            clipboard: false,
            transfer: false,
        };
        host.set_desktop_sharing(grants).unwrap();
        viewer.set_desktop_sharing(grants).unwrap();
    }

    struct IdleCapture;

    impl vnilink::HostCapture for IdleCapture {
        fn monitor(&self) -> (u16, u16) {
            (64, 64)
        }
    }

    fn twin(id: u64, x: i32) -> DisplayInfo {
        DisplayInfo {
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

    fn attach_desktop(viewer: &Core, host: &Core) -> vnilink::RemoteDesktopService {
        let parent = viewer.begin_connect_services(&host.endpoint_id()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while viewer.controller_session(parent.id).unwrap().phase != SessionPhase::Active {
            assert!(Instant::now() < deadline, "parent session did not connect");
            std::thread::sleep(Duration::from_millis(10));
        }
        let parent = viewer.controller_session(parent.id).unwrap();
        let owner = viewer
            .attach_controller_service(parent.id, parent.generation, ServiceKind::Desktop)
            .unwrap();
        viewer
            .open_desktop_service(owner.id, owner.generation)
            .unwrap()
    }

    fn dominant(frame: &EncodedFrame, decoder: &mut MacDecoder) -> [u8; 4] {
        let picture = decoder.decode(frame).unwrap();
        assert_eq!((picture.width(), picture.height()), (64, 64));
        let row = picture.row(32);
        [row[128], row[129], row[130], row[131]]
    }

    /// Two same-sized native sources (one red, one blue) stream through the
    /// real VideoToolbox codec with exact stamps; closing one capture leaves
    /// the sibling's decoded pictures untouched.
    #[test]
    fn twin_mac_sources_stream_distinct_stamped_pictures_independently() {
        let host = Arc::new(boot());
        let red_viewer = boot();
        let blue_viewer = boot();
        host.set_capture(Box::new(IdleCapture)).unwrap();
        for core in [&*host, &red_viewer, &blue_viewer] {
            core.set_video_codecs(vnilink::VideoCodecCaps::h264_full(4096, 4096, "test"))
                .unwrap();
        }
        host.set_topology(9, &[twin(1, 0), twin(2, 1920)]).unwrap();
        pair(&host, &red_viewer);
        pair(&host, &blue_viewer);
        host.start_hosting().unwrap();
        let red = attach_desktop(&red_viewer, &host);
        let blue = attach_desktop(&blue_viewer, &host);
        let deadline = Instant::now() + Duration::from_secs(5);
        while red.topology().ok().map(|d| d.len()) != Some(2)
            || blue.topology().ok().map(|d| d.len()) != Some(2)
        {
            assert!(Instant::now() < deadline, "topology did not propagate");
            std::thread::sleep(Duration::from_millis(10));
        }
        red.select_displays(&[1]).unwrap();
        blue.select_displays(&[2]).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while host.capture_displays().unwrap() != vec![1, 2] {
            assert!(Instant::now() < deadline, "selections did not propagate");
            std::thread::sleep(Duration::from_millis(10));
        }
        let red_frames = red.subscribe_video().unwrap();
        let blue_frames = blue.subscribe_video().unwrap();

        let stop_red = Arc::new(AtomicBool::new(false));
        let stop_blue = Arc::new(AtomicBool::new(false));
        let serve = |display: u64, bgra: [u8; 4], stop: Arc<AtomicBool>, host: Arc<Core>| {
            std::thread::spawn(move || {
                let result = serve_shared_capture(
                    &Tint { bgra },
                    &host,
                    &stop,
                    false,
                    display,
                    |_| {},
                    |_, width, height, fps, bps| {
                        assert_eq!((width, height), (64, 64));
                        assert!(fps > 0 && bps > 0);
                        let encoder = MacEncoder::open(
                            width,
                            height,
                            fps,
                            bps,
                            false,
                            vnilink::YuvMatrix::Bt709,
                        );
                        if let Err(error) = &encoder {
                            eprintln!("encoder open failed: {error:?}");
                        }
                        encoder
                    },
                );
                if let Err(error) = &result {
                    eprintln!("display {display} producer failed: {error:?}");
                }
                result
            })
        };
        let red_worker = serve(
            1,
            [0, 0, 255, 255],
            Arc::clone(&stop_red),
            Arc::clone(&host),
        );
        let blue_worker = serve(
            2,
            [255, 0, 0, 255],
            Arc::clone(&stop_blue),
            Arc::clone(&host),
        );

        let mut red_decoder = MacDecoder::new();
        let mut blue_decoder = MacDecoder::new();
        for _ in 0..2 {
            let frame = red_frames
                .recv_timeout(Duration::from_secs(10))
                .unwrap_or_else(|| {
                    panic!(
                        "red viewer frame; owners: {:?}; targets: {:?}",
                        host.hosted_services(),
                        host.video_targets()
                    )
                });
            assert_eq!((frame.display_id, frame.revision), (1, 9));
            let pixel = dominant(&frame, &mut red_decoder);
            assert!(
                pixel[2] > pixel[0] && pixel[2] > pixel[1] && pixel[2] > 128,
                "red viewer decoded {pixel:?}"
            );
            let frame = blue_frames
                .recv_timeout(Duration::from_secs(10))
                .expect("blue viewer frame");
            assert_eq!((frame.display_id, frame.revision), (2, 9));
            let pixel = dominant(&frame, &mut blue_decoder);
            assert!(
                pixel[0] > pixel[1] && pixel[0] > pixel[2] && pixel[0] > 128,
                "blue viewer decoded {pixel:?}"
            );
        }

        // Closing the red capture silences its viewer while blue pictures
        // keep their stamp and color.
        stop_red.store(true, Ordering::Relaxed);
        red_worker.join().unwrap().unwrap();
        let silence = Instant::now();
        while silence.elapsed() < Duration::from_millis(300) {
            if red_frames
                .recv_timeout(Duration::from_millis(100))
                .is_none()
            {
                break;
            }
        }
        assert!(
            red_frames
                .recv_timeout(Duration::from_millis(300))
                .is_none(),
            "closed capture still published"
        );
        // Drain the sibling's backlog, decoding to keep the VideoToolbox
        // chain alive, so the frames asserted below prove its worker is
        // still publishing rather than replaying its queue.
        let drain_until = Instant::now() + Duration::from_millis(500);
        while Instant::now() < drain_until {
            if let Some(frame) = blue_frames.recv_timeout(Duration::from_millis(100)) {
                blue_decoder
                    .decode(&frame)
                    .expect("drained sibling frame decodes");
            }
        }
        for _ in 0..2 {
            let frame = blue_frames
                .recv_timeout(Duration::from_secs(10))
                .expect("sibling stream after close");
            assert_eq!((frame.display_id, frame.revision), (2, 9));
            let pixel = dominant(&frame, &mut blue_decoder);
            assert!(
                pixel[0] > pixel[1] && pixel[0] > pixel[2] && pixel[0] > 128,
                "sibling color changed after close: {pixel:?}"
            );
        }
        stop_blue.store(true, Ordering::Relaxed);
        blue_worker.join().unwrap().unwrap();
    }
}
