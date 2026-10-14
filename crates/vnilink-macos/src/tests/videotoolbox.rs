use vnilink::EncodedFrame;
use vnilink_media::{
    parse_annex_b, EncodeParams, HostVideo, PumpOutput, Quality, VideoDecoder, VideoEncoder,
};

use crate::decode::MacDecoder;
use crate::encode::MacEncoder;

fn solid(
    width: u16,
    height: u16,
    timestamp_us: u64,
    b: u8,
    g: u8,
    r: u8,
) -> vnilink_media::RawFrame {
    let mut pixels = Vec::with_capacity(usize::from(width) * usize::from(height) * 4);
    for _ in 0..usize::from(width) * usize::from(height) {
        pixels.extend_from_slice(&[b, g, r, 255]);
    }
    vnilink_media::RawFrame::bgra(width, height, timestamp_us, pixels).unwrap()
}

#[test]
fn videotoolbox_encodes_and_decodes_a_synthetic_picture() {
    let mut encoder = MacEncoder::open(64, 64, 30, 500_000, false).unwrap();
    let params = EncodeParams {
        force_keyframe: true,
        bitrate_bps: 500_000,
        frame_rate: 30,
    };
    let sample = encoder
        .encode(&solid(64, 64, 0, 0, 0, 255), &params)
        .unwrap();
    assert!(sample.keyframe());
    let parsed = parse_annex_b(sample.bytes()).unwrap();
    assert!(parsed.has_sps() && parsed.has_pps() && parsed.has_idr());
    let encoded = EncodedFrame::h264(true, 0, sample.bytes().to_vec()).unwrap();
    let mut decoder = MacDecoder::new();
    let picture = decoder.decode(&encoded).unwrap();
    assert_eq!((picture.width(), picture.height()), (64, 64));
    let pixel = &picture.row(32)[32 * 4..32 * 4 + 4];
    assert!(
        pixel[2] > pixel[0] && pixel[2] > pixel[1] && pixel[2] > 128,
        "decoded picture is not the encoded color"
    );

    let delta = encoder
        .encode(
            &solid(64, 64, 33_333, 0, 0, 255),
            &EncodeParams {
                force_keyframe: false,
                bitrate_bps: 500_000,
                frame_rate: 30,
            },
        )
        .unwrap();
    let encoded_delta =
        EncodedFrame::h264(delta.keyframe(), 33_333, delta.bytes().to_vec()).unwrap();
    let next = decoder.decode(&encoded_delta).unwrap();
    assert_eq!((next.width(), next.height()), (64, 64));

    let junk = EncodedFrame::h264(false, 0, vec![0, 0, 0, 1, 0x41, 0x00]).unwrap();
    assert!(MacDecoder::new().decode(&junk).is_err());
}

#[test]
fn the_pump_accepts_a_videotoolbox_keyframe() {
    let quality = Quality::new(500, 30, 64, 64).unwrap();
    let encoder = MacEncoder::open(64, 64, 30, 500_000, false).unwrap();
    let mut video = HostVideo::new(quality, encoder);
    let output = video.push(&solid(64, 64, 0, 0, 255, 0)).unwrap();
    match output {
        PumpOutput::Encoded(frame) => assert!(frame.keyframe),
        PumpOutput::Skipped => panic!("the first picture must be encoded"),
    }
}

#[test]
fn videotoolbox_high_decodes_and_marks_profile_100() {
    let mut encoder = MacEncoder::open(64, 64, 30, 500_000, true).unwrap();
    let sample = encoder
        .encode(
            &solid(64, 64, 0, 0, 0, 255),
            &EncodeParams {
                force_keyframe: true,
                bitrate_bps: 500_000,
                frame_rate: 30,
            },
        )
        .unwrap();
    assert!(sample.keyframe());
    let parsed = parse_annex_b(sample.bytes()).unwrap();
    assert!(parsed.has_sps() && parsed.has_pps() && parsed.has_idr());
    let sps = parsed
        .units()
        .iter()
        .find(|unit| unit.nal_type == 7)
        .expect("SPS");
    assert_eq!(sps.bytes()[1], 100);
    let encoded = EncodedFrame::h264(true, 0, sample.bytes().to_vec()).unwrap();
    let picture = MacDecoder::new().decode(&encoded).unwrap();
    assert_eq!((picture.width(), picture.height()), (64, 64));
    let pixel = &picture.row(32)[32 * 4..32 * 4 + 4];
    assert!(
        pixel[2] > pixel[0] && pixel[2] > pixel[1] && pixel[2] > 128,
        "decoded picture is not the encoded color"
    );
}

#[test]
fn recording_remuxes_native_h264_without_reencoding() {
    use std::io::Cursor;
    use vnilink_media::{MatroskaRecorder, RecordingLimits};
    let mut encoder = MacEncoder::open(64, 64, 30, 500_000, false).unwrap();
    let mut recorder = None;
    for index in 0..60u64 {
        let time = index * 33_333;
        let sample = encoder
            .encode(
                &solid(64, 64, time, 0, 0, 255),
                &EncodeParams {
                    force_keyframe: index % 30 == 0,
                    bitrate_bps: 500_000,
                    frame_rate: 30,
                },
            )
            .unwrap();
        let frame = EncodedFrame::h264(sample.keyframe(), time, sample.bytes().to_vec()).unwrap();
        if index == 0 {
            recorder = Some(
                MatroskaRecorder::start(
                    Cursor::new(Vec::new()),
                    64,
                    64,
                    &frame,
                    RecordingLimits {
                        max_bytes: 8 * 1024 * 1024,
                        max_duration_ms: 10_000,
                    },
                )
                .unwrap(),
            );
        } else {
            recorder
                .as_mut()
                .unwrap()
                .push_video(&frame, time / 1000)
                .unwrap();
        }
    }
    let recorder = recorder.unwrap();
    assert_eq!(recorder.frames(), 60);
    let bytes = recorder.finish().unwrap().into_inner();
    assert!(bytes
        .windows(b"V_MPEG4/ISO/AVC".len())
        .any(|b| b == b"V_MPEG4/ISO/AVC"));
    let complete = vnilink_media::scan_recording_prefix(&mut std::io::Cursor::new(&bytes)).unwrap();
    let directory = std::env::temp_dir().join(format!(
        "vnilink-recover-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir(&directory).unwrap();
    let destination = directory.join("recovered.mkv");
    let partial = directory.join("recovered.mkv.part");
    // Truncate the last video block, not merely its final cue index.
    std::fs::write(&partial, &bytes[..complete.complete_bytes as usize - 8]).unwrap();
    let recovered = vnilink_media::recover_desktop_recording(&destination).unwrap();
    assert_eq!(recovered.clusters, 59);
    assert!(destination.is_file());
    assert!(!partial.exists());
    assert_eq!(
        std::fs::metadata(&destination).unwrap().len(),
        recovered.complete_bytes
    );
    let recovered_bytes = std::fs::read(&destination).unwrap();
    assert_eq!(
        f64::from_be_bytes(
            recovered_bytes
                [recovered.duration_position as usize..recovered.duration_position as usize + 8]
                .try_into()
                .unwrap()
        ),
        recovered.last_timestamp_ms as f64
    );
    // A pre-existing destination cannot be replaced by recovery.
    std::fs::write(&partial, b"preserve partial").unwrap();
    assert!(vnilink_media::recover_desktop_recording(&destination).is_err());
    assert_eq!(std::fs::read(&partial).unwrap(), b"preserve partial");
    if let Ok(path) = std::env::var("VNILINK_RECOVERED_FIXTURE") {
        std::fs::write(path, &recovered_bytes).unwrap();
    }
    std::fs::remove_dir_all(directory).unwrap();
    // Export only this synthetic fixture to an explicit temporary evidence path.
    if let Ok(path) = std::env::var("VNILINK_RECORDING_FIXTURE") {
        assert!(std::path::Path::new(&path).is_absolute());
        std::fs::write(path, bytes).unwrap();
    }
}

#[test]
fn recorder_worker_uses_independent_feed_and_preserves_desktop_after_stop() {
    use std::time::{Duration, Instant};
    use vnilink::{
        Core, CoreConfig, CustomRelay, Grants, Listen, RelaySettings, ServiceKind, SessionPhase,
    };
    use vnilink_media::{DesktopRecorder, RecordingLimits, RecordingPhase};
    fn boot() -> Core {
        let core = Core::start(CoreConfig {
            listen: Listen::Loopback,
            relay: RelaySettings::custom(vec![CustomRelay::new("http://127.0.0.1:9").unwrap()])
                .unwrap(),
            ..CoreConfig::default()
        })
        .unwrap();
        core.set_video_codecs(vnilink::VideoCodecCaps::h264_full(4096, 4096, "test"))
            .unwrap();
        core
    }
    #[track_caller]
    fn wait(mut test: impl FnMut() -> bool) {
        let until = Instant::now() + Duration::from_secs(8);
        while !test() {
            assert!(Instant::now() < until);
            std::thread::sleep(Duration::from_millis(2));
        }
    }
    struct Capture;
    impl vnilink::HostCapture for Capture {
        fn monitor(&self) -> (u16, u16) {
            (64, 64)
        }
    }
    let host = boot();
    let controller = boot();
    host.set_capture(Box::new(Capture)).unwrap();
    host.start_hosting().unwrap();
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
            view: true,
            control: false,
            clipboard: false,
            transfer: false,
        },
    )
    .unwrap();
    let parent = controller
        .begin_connect_services(&host.endpoint_id())
        .unwrap();
    wait(|| controller.controller_session(parent.id).unwrap().phase == SessionPhase::Active);
    let parent = controller.controller_session(parent.id).unwrap();
    let owner = controller
        .attach_controller_service(parent.id, parent.generation, ServiceKind::Desktop)
        .unwrap();
    let desktop = controller
        .open_desktop_service(owner.id, owner.generation)
        .unwrap();
    let viewer = desktop.subscribe_video().unwrap();
    let target = host.video_targets().unwrap().into_iter().next().unwrap();
    let folder = std::env::temp_dir().join(format!(
        "vnilink-recording-worker-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir(&folder).unwrap();
    let path = folder.join("test.mkv");
    let limits = RecordingLimits {
        max_bytes: 8 * 1024 * 1024,
        max_duration_ms: 10_000,
    };
    assert!(matches!(
        DesktopRecorder::start(&desktop, &path, limits),
        Err(vnilink_media::RecordingError::Source(
            vnilink::Error::Unauthorized
        ))
    ));
    assert_eq!(std::fs::read_dir(&folder).unwrap().count(), 0);
    host.authorize_recording(&controller.endpoint_id(), true)
        .unwrap();
    wait(|| desktop.recording_allowed().unwrap());
    // Capture is already encoding when a viewer starts a recording.
    let mut encoder = MacEncoder::open(64, 64, 30, 500_000, false).unwrap();
    let recorder = DesktopRecorder::start(
        &desktop,
        &path,
        RecordingLimits {
            max_bytes: 8 * 1024 * 1024,
            max_duration_ms: 10_000,
        },
    )
    .unwrap();
    for index in 0..30u64 {
        let time = index * 33_333;
        let sample = encoder
            .encode(
                &solid(64, 64, time, 0, 0, 255),
                &EncodeParams {
                    force_keyframe: index == 0,
                    bitrate_bps: 500_000,
                    frame_rate: 30,
                },
            )
            .unwrap();
        host.publish_desktop_video(
            target.id,
            target.generation,
            EncodedFrame::h264(sample.keyframe(), time, sample.bytes().to_vec()).unwrap(),
        )
        .unwrap();
        assert_eq!(
            viewer
                .recv_timeout(Duration::from_secs(2))
                .unwrap()
                .timestamp_us,
            time
        );
        wait(|| {
            let status = recorder.status();
            assert_ne!(
                status.phase,
                RecordingPhase::Failed,
                "recorder failed at frame {index}: {status:?}"
            );
            status.frames == index + 1
        });
    }
    recorder.stop();
    wait(|| {
        matches!(
            recorder.status().phase,
            RecordingPhase::Finished | RecordingPhase::Failed
        )
    });
    assert_eq!(recorder.status().phase, RecordingPhase::Finished);
    assert_eq!(recorder.status().frames, 30);
    assert!(folder.join("test.000001.mkv").is_file());
    assert!(!folder.join("test.000001.mkv.part").exists());
    let revoked_path = folder.join("revoked.mkv");
    let revoked = DesktopRecorder::start(&desktop, &revoked_path, limits).unwrap();
    let sample = encoder
        .encode(
            &solid(64, 64, 1_000_000, 0, 255, 0),
            &EncodeParams {
                force_keyframe: true,
                bitrate_bps: 500_000,
                frame_rate: 30,
            },
        )
        .unwrap();
    host.publish_desktop_video(
        target.id,
        target.generation,
        EncodedFrame::h264(sample.keyframe(), 1_000_000, sample.bytes().to_vec()).unwrap(),
    )
    .unwrap();
    assert_eq!(
        viewer
            .recv_timeout(Duration::from_secs(2))
            .unwrap()
            .timestamp_us,
        1_000_000
    );
    wait(|| revoked.status().frames == 1);
    host.authorize_recording(&controller.endpoint_id(), false)
        .unwrap();
    wait(|| !desktop.recording_allowed().unwrap());
    wait(|| revoked.status().phase == RecordingPhase::Finished);
    assert_eq!(revoked.status().frames, 1);
    assert!(folder.join("revoked.000001.mkv").is_file());
    assert!(!folder.join("revoked.000001.mkv.part").exists());
    let sample = encoder
        .encode(
            &solid(64, 64, 1_033_333, 0, 255, 0),
            &EncodeParams {
                force_keyframe: false,
                bitrate_bps: 500_000,
                frame_rate: 30,
            },
        )
        .unwrap();
    host.publish_desktop_video(
        target.id,
        target.generation,
        EncodedFrame::h264(sample.keyframe(), 1_033_333, sample.bytes().to_vec()).unwrap(),
    )
    .unwrap();
    assert_eq!(
        viewer
            .recv_timeout(Duration::from_secs(2))
            .unwrap()
            .timestamp_us,
        1_033_333
    );
    // A mid-stream encoder profile change rolls the recording to a new
    // segment instead of failing it; both segments stay self-contained.
    host.authorize_recording(&controller.endpoint_id(), true)
        .unwrap();
    wait(|| desktop.recording_allowed().unwrap());
    let roll = DesktopRecorder::start(
        &desktop,
        &folder.join("roll.mkv"),
        RecordingLimits {
            max_bytes: 8 * 1024 * 1024,
            max_duration_ms: 10_000,
        },
    )
    .unwrap();
    let mut high = MacEncoder::open(64, 64, 30, 500_000, true).unwrap();
    for index in 0..10u64 {
        let time = 2_000_000 + index * 33_333;
        let sample = encoder
            .encode(
                &solid(64, 64, time, 0, 0, 255),
                &EncodeParams {
                    force_keyframe: index == 0,
                    bitrate_bps: 500_000,
                    frame_rate: 30,
                },
            )
            .unwrap();
        host.publish_desktop_video(
            target.id,
            target.generation,
            EncodedFrame::h264(sample.keyframe(), time, sample.bytes().to_vec()).unwrap(),
        )
        .unwrap();
        assert_eq!(
            viewer
                .recv_timeout(Duration::from_secs(2))
                .unwrap()
                .timestamp_us,
            time
        );
        wait(|| roll.status().frames == index + 1);
    }
    for index in 0..10u64 {
        let time = 3_000_000 + index * 33_333;
        let sample = high
            .encode(
                &solid(64, 64, time, 0, 255, 0),
                &EncodeParams {
                    force_keyframe: index == 0,
                    bitrate_bps: 500_000,
                    frame_rate: 30,
                },
            )
            .unwrap();
        host.publish_desktop_video(
            target.id,
            target.generation,
            EncodedFrame::h264(sample.keyframe(), time, sample.bytes().to_vec()).unwrap(),
        )
        .unwrap();
        assert_eq!(
            viewer
                .recv_timeout(Duration::from_secs(2))
                .unwrap()
                .timestamp_us,
            time
        );
        wait(|| roll.status().frames == 10 + index + 1);
    }
    wait(|| roll.status().segments == 1);
    roll.stop();
    wait(|| {
        matches!(
            roll.status().phase,
            RecordingPhase::Finished | RecordingPhase::Failed
        )
    });
    assert_eq!(roll.status().phase, RecordingPhase::Finished);
    assert_eq!((roll.status().frames, roll.status().segments), (20, 2));
    let final_bytes: u64 = ["roll.000001.mkv", "roll.000002.mkv"]
        .iter()
        .map(|name| std::fs::metadata(folder.join(name)).unwrap().len())
        .sum();
    assert_eq!(roll.status().bytes, final_bytes);
    for (name, frames) in [("roll.000001.mkv", 10), ("roll.000002.mkv", 10)] {
        let bytes = std::fs::read(folder.join(name)).unwrap();
        assert_eq!(
            vnilink_media::scan_recording_prefix(&mut std::io::Cursor::new(&bytes))
                .unwrap()
                .clusters,
            frames
        );
    }
    assert_eq!(
        controller.controller_session(parent.id).unwrap().phase,
        SessionPhase::Active
    );
    assert!(DesktopRecorder::start(
        &desktop,
        &path,
        RecordingLimits {
            max_bytes: 8 * 1024 * 1024,
            max_duration_ms: 10_000
        }
    )
    .is_err());
    desktop.close();
    controller.shutdown();
    host.shutdown();
    std::fs::remove_dir_all(folder).unwrap();
}

#[test]
fn hevc_probe_and_negotiated_encoder_process_real_synthetic_frames() {
    let support = crate::probe_hevc();
    assert!(
        support.encode && support.decode,
        "VideoToolbox HEVC roundtrip failed"
    );
    let mut encoder = crate::MacVideoEncoder::open(64, 64, 30, 500_000, 3).unwrap();
    let mut decoder = crate::MacDecoder::new();
    for i in 0..3 {
        let raw = solid(64, 64, i * 33333, 32, 64, 128);
        let sample = encoder
            .encode(
                &raw,
                &EncodeParams {
                    force_keyframe: i == 0,
                    bitrate_bps: 500_000,
                    frame_rate: 30,
                },
            )
            .unwrap();
        assert_eq!(sample.codec(), vnilink::VideoCodec::H265);
        let frame = EncodedFrame::new(
            sample.codec(),
            sample.keyframe(),
            raw.timestamp_us(),
            sample.bytes().to_vec(),
        )
        .unwrap();
        let decoded = decoder.decode(&frame).unwrap();
        assert_eq!((decoded.width(), decoded.height()), (64, 64));
        assert!(decoded.pixels()[2].abs_diff(128) < 20);
    }
}
