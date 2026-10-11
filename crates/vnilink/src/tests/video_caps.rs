use super::support::{pair_for_control, start_direct};
use crate::protocol::{Quality, CODEC_H264, CODEC_H264_HIGH};
use crate::video::{EncodedFrame, HostCapture};
use crate::video_caps::{clamp_quality, negotiate, VideoCodecCaps};

struct SizeCapture {
    width: u16,
    height: u16,
}

impl HostCapture for SizeCapture {
    fn monitor(&self) -> (u16, u16) {
        (self.width, self.height)
    }
}

#[test]
fn negotiation_prefers_high_and_falls_back_to_baseline() {
    let full = VideoCodecCaps::h264_full(4096, 4096, "test");
    let agreed = negotiate(CODEC_H264, &full, false).unwrap();
    assert_eq!(agreed.codec, CODEC_H264);
    let agreed = negotiate(CODEC_H264_HIGH, &full, true).unwrap();
    assert_eq!(agreed.codec, CODEC_H264_HIGH);
    assert_eq!((agreed.max_width, agreed.max_height), (4096, 4096));
    // Silent safe downgrade: every High decoder also decodes Baseline.
    let agreed = negotiate(CODEC_H264_HIGH, &full, false).unwrap();
    assert_eq!(agreed.codec, CODEC_H264);
    let baseline = VideoCodecCaps::h264_baseline(1920, 1080, "test");
    let agreed = negotiate(CODEC_H264_HIGH, &baseline, true).unwrap();
    assert_eq!(agreed.codec, CODEC_H264);
    assert_eq!((agreed.max_width, agreed.max_height), (1920, 1080));
    assert_eq!(negotiate(9, &full, true), None);
    assert_eq!(
        negotiate(CODEC_H264, &VideoCodecCaps::default(), true),
        None
    );
    // SPEC06 boundary: cleared probe bits (no-GPU Linux) agree on
    // nothing for either profile, so admission refuses explicitly.
    assert_eq!(
        negotiate(CODEC_H264_HIGH, &VideoCodecCaps::default(), true),
        None
    );
    assert_eq!(
        negotiate(CODEC_H264_HIGH, &VideoCodecCaps::decode_only("test"), true),
        None
    );
}

#[test]
fn clamp_respects_monitor_codec_limits_and_zero_sides() {
    let offer = Quality::new(2_000, 30, 1920, 1080).unwrap();
    let caps = VideoCodecCaps::h264_full(4096, 4096, "test");
    assert_eq!(clamp_quality(&offer, (800, 600), &caps).max_width, 800);
    assert_eq!(clamp_quality(&offer, (0, 0), &caps), offer);
    let small = VideoCodecCaps::h264_full(640, 480, "test");
    let clamped = clamp_quality(&offer, (4000, 3000), &small);
    assert_eq!((clamped.max_width, clamped.max_height), (640, 480));
    let unbounded = VideoCodecCaps::decode_only("test");
    assert_eq!(clamp_quality(&offer, (800, 600), &unbounded).max_width, 800);
}

#[test]
fn caps_validation_rejects_missing_limits_and_long_labels() {
    assert!(VideoCodecCaps::h264_full(4096, 4096, "test")
        .validate()
        .is_ok());
    assert!(VideoCodecCaps::default().validate().is_ok());
    let mut encode_no_limits = VideoCodecCaps::encode_only(0, 0, "test");
    assert!(encode_no_limits.validate().is_err());
    encode_no_limits.max_width = 640;
    encode_no_limits.max_height = 480;
    assert!(encode_no_limits.validate().is_ok());
    let long = VideoCodecCaps {
        device: "d".repeat(65),
        ..VideoCodecCaps::h264_full(4096, 4096, "test")
    };
    assert!(long.validate().is_err());
    assert!(!VideoCodecCaps::default().can_encode());
    assert!(!VideoCodecCaps::default().can_decode());
    assert!(VideoCodecCaps::decode_only("test").can_decode());
    assert!(!VideoCodecCaps::decode_only("test").can_encode());
}

/// Needs loopback cores. A 1080p monitor with 640x480 codec caps admits
/// the session but negotiates clamped dimensions on both records.
#[test]
fn admission_clamps_quality_to_host_codec_caps() {
    let host = start_direct();
    let controller = start_direct();
    host.set_capture(Box::new(SizeCapture {
        width: 1920,
        height: 1080,
    }))
    .unwrap();
    pair_for_control(&host, &controller);
    host.set_video_codecs(VideoCodecCaps::h264_full(640, 480, "test"))
        .unwrap();
    let session = controller.connect(&host.endpoint_id()).unwrap();
    let agreed = Quality::new(2_000, 30, 640, 480).unwrap();
    assert_eq!(session.quality, Some(agreed));
    assert_eq!(
        host.snapshot().unwrap().incoming.unwrap().quality,
        Some(agreed)
    );
    assert_eq!(host.video_codecs().unwrap().device, "test");
    // The wire still carries Baseline-capable frames only.
    assert!(EncodedFrame::h264(true, 1, vec![1]).is_ok());
}
