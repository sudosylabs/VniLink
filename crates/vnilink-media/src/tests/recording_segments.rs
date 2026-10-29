//! Segmented recording without capture hardware. The H.264 fixtures are real
//! VideoToolbox output captured once on macOS (64x64 and 128x128 IDR frames
//! plus a 64x64 P frame); the remuxer never parses slice content, so large
//! filler NALs exercise byte accounting identically.
use crate::{
    scan_recording_prefix, RecordingError, RecordingFailure, RecordingLimits, RecordingRetention,
    SegmentWriter,
};
use std::path::PathBuf;
use vnilink::EncodedFrame;

// Real 64x64 Baseline IDR: SPS + PPS + SEI + IDR.
const IDR64: &str = "000000012742c00dab4186f020c2280000000128ce3c800000000106053247564adc5c4c433f94efc5113cd143a8010000030001030000030001020007a1200b000003000003000041140c038928010dffffffff800000000125b820013fffff8228a00020a7e3800081dc70001170befbe78251418e0006e38000811df7df7df7df50495d75d304b5d75d75d75d412575d74c12d75d75d75d750495d75d304b5d75d75d75cd8a0c7000371c000408ed6d6d6baebaebaebaebaebaebaebaebaebaf0";
// Real 64x64 P frame: one type-1 NAL.
const P64: &str = "0000000121e1040578cff5517feaa2ffd545ffa046a2ffd545fc";
// Real 128x128 Baseline IDR: different SPS, same PPS as IDR64.
const IDR128: &str = "000000012742c00dab41823c10e80000000128ce3c800000000106053247564adc5c4c433f94efc5113cd143a8010000030001030000030001020007a1200b0000030000030000411e0c038928010dffffffff800000000125b82003fffffe08a2800081078e00021b71c000441efbefbefbeff0fffc11029147c70063837df7d414575d75d75d75d74c14d75d750515d75d75d75d30535d75d414575d75d75d74c14d75d750515d75d75d75d30535d75d414575d75d75d74c14d75d750515d75d75d75d30535d75d414575d75d75d74c14d75d750515d75d75d75d30535d75d78";

fn unhex(hex: &str) -> Vec<u8> {
    assert_eq!(hex.len() % 2, 0);
    (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
        .collect()
}

fn idr64(timestamp_us: u64) -> EncodedFrame {
    EncodedFrame::h264(true, timestamp_us, unhex(IDR64)).unwrap()
}

fn idr128(timestamp_us: u64) -> EncodedFrame {
    EncodedFrame::h264(true, timestamp_us, unhex(IDR128)).unwrap()
}

fn p64(timestamp_us: u64) -> EncodedFrame {
    EncodedFrame::h264(false, timestamp_us, unhex(P64)).unwrap()
}

/// Type-1 NAL of 0xFF filler: no start codes, never valid picture content,
/// but the remuxer only frames NALs, so byte accounting matches exactly.
fn fat_p(timestamp_us: u64, size: usize) -> EncodedFrame {
    let mut payload = vec![0, 0, 0, 1, 0x41];
    payload.extend(std::iter::repeat_n(0xFF, size - payload.len()));
    EncodedFrame::h264(false, timestamp_us, payload).unwrap()
}

/// The real 64x64 IDR with its final NAL padded by 0xFF filler. Slice bytes
/// are opaque to the remuxer; every frame primes, so byte rolls never skip.
fn fat_idr(timestamp_us: u64, size: usize) -> EncodedFrame {
    let mut payload = unhex(IDR64);
    payload.extend(std::iter::repeat_n(0xFF, size - payload.len()));
    EncodedFrame::h264(true, timestamp_us, payload).unwrap()
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "vnilink-segments-{}-{}-{}",
        name,
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir(&dir).unwrap();
    dir
}

fn limits() -> RecordingLimits {
    RecordingLimits {
        max_bytes: 1024 * 1024,
        max_duration_ms: 10_000,
    }
}

fn finished(dir: &std::path::Path) -> Vec<PathBuf> {
    let mut found: Vec<PathBuf> = std::fs::read_dir(dir)
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "mkv"))
        .collect();
    found.sort();
    found
}

fn scanned_clusters(path: &std::path::Path) -> u64 {
    let bytes = std::fs::read(path).unwrap();
    scan_recording_prefix(&mut std::io::Cursor::new(&bytes))
        .unwrap()
        .clusters
}

#[test]
fn byte_limit_rolls_to_numbered_segments_without_losing_frames() {
    let dir = scratch("bytes");
    let mut writer = SegmentWriter::create(&dir.join("talk.mkv"), limits(), None).unwrap();
    writer.push(&fat_idr(0, 128 * 1024)).unwrap();
    for index in 1..=20u64 {
        writer.push(&fat_idr(index * 33_333, 128 * 1024)).unwrap();
    }
    let snapshot = writer.snapshot();
    assert_eq!(snapshot.frames, 21);
    assert!(snapshot.segments >= 2);
    writer.finish().unwrap();
    let files = finished(&dir);
    assert_eq!(files.len() as u32, snapshot.segments + 1);
    assert_eq!(
        files[0].file_name().unwrap().to_str().unwrap(),
        "talk.000001.mkv"
    );
    let clusters: u64 = files.iter().map(|f| scanned_clusters(f)).sum();
    assert_eq!(clusters, 21);
    assert!(
        std::fs::read_dir(&dir).unwrap().flatten().all(|e| !e
            .file_name()
            .to_str()
            .unwrap()
            .ends_with(".part")),
        "no partials remain after finish"
    );
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn format_change_primes_the_next_segment_without_loss() {
    let dir = scratch("format");
    let mut writer = SegmentWriter::create(&dir.join("talk.mkv"), limits(), None).unwrap();
    writer.push(&idr64(0)).unwrap();
    writer.push(&p64(33_333)).unwrap();
    // New SPS ends segment one; the same frame opens segment two.
    writer.push(&idr128(66_666)).unwrap();
    writer.push(&idr128(99_999)).unwrap();
    let snapshot = writer.snapshot();
    assert_eq!((snapshot.frames, snapshot.segments), (4, 1));
    writer.finish().unwrap();
    let files = finished(&dir);
    assert_eq!(files.len(), 2);
    assert_eq!(scanned_clusters(&files[0]), 2);
    assert_eq!(scanned_clusters(&files[1]), 2);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn duration_limit_rolls_and_skips_mid_gop_until_the_next_idr() {
    let dir = scratch("duration");
    let mut writer = SegmentWriter::create(
        &dir.join("talk.mkv"),
        RecordingLimits {
            max_bytes: 8 * 1024 * 1024,
            max_duration_ms: 1000,
        },
        None,
    )
    .unwrap();
    writer.push(&idr64(0)).unwrap();
    for index in 1..=11u64 {
        writer.push(&p64(index * 100_000)).unwrap();
    }
    // The 1100ms P frame rolls segment one and is skipped; this IDR primes two.
    writer.push(&idr64(1_200_000)).unwrap();
    let snapshot = writer.snapshot();
    assert_eq!((snapshot.frames, snapshot.segments), (12, 1));
    writer.finish().unwrap();
    let files = finished(&dir);
    assert_eq!(files.len(), 2);
    assert_eq!(scanned_clusters(&files[0]), 11);
    assert_eq!(scanned_clusters(&files[1]), 1);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn retention_by_count_keeps_the_newest_segments() {
    let dir = scratch("retention-count");
    let mut writer = SegmentWriter::create(
        &dir.join("talk.mkv"),
        RecordingLimits {
            max_bytes: 8 * 1024 * 1024,
            max_duration_ms: 1000,
        },
        Some(RecordingRetention {
            max_segments: 2,
            max_total_bytes: 1024 * 1024 * 1024,
        }),
    )
    .unwrap();
    writer.push(&idr64(0)).unwrap();
    writer.push(&idr64(1_100_000)).unwrap();
    writer.push(&idr64(2_200_000)).unwrap();
    writer.finish().unwrap();
    assert!(!dir.join("talk.000001.mkv").exists());
    assert!(dir.join("talk.000002.mkv").exists());
    assert!(dir.join("talk.000003.mkv").exists());
    assert_eq!(scanned_clusters(&dir.join("talk.000003.mkv")), 1);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn retention_by_bytes_keeps_the_latest_and_spares_foreign_files() {
    let dir = scratch("retention-bytes");
    std::fs::write(dir.join("notes.txt"), b"not a recording").unwrap();
    std::fs::write(dir.join("talk.backup.mkv"), b"not a segment").unwrap();
    let mut writer = SegmentWriter::create(
        &dir.join("talk.mkv"),
        RecordingLimits {
            max_bytes: 8 * 1024 * 1024,
            max_duration_ms: 1000,
        },
        Some(RecordingRetention {
            max_segments: 100,
            max_total_bytes: 1024 * 1024,
        }),
    )
    .unwrap();
    for index in 0..10u64 {
        writer.push(&idr64(index * 2_000_000)).unwrap();
        writer
            .push(&fat_p(index * 2_000_000 + 33_333, 128 * 1024))
            .unwrap();
    }
    writer.finish().unwrap();
    let files = finished(&dir);
    assert!(files.len() < 10, "oldest segments are swept");
    assert!(
        files
            .iter()
            .any(|p| p.file_name().unwrap() == "talk.000010.mkv"),
        "newest segment is always kept"
    );
    let total: u64 = files
        .iter()
        .map(|p| std::fs::metadata(p).unwrap().len())
        .sum();
    assert!(total <= 1024 * 1024, "sweep honors the byte budget");
    assert_eq!(
        std::fs::read(dir.join("notes.txt")).unwrap(),
        b"not a recording"
    );
    assert_eq!(
        std::fs::read(dir.join("talk.backup.mkv")).unwrap(),
        b"not a segment"
    );
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn retention_bounds_are_validated() {
    assert!(matches!(
        RecordingRetention {
            max_segments: 0,
            max_total_bytes: 1024 * 1024,
        }
        .validate(),
        Err(RecordingError::Limit)
    ));
    assert!(matches!(
        RecordingRetention {
            max_segments: 2,
            max_total_bytes: 1024,
        }
        .validate(),
        Err(RecordingError::Limit)
    ));
}

#[test]
fn startup_requires_a_usable_idr_and_leaves_no_files() {
    let dir = scratch("startup");
    let mut writer = SegmentWriter::create(&dir.join("talk.mkv"), limits(), None).unwrap();
    assert_eq!(writer.push(&p64(0)), Err(RecordingFailure::Media));
    assert_eq!(writer.push(&fat_p(0, 1024)), Err(RecordingFailure::Media));
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0);
    assert_eq!(writer.finish(), Err(RecordingFailure::Startup));
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_frame_larger_than_the_segment_budget_is_limit() {
    let dir = scratch("oversize");
    let mut writer = SegmentWriter::create(&dir.join("talk.mkv"), limits(), None).unwrap();
    // A priming IDR bloated by one SEI NAL to just under the frame cap leaves
    // no room for the Matroska headers, so no segment can hold it.
    let mut payload = unhex(IDR64);
    payload.extend_from_slice(&[0, 0, 0, 1, 0x06]);
    payload.extend(std::iter::repeat_n(0xFF, 1_048_576 - payload.len() - 10));
    let frame = EncodedFrame::h264(true, 0, payload).unwrap();
    assert_eq!(writer.push(&frame), Err(RecordingFailure::Limit));
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn wall_roll_finalizes_without_waiting_for_frames() {
    let dir = scratch("wall");
    let mut writer = SegmentWriter::create(&dir.join("talk.mkv"), limits(), None).unwrap();
    writer.roll_wall().unwrap();
    assert_eq!(writer.finish(), Err(RecordingFailure::Startup));
    let mut writer = SegmentWriter::create(&dir.join("talk.mkv"), limits(), None).unwrap();
    writer.push(&idr64(0)).unwrap();
    writer.roll_wall().unwrap();
    assert_eq!(writer.snapshot().segments, 1);
    writer.finish().unwrap();
    assert_eq!(finished(&dir).len(), 1);
    assert_eq!(scanned_clusters(&dir.join("talk.000001.mkv")), 1);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn interrupted_segment_recovers_from_its_own_part() {
    let dir = scratch("recover");
    let mut writer = SegmentWriter::create(&dir.join("talk.mkv"), limits(), None).unwrap();
    writer.push(&idr64(0)).unwrap();
    writer.push(&p64(33_333)).unwrap();
    drop(writer);
    let partial = dir.join("talk.000001.mkv.part");
    assert!(partial.is_file());
    let recovered = crate::recover_desktop_recording(&dir.join("talk.000001.mkv")).unwrap();
    assert_eq!(recovered.clusters, 2);
    assert!(dir.join("talk.000001.mkv").is_file());
    assert!(!partial.exists());
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn second_recording_to_the_same_stem_is_rejected() {
    let dir = scratch("collision");
    let mut writer = SegmentWriter::create(&dir.join("talk.mkv"), limits(), None).unwrap();
    writer.push(&idr64(0)).unwrap();
    writer.finish().unwrap();
    assert!(matches!(
        SegmentWriter::create(&dir.join("talk.mkv"), limits(), None),
        Err(RecordingError::Storage(_))
    ));
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn concurrent_recorders_sweep_only_their_own_stem_and_spare_live_partials() {
    let dir = scratch("concurrent");
    let retention = Some(RecordingRetention {
        max_segments: 2,
        max_total_bytes: 1024 * 1024,
    });
    let mut first = SegmentWriter::create(&dir.join("desk.mkv"), limits(), retention).unwrap();
    let mut second = SegmentWriter::create(&dir.join("cam.mkv"), limits(), retention).unwrap();
    // Interleave both recorders: each rolls several segments while the other
    // holds a live partial. Neither sweep may touch the other's stem.
    for index in 0..6u64 {
        first.push(&fat_idr(index * 2_000_000, 64 * 1024)).unwrap();
        first.roll_wall().unwrap();
        second.push(&fat_idr(index * 2_000_000, 64 * 1024)).unwrap();
        second.roll_wall().unwrap();
        assert_eq!(first.snapshot().segments, index as u32 + 1);
        assert_eq!(second.snapshot().segments, index as u32 + 1);
    }
    // Both stems swept to their own budget: newest kept, oldest gone.
    for stem in ["desk", "cam"] {
        let kept: Vec<PathBuf> = finished(&dir)
            .into_iter()
            .filter(|p| p.file_name().unwrap().to_str().unwrap().starts_with(stem))
            .collect();
        assert_eq!(kept.len(), 2, "{stem} keeps exactly two segments");
        assert!(
            kept.iter()
                .any(|p| p.file_name().unwrap() == format!("{stem}.000006.mkv").as_str()),
            "{stem} keeps its newest segment"
        );
        assert!(
            !dir.join(format!("{stem}.000001.mkv")).exists(),
            "{stem} swept its oldest segment"
        );
    }
    // Live partials are never sweep candidates, even mid-roll.
    first.push(&fat_idr(12_000_000, 64 * 1024)).unwrap();
    assert!(dir.join("desk.000007.mkv.part").is_file());
    second.push(&fat_idr(12_000_000, 64 * 1024)).unwrap();
    second.roll_wall().unwrap();
    assert!(
        dir.join("desk.000007.mkv.part").is_file(),
        "sweep spares the other recorder's live partial"
    );
    first.finish().unwrap();
    second.finish().unwrap();
    assert_eq!(finished(&dir).len(), 4);
    std::fs::remove_dir_all(&dir).unwrap();
}

/// Finished segments decode with independent tooling: ffprobe (not our
/// remuxer) must see an H.264 64x64 stream with exactly the pushed frames
/// and the right keyframe cadence in every segment.
#[test]
fn rolled_segments_decode_with_independent_tooling() {
    if !crate::needs::needs("ffprobe", crate::needs::tool("ffprobe", &["-version"])) {
        return;
    }
    let dir = scratch("ffprobe");
    let mut writer = SegmentWriter::create(
        &dir.join("talk.mkv"),
        RecordingLimits {
            max_bytes: 8 * 1024 * 1024,
            max_duration_ms: 1000,
        },
        None,
    )
    .unwrap();
    // Three duration-rolled segments of genuine frames (an IDR plus nine
    // P frames each): byte rolls would need filler a real decoder rejects,
    // and the segment minimum is 1 MiB.
    for segment in 0..3u64 {
        let start = segment * 1_100_000;
        writer.push(&idr64(start)).unwrap();
        for index in 1..10u64 {
            writer.push(&p64(start + index * 100_000)).unwrap();
        }
    }
    writer.finish().unwrap();
    let files = finished(&dir);
    assert_eq!(files.len(), 3);
    let mut total_frames = 0u64;
    for (position, path) in files.iter().enumerate() {
        let output = std::process::Command::new("ffprobe")
            .args([
                "-v",
                "error",
                "-count_frames",
                "-select_streams",
                "v:0",
                "-show_entries",
                "stream=codec_name,width,height,nb_read_frames",
                "-show_entries",
                "frame=pict_type",
                "-of",
                "default=noprint_wrappers=1",
                path.to_str().unwrap(),
            ])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "ffprobe failed on {}: {}",
            path.display(),
            String::from_utf8_lossy(&output.stderr)
        );
        let text = String::from_utf8_lossy(&output.stdout);
        let entry = |key: &str| {
            text.lines()
                .find_map(|line| line.strip_prefix(key).map(str::to_string))
        };
        assert_eq!(entry("codec_name="), Some("h264".into()), "{text}");
        assert_eq!(entry("width="), Some("64".into()), "{text}");
        assert_eq!(entry("height="), Some("64".into()), "{text}");
        let frames: u64 = entry("nb_read_frames=").unwrap().parse().unwrap();
        assert_eq!(
            frames,
            10,
            "segment {} holds 10 frames: {text}",
            position + 1
        );
        let pictures: Vec<String> = text
            .lines()
            .filter_map(|line| line.strip_prefix("pict_type="))
            .map(str::to_string)
            .collect();
        assert_eq!(pictures.len() as u64, frames, "frame entries: {text}");
        assert_eq!(pictures[0], "I", "segment opens with a keyframe: {text}");
        total_frames += frames;
    }
    assert_eq!(total_frames, 30);
    std::fs::remove_dir_all(&dir).unwrap();
}

/// A segment file that cannot be created fails prime as Storage and leaves
/// the destination otherwise untouched. A pre-existing partial stands in for
/// any create failure (disk full, read-only mount, quota): the recorder sees
/// the same create error either way, and the probe works as root, where
/// permission bits stop denying. Recovery stays an explicit separate path.
#[test]
fn uncreatable_segment_file_fails_prime_as_storage() {
    let dir = scratch("uncreatable");
    let mut writer = SegmentWriter::create(&dir.join("talk.mkv"), limits(), None).unwrap();
    std::fs::write(dir.join("talk.000001.mkv.part"), b"stale partial").unwrap();
    assert_eq!(writer.push(&idr64(0)), Err(RecordingFailure::Storage));
    assert_eq!(writer.finish(), Err(RecordingFailure::Startup));
    assert_eq!(
        std::fs::read(dir.join("talk.000001.mkv.part")).unwrap(),
        b"stale partial"
    );
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn retention_expires_owned_segments_and_preserves_foreign_ones() {
    let dir = scratch("ownership");
    let foreign = dir.join("talk.000005.mkv");
    std::fs::write(&foreign, b"another recording").unwrap();
    let mut writer = SegmentWriter::create(
        &dir.join("talk.mkv"),
        RecordingLimits {
            max_bytes: 8 * 1024 * 1024,
            max_duration_ms: 1000,
        },
        Some(RecordingRetention {
            max_segments: 2,
            max_total_bytes: 1024 * 1024,
        }),
    )
    .unwrap();
    for segment in 0..3u64 {
        writer.push(&idr64(segment * 1_100_000)).unwrap();
    }
    writer.finish().unwrap();
    // Owned oldest expired, owned newest kept, foreign untouched.
    assert!(!dir.join("talk.000001.mkv").exists());
    assert!(dir.join("talk.000003.mkv").is_file());
    assert_eq!(std::fs::read(&foreign).unwrap(), b"another recording");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn review_retention_preserves_preexisting_numbered_file() {
    let dir = scratch("review-existing");
    let foreign = dir.join("talk.000002.mkv");
    std::fs::write(&foreign, b"pre-existing user recording").unwrap();
    let mut writer = SegmentWriter::create(
        &dir.join("talk.mkv"),
        limits(),
        Some(RecordingRetention {
            max_segments: 1,
            max_total_bytes: 1024 * 1024,
        }),
    )
    .unwrap();
    writer.push(&idr64(0)).unwrap();
    writer.finish().unwrap();
    assert!(
        foreign.exists(),
        "retention deleted a pre-existing unowned file"
    );
    assert_eq!(
        std::fs::read(&foreign).unwrap(),
        b"pre-existing user recording"
    );
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn review_v10_retention_preserves_replacement_of_an_owned_segment() {
    let dir = scratch("review-v10-replacement");
    let mut writer = SegmentWriter::create(
        &dir.join("talk.mkv"),
        limits(),
        Some(RecordingRetention {
            max_segments: 1,
            max_total_bytes: 1024 * 1024,
        }),
    )
    .unwrap();
    writer.push(&idr64(0)).unwrap();
    writer.roll_wall().unwrap();
    let first = dir.join("talk.000001.mkv");
    std::fs::rename(&first, dir.join("saved-original.mkv")).unwrap();
    let unrelated = b"user file replacing the former segment path";
    std::fs::write(&first, unrelated).unwrap();
    writer.push(&idr64(1_000_000)).unwrap();
    writer.roll_wall().unwrap();
    let preserved = std::fs::read(&first);
    writer.finish().unwrap();
    std::fs::remove_dir_all(&dir).unwrap();
    assert_eq!(
        preserved.ok().as_deref(),
        Some(unrelated.as_slice()),
        "retention deleted an unrelated replacement using an old owned index"
    );
}

#[test]
fn review_v11_finished_recorder_status_counts_finalized_segment() {
    v11_recording_totals(1, false);
}
#[test]
fn review_v11_finished_recorder_status_counts_multiple_segments() {
    v11_recording_totals(3, false);
}
#[test]
fn review_v11_wall_roll_refreshes_status_before_stop_without_more_frames() {
    v11_recording_totals(1, true);
}
fn v11_recording_totals(frame_count: u64, wall: bool) {
    use std::time::{Duration, Instant};
    use vnilink::{
        Core, CoreConfig, CustomRelay, Grants, Listen, RelaySettings, ServiceKind, SessionPhase,
        VideoCodecCaps,
    };
    let boot = || {
        Core::start(CoreConfig {
            listen: Listen::Loopback,
            relay: RelaySettings::custom(vec![CustomRelay::new("http://127.0.0.1:9").unwrap()])
                .unwrap(),
            ..CoreConfig::default()
        })
        .unwrap()
    };
    let host = boot();
    let peer = boot();
    for c in [&host, &peer] {
        c.set_video_codecs(VideoCodecCaps::h264_full(4096, 4096, "test"))
            .unwrap();
    }
    host.set_capture(Box::new(crate::MonitorSize::new(64, 64)))
        .unwrap();
    host.start_hosting().unwrap();
    peer.accept_invitation(&host.create_invitation(Duration::from_secs(60)).unwrap())
        .unwrap();
    host.accept_invitation(&peer.create_invitation(Duration::from_secs(60)).unwrap())
        .unwrap();
    host.set_desktop_sharing(Grants {
        view: true,
        control: false,
        clipboard: false,
        transfer: false,
    })
    .unwrap();
    host.set_recording_sharing(true).unwrap();
    let parent = peer.begin_connect_services(&host.endpoint_id()).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while peer.controller_session(parent.id).unwrap().phase != SessionPhase::Active {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    }
    let parent = peer.controller_session(parent.id).unwrap();
    let owner = peer
        .attach_controller_service(parent.id, parent.generation, ServiceKind::Desktop)
        .unwrap();
    let desktop = peer
        .open_desktop_service(owner.id, owner.generation)
        .unwrap();
    let dir = scratch("review-v11-final-status");
    let mut budget = limits();
    if wall {
        budget.max_duration_ms = 1000;
    }
    let recorder = crate::Recorder::start(&desktop, &dir.join("talk.mkv"), budget).unwrap();
    let target = host.video_targets().unwrap()[0];
    let deadline = Instant::now() + Duration::from_secs(5);
    for index in 0..frame_count {
        host.publish_desktop_video(target.id, target.generation, idr64(index * 11_000_000))
            .unwrap();
        while recorder.status().frames != index + 1 {
            assert!(
                Instant::now() < deadline,
                "recorder missed frame: {:?}",
                recorder.status()
            );
            std::thread::sleep(Duration::from_millis(2));
        }
    }
    if wall {
        while recorder.status().segments != 1 {
            assert!(
                Instant::now() < deadline,
                "missing wall-roll status: {:?}",
                recorder.status()
            );
            std::thread::sleep(Duration::from_millis(2));
        }
        let state = recorder.status();
        assert_eq!(state.phase, crate::RecordingPhase::Recording);
        assert_eq!(
            state.bytes,
            std::fs::metadata(finished(&dir)[0].clone()).unwrap().len()
        );
    }
    recorder.stop();
    while !matches!(
        recorder.status().phase,
        crate::RecordingPhase::Finished | crate::RecordingPhase::Failed
    ) {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    }
    let status = recorder.status();
    let files = finished(&dir);
    let actual_bytes = files
        .iter()
        .map(|p| std::fs::metadata(p).unwrap().len())
        .sum::<u64>();
    desktop.close();
    host.shutdown();
    peer.shutdown();
    drop(recorder);
    std::fs::remove_dir_all(&dir).unwrap();
    assert_eq!(status.phase, crate::RecordingPhase::Finished);
    assert_eq!(status.frames, frame_count);
    assert_eq!(status.duration_ms, 0);
    assert_eq!(files.len() as u64, frame_count);
    assert_eq!(
        status.segments,
        files.len() as u32,
        "Finished status must count actual finalized segments (bytes status{} actual{})",
        status.bytes,
        actual_bytes
    );
    assert_eq!(
        status.bytes, actual_bytes,
        "Finished status must include final muxing bytes"
    );
}

#[test]
fn review_v12_failed_recorder_preserves_finalized_segment_totals() {
    use std::time::{Duration, Instant};
    use vnilink::{
        Core, CoreConfig, CustomRelay, Grants, Listen, RelaySettings, ServiceKind, SessionPhase,
        VideoCodecCaps,
    };
    let boot = || {
        Core::start(CoreConfig {
            listen: Listen::Loopback,
            relay: RelaySettings::custom(vec![CustomRelay::new("http://127.0.0.1:9").unwrap()])
                .unwrap(),
            ..CoreConfig::default()
        })
        .unwrap()
    };
    let host = boot();
    let peer = boot();
    for c in [&host, &peer] {
        c.set_video_codecs(VideoCodecCaps::h264_full(4096, 4096, "test"))
            .unwrap();
    }
    host.set_capture(Box::new(crate::MonitorSize::new(64, 64)))
        .unwrap();
    host.start_hosting().unwrap();
    peer.accept_invitation(&host.create_invitation(Duration::from_secs(60)).unwrap())
        .unwrap();
    host.accept_invitation(&peer.create_invitation(Duration::from_secs(60)).unwrap())
        .unwrap();
    host.set_desktop_sharing(Grants {
        view: true,
        control: false,
        clipboard: false,
        transfer: false,
    })
    .unwrap();
    host.set_recording_sharing(true).unwrap();
    let parent = peer.begin_connect_services(&host.endpoint_id()).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while peer.controller_session(parent.id).unwrap().phase != SessionPhase::Active {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    }
    let parent = peer.controller_session(parent.id).unwrap();
    let owner = peer
        .attach_controller_service(parent.id, parent.generation, ServiceKind::Desktop)
        .unwrap();
    let desktop = peer
        .open_desktop_service(owner.id, owner.generation)
        .unwrap();
    let dir = scratch("review-v12-failed-status");
    let recorder = crate::Recorder::start(&desktop, &dir.join("talk.mkv"), limits()).unwrap();
    let target = host.video_targets().unwrap()[0];
    host.publish_desktop_video(target.id, target.generation, idr64(0))
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while recorder.status().frames != 1 {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(2));
    }
    std::fs::write(dir.join("talk.000002.mkv.part"), b"foreign pending segment").unwrap();
    host.publish_desktop_video(target.id, target.generation, idr64(11_000_000))
        .unwrap();
    while recorder.status().phase != crate::RecordingPhase::Failed {
        assert!(
            Instant::now() < deadline,
            "expected storage collision failure: {:?}",
            recorder.status()
        );
        std::thread::sleep(Duration::from_millis(2));
    }
    let status = recorder.status();
    let files = finished(&dir);
    let bytes = files
        .iter()
        .map(|p| std::fs::metadata(p).unwrap().len())
        .sum::<u64>();
    let foreign = std::fs::read(dir.join("talk.000002.mkv.part")).unwrap();
    desktop.close();
    host.shutdown();
    peer.shutdown();
    drop(recorder);
    std::fs::remove_dir_all(&dir).unwrap();
    assert_eq!(foreign, b"foreign pending segment");
    assert_eq!(status.failure, Some(RecordingFailure::Storage));
    assert_eq!((status.frames, status.duration_ms), (1, 0));
    assert_eq!(files.len(), 1);
    assert_eq!(
        status.segments,
        files.len() as u32,
        "Failed recording lost finalized totals (statusbytes{} actual{})",
        status.bytes,
        bytes
    );
    assert_eq!(status.bytes, bytes);
}

#[test]
fn review_v12_finalization_error_keeps_completed_output_snapshot() {
    let dir = scratch("review-v12-failed-finalization");
    let mut writer = SegmentWriter::create(&dir.join("talk.mkv"), limits(), None).unwrap();
    writer.push(&idr64(0)).unwrap();
    writer.roll_wall().unwrap();
    let completed = writer.snapshot();
    writer.push(&idr64(11_000_000)).unwrap();
    let destination = dir.join("talk.000002.mkv");
    let foreign = b"foreign finalized destination";
    std::fs::write(&destination, foreign).unwrap();
    assert_eq!(writer.finalize(), Err(RecordingFailure::Storage));
    let failed = writer.snapshot();
    assert_eq!(failed.segments, completed.segments);
    assert_eq!(failed.frames, 2);
    assert_eq!(failed.duration_ms, completed.duration_ms);
    assert_eq!(
        failed.bytes,
        completed.bytes
            + std::fs::metadata(dir.join("talk.000002.mkv.part"))
                .unwrap()
                .len()
    );
    assert_eq!(
        completed.bytes,
        std::fs::metadata(dir.join("talk.000001.mkv"))
            .unwrap()
            .len()
    );
    assert_eq!(std::fs::read(destination).unwrap(), foreign);
    assert!(dir.join("talk.000002.mkv.part").is_file());
    drop(writer);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn review_v13_finalization_must_not_publish_a_replacement_partial() {
    let dir = scratch("review-v13-partial-replacement");
    let mut reference = SegmentWriter::create(&dir.join("reference.mkv"), limits(), None).unwrap();
    reference.push(&idr64(0)).unwrap();
    reference.finish().unwrap();
    let expected = std::fs::read(dir.join("reference.000001.mkv")).unwrap();
    let mut writer = SegmentWriter::create(&dir.join("talk.mkv"), limits(), None).unwrap();
    writer.push(&idr64(0)).unwrap();
    let original = dir.join("saved-original.mkv.part");
    let partial = dir.join("talk.000001.mkv.part");
    std::fs::rename(&partial, &original).unwrap();
    let unrelated = b"user file replacing live recording path";
    std::fs::write(&partial, unrelated).unwrap();
    let result = writer.finish();
    let output = std::fs::read(dir.join("talk.000001.mkv")).ok();
    let foreign = std::fs::read(&partial).ok();
    let original_bytes = std::fs::read(&original).unwrap();
    std::fs::remove_dir_all(&dir).unwrap();
    assert_eq!(
        original_bytes, expected,
        "original finalized media must remain byte-exact"
    );
    assert_eq!(
        result,
        Err(RecordingFailure::Storage),
        "foreign replacement was published as recording: {:?}",
        output
    );
    assert_eq!(foreign.as_deref(), Some(unrelated.as_slice()));
}

#[test]
fn review_v13_finalization_refuses_moved_partial() {
    v13_partial_alias("moved");
}
#[cfg(unix)]
#[test]
fn review_v13_finalization_refuses_symlink_partial() {
    v13_partial_alias("symlink");
}
#[test]
fn review_v13_finalization_refuses_multiply_linked_partial() {
    v13_partial_alias("hardlink");
}
fn v13_partial_alias(kind: &str) {
    let dir = scratch(&format!("review-v13-{kind}"));
    let mut writer = SegmentWriter::create(&dir.join("talk.mkv"), limits(), None).unwrap();
    writer.push(&idr64(0)).unwrap();
    let partial = dir.join("talk.000001.mkv.part");
    let saved = dir.join("saved-original.mkv.part");
    match kind {
        "hardlink" => std::fs::hard_link(&partial, &saved).unwrap(),
        _ => {
            std::fs::rename(&partial, &saved).unwrap();
            #[cfg(unix)]
            if kind == "symlink" {
                std::os::unix::fs::symlink(&saved, &partial).unwrap();
            }
        }
    }
    assert_eq!(writer.finalize(), Err(RecordingFailure::Storage));
    let status = writer.snapshot();
    assert_eq!((status.frames, status.segments), (1, 0));
    assert_eq!(status.bytes, std::fs::metadata(&saved).unwrap().len());
    assert!(!dir.join("talk.000001.mkv").exists());
    match kind {
        "moved" => assert!(!partial.exists()),
        "symlink" => assert_eq!(std::fs::read_link(&partial).unwrap(), saved),
        "hardlink" => assert_eq!(
            std::fs::read(&partial).unwrap(),
            std::fs::read(&saved).unwrap()
        ),
        _ => unreachable!(),
    }
    drop(writer);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn v14_failed_prime_must_preserve_replacement_partial() {
    let dir = scratch("v14-prime-replacement");
    let mut writer = SegmentWriter::create(&dir.join("talk.mkv"), limits(), None).unwrap();
    let partial = dir.join("talk.000001.mkv.part");
    let saved = dir.join("saved-original.part");
    let saved_hook = saved.clone();
    crate::recording_segments::V14_AFTER_CREATE.with(|hook| {
        *hook.borrow_mut() = Some(Box::new(move |partial| {
            std::fs::rename(partial, &saved_hook).unwrap();
            std::fs::write(partial, b"unrelated user file").unwrap();
        }))
    });
    let mut payload = unhex(IDR64);
    payload.extend_from_slice(&[0, 0, 0, 1, 0x06]);
    payload.extend(std::iter::repeat_n(0xFF, 1_048_576 - payload.len() - 10));
    let frame = EncodedFrame::h264(true, 0, payload).unwrap();
    let error = writer.push(&frame);
    let replacement = std::fs::read(&partial).ok();
    let original_exists = saved.exists();
    std::fs::remove_dir_all(&dir).unwrap();
    assert_eq!(error, Err(RecordingFailure::Limit));
    assert!(original_exists);
    assert_eq!(
        replacement.as_deref(),
        Some(b"unrelated user file".as_slice()),
        "failed prime removed foreign replacement by filename"
    );
}

#[test]
fn review_v14_foreign_segments_must_not_shorten_this_recordings_retention() {
    let dir = scratch("review-v14-foreign-budget");
    let foreign = dir.join("talk.000099.mkv");
    std::fs::write(&foreign, b"unrelated user recording").unwrap();
    let mut writer = SegmentWriter::create(
        &dir.join("talk.mkv"),
        limits(),
        Some(RecordingRetention {
            max_segments: 2,
            max_total_bytes: 1024 * 1024,
        }),
    )
    .unwrap();
    writer.push(&idr64(0)).unwrap();
    writer.roll_wall().unwrap();
    writer.push(&idr64(1_000_000)).unwrap();
    writer.finish().unwrap();
    let first = std::fs::read(dir.join("talk.000001.mkv")).ok();
    let second = std::fs::read(dir.join("talk.000002.mkv")).unwrap();
    let preserved_foreign = std::fs::read(&foreign).unwrap();
    std::fs::remove_dir_all(&dir).unwrap();
    assert!(!second.is_empty());
    assert_eq!(preserved_foreign, b"unrelated user recording");
    assert!(
        first.is_some(),
        "two owned segments are within this recording's max_segments2"
    );
}

#[test]
fn review_v14_foreign_bytes_must_not_consume_retention_budget() {
    v14_retention_budget(false);
}
#[test]
fn review_v14_replaced_owned_entry_must_not_consume_retention_budget() {
    v14_retention_budget(true);
}
fn v14_retention_budget(replace: bool) {
    let dir = scratch("review-v14-byte-budget");
    let mut writer = SegmentWriter::create(
        &dir.join("talk.mkv"),
        limits(),
        Some(RecordingRetention {
            max_segments: if replace { 2 } else { 8 },
            max_total_bytes: 1024 * 1024,
        }),
    )
    .unwrap();
    writer.push(&idr64(0)).unwrap();
    writer.roll_wall().unwrap();
    let foreign = if replace {
        let first = dir.join("talk.000001.mkv");
        std::fs::rename(&first, dir.join("saved-original.mkv")).unwrap();
        first
    } else {
        dir.join("talk.000099.mkv")
    };
    let foreign_bytes = vec![0x5a; 1024 * 1024];
    std::fs::write(&foreign, &foreign_bytes).unwrap();
    writer.push(&idr64(1_000_000)).unwrap();
    writer.roll_wall().unwrap();
    if replace {
        writer.push(&idr64(2_000_000)).unwrap();
    }
    let status = writer.finish_snapshot().unwrap();
    assert_eq!(status.segments, if replace { 3 } else { 2 });
    assert!(status.bytes < 1024 * 1024);
    assert_eq!(std::fs::read(foreign).unwrap(), foreign_bytes);
    assert!(dir.join("talk.000002.mkv").is_file());
    assert!(dir
        .join(if replace {
            "talk.000003.mkv"
        } else {
            "talk.000001.mkv"
        })
        .is_file());
    std::fs::remove_dir_all(dir).unwrap();
}
#[cfg(unix)]
#[test]
fn v14_failed_prime_preserves_aliases() {
    for hardlink in [false, true] {
        let dir = scratch("v14-prime-alias");
        let mut writer = SegmentWriter::create(&dir.join("talk.mkv"), limits(), None).unwrap();
        let partial = dir.join("talk.000001.mkv.part");
        let saved = dir.join("saved-original.part");
        let saved_hook = saved.clone();
        crate::recording_segments::V14_AFTER_CREATE.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move |partial| {
                if hardlink {
                    std::fs::hard_link(partial, &saved_hook).unwrap();
                } else {
                    std::fs::rename(partial, &saved_hook).unwrap();
                    std::os::unix::fs::symlink(&saved_hook, partial).unwrap();
                }
            }))
        });
        let mut payload = unhex(IDR64);
        payload.extend_from_slice(&[0, 0, 0, 1, 0x06]);
        payload.extend(std::iter::repeat_n(0xFF, 1_048_576 - payload.len() - 10));
        assert_eq!(
            writer.push(&EncodedFrame::h264(true, 0, payload).unwrap()),
            Err(RecordingFailure::Limit)
        );
        assert!(saved.is_file());
        if hardlink {
            assert_eq!(
                std::fs::read(&partial).unwrap(),
                std::fs::read(&saved).unwrap()
            );
        } else {
            assert_eq!(std::fs::read_link(&partial).unwrap(), saved);
        }
        drop(writer);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
