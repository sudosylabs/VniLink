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
        recovery: false,
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
                recovery: false,
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

/// The SPS says BT.709 limited range, and VideoToolbox converts the BGRA
/// pictures with that matrix: saturated colours come back as they went in.
/// Tagged BT.601 instead, pure red comes back 9 low (measured 2026-10-22).
#[test]
fn videotoolbox_colours_match_the_stream_signal() {
    let params = EncodeParams {
        force_keyframe: true,
        recovery: false,
        bitrate_bps: 8_000_000,
        frame_rate: 30,
    };
    // (b, g, r)
    for colour in [
        [0, 0, 255],
        [0, 255, 0],
        [255, 0, 0],
        [0, 255, 255],
        [255, 255, 0],
        [255, 0, 255],
        [0, 128, 255],
        [128, 128, 128],
    ] {
        let mut encoder = MacEncoder::open(64, 64, 30, 8_000_000, false).unwrap();
        let picture = solid(64, 64, 0, colour[0], colour[1], colour[2]);
        let sample = encoder.encode(&picture, &params).unwrap();
        let parsed = parse_annex_b(sample.bytes()).unwrap();
        let sps = parsed
            .units()
            .iter()
            .find(|unit| unit.nal_type == 7)
            .unwrap();
        assert_eq!(
            vnilink_media::h264_video_signal(sps.bytes()).unwrap(),
            Some(vnilink_media::STREAM_SIGNAL)
        );
        let encoded = EncodedFrame::h264(true, 0, sample.bytes().to_vec()).unwrap();
        let decoded = MacDecoder::new().decode(&encoded).unwrap();
        let pixel = &decoded.row(32)[32 * 4..32 * 4 + 3];
        for channel in 0..3 {
            let drift = (i16::from(pixel[channel]) - i16::from(colour[channel])).abs();
            assert!(drift <= 6, "{colour:?} came back as {pixel:?}");
        }
    }
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
                recovery: false,
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
                    recovery: false,
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
    use vnilink_media::{Recorder, RecordingLimits, RecordingPhase};
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
    host.set_desktop_sharing(Grants {
        view: true,
        control: false,
        clipboard: false,
        transfer: false,
    })
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
        Recorder::start(&desktop, &path, limits),
        Err(vnilink_media::RecordingError::Source(
            vnilink::Error::Unauthorized
        ))
    ));
    assert_eq!(std::fs::read_dir(&folder).unwrap().count(), 0);
    host.set_recording_sharing(true).unwrap();
    wait(|| desktop.recording_allowed().unwrap());
    // Capture is already encoding when a viewer starts a recording.
    let mut encoder = MacEncoder::open(64, 64, 30, 500_000, false).unwrap();
    let recorder = Recorder::start(
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
                    recovery: false,
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
    let revoked = Recorder::start(&desktop, &revoked_path, limits).unwrap();
    let sample = encoder
        .encode(
            &solid(64, 64, 1_000_000, 0, 255, 0),
            &EncodeParams {
                force_keyframe: true,
                recovery: false,
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
    host.set_recording_sharing(false).unwrap();
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
                recovery: false,
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
    host.set_recording_sharing(true).unwrap();
    wait(|| desktop.recording_allowed().unwrap());
    let roll = Recorder::start(
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
                    recovery: false,
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
                    recovery: false,
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
    assert!(Recorder::start(
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
                    recovery: false,
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

/// A running VideoToolbox session takes a new bitrate and frame rate, and
/// the pictures after it decode on from the same keyframe.
#[test]
fn videotoolbox_takes_a_new_rate_without_a_keyframe() {
    let mut encoder = MacEncoder::open(64, 64, 30, 500_000, false).unwrap();
    let params = |force_keyframe, bitrate_bps, frame_rate| EncodeParams {
        force_keyframe,
        recovery: false,
        bitrate_bps,
        frame_rate,
    };
    let first = encoder
        .encode(&solid(64, 64, 0, 255, 0, 0), &params(true, 500_000, 30))
        .unwrap();
    let mut decoder = MacDecoder::new();
    decoder
        .decode(&EncodedFrame::h264(true, 0, first.bytes().to_vec()).unwrap())
        .unwrap();
    assert!(encoder.retune(2_000_000, 60).unwrap());
    for index in 1..4u64 {
        let sample = encoder
            .encode(
                &solid(64, 64, index * 16_667, 255, 0, 0),
                &params(false, 2_000_000, 60),
            )
            .unwrap();
        assert!(
            !sample.keyframe(),
            "picture {index} after the retune was a keyframe"
        );
        decoder
            .decode(&EncodedFrame::h264(false, index * 16_667, sample.bytes().to_vec()).unwrap())
            .unwrap();
    }
    assert!(!encoder.retune(0, 60).unwrap(), "a zero bitrate was taken");
}

fn page(width: u16, height: u16, index: u64) -> vnilink_media::RawFrame {
    vnilink_media::measure::text_page(width, height, index)
}

/// Sizes of the forced keyframes in a stream that asks for one every
/// `every` pictures after the first, and how many pictures the encoder
/// dropped to repay their bytes. A dropped keyframe stays owed, as in the
/// pump. Every picture the encoder made is decoded.
fn forced_keyframes(
    width: u16,
    height: u16,
    bitrate_bps: u32,
    recovery: bool,
    every: u64,
    count: u64,
) -> (Vec<usize>, u64) {
    let mut encoder = MacEncoder::open(width, height, 30, bitrate_bps, true).unwrap();
    let mut decoder = MacDecoder::new();
    let (mut sizes, mut dropped, mut owed) = (Vec::new(), 0, false);
    for index in 0..every * (count + 1) {
        let force = owed || index % every == 0;
        let params = EncodeParams {
            force_keyframe: force,
            recovery: recovery && force && index > 0,
            bitrate_bps,
            frame_rate: 30,
        };
        match encoder.encode(&page(width, height, index), &params) {
            Ok(sample) => {
                owed = false;
                if force && index > 0 {
                    assert!(sample.keyframe(), "picture {index} was not a keyframe");
                    sizes.push(sample.bytes().len());
                }
                let frame =
                    EncodedFrame::h264(sample.keyframe(), index * 33_333, sample.bytes().to_vec())
                        .unwrap();
                decoder.decode(&frame).unwrap();
            }
            Err(error) if error.is_dropped() => {
                owed = force;
                dropped += 1;
            }
            Err(error) => panic!("picture {index}: {error}"),
        }
    }
    (sizes, dropped)
}

/// A recovery keyframe comes out smaller than a full one of the same
/// screen, still decodes, and the stream after it decodes on.
#[test]
fn a_recovery_keyframe_is_smaller_than_a_full_one() {
    let (full, _) = forced_keyframes(1280, 720, 2_000_000, false, 15, 3);
    let (recovery, _) = forced_keyframes(1280, 720, 2_000_000, true, 15, 3);
    let (full, recovery) = (full.iter().max().unwrap(), recovery.iter().max().unwrap());
    assert!(
        recovery * 3 < full * 2,
        "recovery keyframes reached {recovery} bytes against {full} for a full one"
    );
}

/// Prints forced-keyframe sizes, full against recovery, for the V4 record:
/// `cargo test -p vnilink-macos --release --lib recovery_keyframe_sizes -- --ignored --nocapture`.
#[test]
#[ignore = "measurement"]
fn recovery_keyframe_sizes() {
    for (width, height) in [(1280, 720), (1920, 1080)] {
        for bitrate_bps in [2_000_000, 8_000_000] {
            let budget = vnilink_media::RecoveryQp::budget(&EncodeParams {
                force_keyframe: true,
                recovery: true,
                bitrate_bps,
                frame_rate: 30,
            });
            for recovery in [false, true] {
                let (sizes, dropped) =
                    forced_keyframes(width, height, bitrate_bps, recovery, 30, 8);
                println!(
                    "{width}x{height} {} kb/s {}: budget {budget} B, forced keyframes {sizes:?} B, pictures dropped {dropped}",
                    bitrate_bps / 1_000,
                    if recovery { "recovery" } else { "full" },
                );
            }
        }
    }
}

/// Saves decoded pictures of a 720p 2 Mb/s stream, to judge by eye what a
/// recovery keyframe costs in legibility: the full keyframe at picture 60,
/// and in a stream whose keyframes after the first are recovery ones, the
/// recovery keyframe at 60 and the pictures 5 and 15 after it. Written as
/// PPM to `$VNILINK_PICTURES` (default `target/recovery-pictures`):
/// `cargo test -p vnilink-macos --release --lib recovery_keyframe_pictures -- --ignored --nocapture`.
#[test]
#[ignore = "measurement"]
fn recovery_keyframe_pictures() {
    let directory = std::path::PathBuf::from(
        std::env::var("VNILINK_PICTURES").unwrap_or_else(|_| "target/recovery-pictures".into()),
    );
    std::fs::create_dir_all(&directory).unwrap();
    for recovery in [false, true] {
        let mut encoder = MacEncoder::open(1280, 720, 30, 2_000_000, true).unwrap();
        let mut decoder = MacDecoder::new();
        let mut owed = false;
        for index in 0..76u64 {
            let force = owed || index % 30 == 0;
            let params = EncodeParams {
                force_keyframe: force,
                recovery: recovery && force && index > 0,
                bitrate_bps: 2_000_000,
                frame_rate: 30,
            };
            let sample = match encoder.encode(&page(1280, 720, index), &params) {
                Ok(sample) => sample,
                Err(error) if error.is_dropped() => {
                    owed = force;
                    continue;
                }
                Err(error) => panic!("picture {index}: {error}"),
            };
            owed = false;
            let frame =
                EncodedFrame::h264(sample.keyframe(), index * 33_333, sample.bytes().to_vec())
                    .unwrap();
            let picture = decoder.decode(&frame).unwrap();
            let name = match (recovery, index) {
                (false, 60) => "full-keyframe",
                (true, 60) => "recovery-keyframe",
                (true, 65) => "recovery-plus-5",
                (true, 75) => "recovery-plus-15",
                _ => continue,
            };
            let path = directory.join(format!("{name}.ppm"));
            let mut bytes =
                format!("P6\n{} {}\n255\n", picture.width(), picture.height()).into_bytes();
            for y in 0..picture.height() {
                for bgra in picture.row(y)[..usize::from(picture.width()) * 4].chunks_exact(4) {
                    bytes.extend_from_slice(&[bgra[2], bgra[1], bgra[0]]);
                }
            }
            std::fs::write(&path, bytes).unwrap();
            println!("{name}: {} B -> {}", sample.bytes().len(), path.display());
        }
    }
}

/// Whether the encoder refines a still screen after a keyframe: the same
/// page is encoded for two seconds after a full or a recovery keyframe, and
/// each decoded picture is compared with the page (PSNR over RGB). Prints
/// sizes and PSNR every ten pictures:
/// `cargo test -p vnilink-macos --release --lib still_screen_refinement -- --ignored --nocapture`.
#[test]
#[ignore = "measurement"]
fn still_screen_refinement() {
    let psnr = vnilink_media::measure::psnr;
    for recovery in [false, true] {
        let mut encoder = MacEncoder::open(1280, 720, 30, 2_000_000, true).unwrap();
        let mut decoder = MacDecoder::new();
        let still = page(1280, 720, 0);
        let mut line = Vec::new();
        for index in 0..91u64 {
            // A full first keyframe, then the keyframe under test at 30.
            let force = index == 0 || index == 30;
            let params = EncodeParams {
                force_keyframe: force,
                recovery: recovery && index == 30,
                bitrate_bps: 2_000_000,
                frame_rate: 30,
            };
            let frame =
                vnilink_media::RawFrame::bgra(1280, 720, index * 33_333, still.pixels().to_vec())
                    .unwrap();
            let Ok(sample) = encoder.encode(&frame, &params) else {
                line.push(format!("{index}:dropped"));
                continue;
            };
            let encoded =
                EncodedFrame::h264(sample.keyframe(), index * 33_333, sample.bytes().to_vec())
                    .unwrap();
            let picture = decoder.decode(&encoded).unwrap();
            if index >= 30 && (index - 30) % 10 == 0 || index == 31 {
                line.push(format!(
                    "+{}: {} B {:.1} dB",
                    index - 30,
                    sample.bytes().len(),
                    psnr(&still, &picture)
                ));
            }
        }
        println!(
            "{}: {}",
            if recovery { "recovery" } else { "full" },
            line.join(", ")
        );
    }
}

/// Coloured text through VideoToolbox H.264 (4:2:0) against its source:
/// how close the product's own encoder comes to the 4:2:0 ceiling that
/// `vnilink-media`'s `chroma_measure` example finds with no codec.
/// `cargo test -p vnilink-macos --release --lib chroma_quality -- --ignored --nocapture`.
#[test]
#[ignore = "measurement"]
fn chroma_quality() {
    use vnilink_media::measure::{chroma_psnr, colour_text_page, edge_psnr, psnr};
    println!("| Size | Target Mb/s | Sent kB/s | Key PSNR | Key chroma | Key edge | Last PSNR | Last chroma | Last edge |");
    println!("|---|---|---|---|---|---|---|---|---|");
    for (width, height) in [(1280u16, 720u16), (1920, 1080)] {
        for megabits in [1u32, 2, 4, 8] {
            let bitrate = megabits * 1_000_000;
            let mut encoder = MacEncoder::open(width, height, 30, bitrate, false).unwrap();
            let mut decoder = MacDecoder::new();
            let (mut bytes, mut key, mut last) = (0usize, None, None);
            for index in 0..60u64 {
                let params = EncodeParams {
                    force_keyframe: index == 0,
                    recovery: false,
                    bitrate_bps: bitrate,
                    frame_rate: 30,
                };
                let sample = match encoder.encode(&colour_text_page(width, height, index), &params)
                {
                    Ok(sample) => sample,
                    Err(error) if error.is_dropped() => continue,
                    Err(error) => panic!("picture {index}: {error}"),
                };
                bytes += sample.bytes().len();
                let frame =
                    EncodedFrame::h264(sample.keyframe(), index * 33_333, sample.bytes().to_vec())
                        .unwrap();
                let picture = decoder.decode(&frame).unwrap();
                if key.is_none() {
                    key = Some(picture.clone());
                }
                last = Some((index, picture));
            }
            let key = key.unwrap();
            let (index, last) = last.unwrap();
            let (source_key, source_last) = (
                colour_text_page(width, height, 0),
                colour_text_page(width, height, index),
            );
            println!(
                "| {width}x{height} | {megabits} | {:.0} | {:.1} | {:.1} | {:.1} | {:.1} | {:.1} | {:.1} |",
                bytes as f64 / 1000.0 / 2.0,
                psnr(&source_key, &key),
                chroma_psnr(&source_key, &key),
                edge_psnr(&source_key, &key),
                psnr(&source_last, &last),
                chroma_psnr(&source_last, &last),
                edge_psnr(&source_last, &last),
            );
        }
    }
}
