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

#[test]
fn quality_edit_rejects_stale_generation_without_changing_live_limits() {
    let host = start_direct();
    let controller = start_direct();
    host.set_capture(Box::new(SizeCapture {
        width: 1920,
        height: 1080,
    }))
    .unwrap();
    pair_for_control(&host, &controller);
    let session = controller.connect(&host.endpoint_id()).unwrap();
    let chosen = Quality::new(1000, 20, 1280, 720).unwrap();
    assert!(matches!(
        controller.update_session_quality_for_generation(
            session.id,
            session.generation + 1,
            crate::SessionRole::Controller,
            chosen,
        ),
        Err(crate::Error::StaleSession)
    ));
    assert_eq!(
        controller.controller_session(session.id).unwrap().quality,
        session.quality
    );
    let incoming = host.snapshot().unwrap().incoming.unwrap();
    assert!(matches!(
        host.update_session_quality_for_generation(
            incoming.id,
            incoming.generation + 1,
            crate::SessionRole::Host,
            chosen,
        ),
        Err(crate::Error::StaleSession)
    ));
    assert_eq!(
        host.snapshot().unwrap().incoming.unwrap().quality,
        incoming.quality
    );
    host.update_session_quality_for_generation(
        incoming.id,
        incoming.generation,
        crate::SessionRole::Host,
        chosen,
    )
    .unwrap();
    controller
        .update_session_quality_for_generation(
            session.id,
            session.generation,
            crate::SessionRole::Controller,
            chosen,
        )
        .unwrap();
    assert_eq!(
        controller.controller_session(session.id).unwrap().quality,
        Some(chosen)
    );
}

#[test]
fn saved_quality_defaults_start_new_connections_and_leave_live_limits_alone() {
    let first_host = start_direct();
    let second_host = start_direct();
    let controller = start_direct();
    for host in [&first_host, &second_host] {
        host.set_capture(Box::new(SizeCapture {
            width: 1920,
            height: 1080,
        }))
        .unwrap();
        pair_for_control(host, &controller);
    }
    let first = controller.connect(&first_host.endpoint_id()).unwrap();
    let defaults = Quality::new(1500, 24, 1280, 720).unwrap();
    controller.set_desktop_quality_defaults(defaults).unwrap();
    assert_eq!(
        controller.controller_session(first.id).unwrap().quality,
        first.quality
    );
    let pending = controller
        .begin_connect(&second_host.endpoint_id())
        .unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(8);
    let second = loop {
        let info = controller.controller_session(pending.id).unwrap();
        if info.phase == crate::SessionPhase::Active {
            break info;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "new desktop session did not activate"
        );
        std::thread::sleep(std::time::Duration::from_millis(5));
    };
    assert_eq!(second.quality, Some(defaults));
    assert_eq!(
        second_host.snapshot().unwrap().incoming.unwrap().quality,
        Some(defaults)
    );
}

#[test]
fn auto_negotiates_only_common_probed_codecs_and_prefers_hardware() {
    use crate::{CodecSupport, VideoCodec as C};
    let mut host = VideoCodecCaps::h264_full(4096, 4096, "test");
    host.additional = vec![
        CodecSupport {
            codec: C::H265,
            encode: true,
            decode: true,
            hardware: true,
        },
        CodecSupport {
            codec: C::Vp9,
            encode: true,
            decode: true,
            hardware: false,
        },
        CodecSupport {
            codec: C::Av1,
            encode: true,
            decode: true,
            hardware: false,
        },
    ];
    let mut viewer = host.clone();
    assert_eq!(
        negotiate(viewer.decoder_offer(), &host, true)
            .unwrap()
            .codec,
        C::H265.wire_id()
    );
    viewer
        .additional
        .iter_mut()
        .find(|c| c.codec == C::H265)
        .unwrap()
        .decode = false;
    assert_eq!(
        negotiate(viewer.decoder_offer(), &host, true)
            .unwrap()
            .codec,
        CODEC_H264_HIGH
    );
    host.encode_baseline = false;
    host.encode_high = false;
    host.additional[0].encode = false;
    assert_eq!(
        negotiate(viewer.decoder_offer(), &host, true)
            .unwrap()
            .codec,
        C::Vp9.wire_id()
    );
    viewer.additional.iter_mut().for_each(|c| c.decode = false);
    assert!(negotiate(viewer.decoder_offer(), &host, true).is_none());
    assert!(negotiate(255, &host, true).is_none());
    host.additional.push(host.additional[0].clone());
    assert!(host.validate().is_err());
}

#[test]
fn auto_and_explicit_codec_survive_owned_desktop_transport_and_reject_mismatched_publisher() {
    use crate::{CodecSupport, ServiceKind, VideoCodec};
    use std::time::Duration;
    for codec in [
        VideoCodec::H265,
        VideoCodec::Vp8,
        VideoCodec::Vp9,
        VideoCodec::Av1,
    ] {
        for explicit in [false, true] {
            let host = start_direct();
            let viewer = start_direct();
            assert_eq!(viewer.desktop_codec_default().unwrap(), None);
            if explicit {
                viewer.set_desktop_codec_default(Some(codec)).unwrap();
            }
            host.set_capture(Box::new(SizeCapture {
                width: 64,
                height: 64,
            }))
            .unwrap();
            let caps = VideoCodecCaps {
                additional: vec![CodecSupport {
                    codec,
                    encode: true,
                    decode: true,
                    hardware: false,
                }],
                max_width: 4096,
                max_height: 4096,
                ..Default::default()
            };
            let caps = if explicit {
                VideoCodecCaps {
                    additional: caps.additional,
                    ..VideoCodecCaps::h264_full(4096, 4096, "competing hardware")
                }
            } else {
                caps
            };
            pair_for_control(&host, &viewer);
            host.set_video_codecs(caps.clone()).unwrap();
            viewer.set_video_codecs(caps).unwrap();
            host.set_service_sharing(ServiceKind::Desktop, true)
                .unwrap();
            let pending = viewer.begin_connect_services(&host.endpoint_id()).unwrap();
            let deadline = std::time::Instant::now() + Duration::from_secs(5);
            while viewer.controller_session(pending.id).unwrap().phase
                != crate::SessionPhase::Active
            {
                assert!(std::time::Instant::now() < deadline);
                std::thread::sleep(Duration::from_millis(10));
            }
            let parent = viewer.controller_session(pending.id).unwrap();
            let owner = viewer
                .attach_controller_service(parent.id, parent.generation, ServiceKind::Desktop)
                .unwrap();
            let desktop = viewer
                .open_desktop_service(owner.id, owner.generation)
                .unwrap();
            assert_eq!(desktop.desktop_info().unwrap().codec, codec.wire_id());
            viewer
                .set_desktop_codec_default(Some(crate::VideoCodec::H264))
                .unwrap();
            assert_eq!(desktop.desktop_info().unwrap().codec, codec.wire_id());
            let subscription = desktop.subscribe_video().unwrap();
            let target = host
                .video_targets()
                .unwrap()
                .into_iter()
                .find(|t| t.service)
                .unwrap();
            assert!(matches!(
                host.publish_desktop_video(
                    target.id,
                    target.generation,
                    EncodedFrame::h264(true, 0, vec![1]).unwrap()
                ),
                Err(crate::Error::InvalidSetting(_))
            ));
            let packet = EncodedFrame::new(codec, true, 33_333, vec![1, 2, 3, 4]).unwrap();
            host.publish_desktop_video(target.id, target.generation, packet.clone())
                .unwrap();
            let received = subscription.recv_timeout(Duration::from_secs(5)).unwrap();
            assert_eq!(
                (received.codec, received.timestamp_us, received.payload),
                (codec, packet.timestamp_us, packet.payload)
            );
            desktop.close();
            viewer.shutdown();
            host.shutdown();
        }
    }
}

#[test]
fn explicit_codec_offer_never_negotiates_another_codec() {
    use crate::{CodecSupport, VideoCodec};
    let mut viewer = VideoCodecCaps::decode_only("test");
    viewer.additional = VideoCodec::ALL
        .into_iter()
        .filter(|c| *c != VideoCodec::H264)
        .map(|codec| CodecSupport {
            codec,
            encode: false,
            decode: true,
            hardware: false,
        })
        .collect();
    let mut host = VideoCodecCaps::h264_full(4096, 4096, "test");
    host.additional = viewer
        .additional
        .iter()
        .map(|c| CodecSupport {
            encode: true,
            ..c.clone()
        })
        .collect();
    for codec in VideoCodec::ALL {
        let offer = viewer.preferred_offer(Some(codec)).unwrap();
        assert_eq!(
            VideoCodec::from_wire_id(negotiate(offer, &host, true).unwrap().codec),
            Some(codec)
        );
        let mut missing = host.clone();
        if codec == VideoCodec::H264 {
            missing.encode_baseline = false;
            missing.encode_high = false;
        } else {
            missing.additional.retain(|c| c.codec != codec);
        }
        assert!(negotiate(offer, &missing, true).is_none());
    }
    viewer.decode_baseline = false;
    assert_eq!(
        negotiate(
            viewer.preferred_offer(Some(VideoCodec::H264)).unwrap(),
            &host,
            true
        )
        .unwrap()
        .codec,
        CODEC_H264_HIGH
    );
    host.encode_high = false;
    assert!(negotiate(
        viewer.preferred_offer(Some(VideoCodec::H264)).unwrap(),
        &host,
        true
    )
    .is_none());
    assert!(VideoCodecCaps::default()
        .preferred_offer(Some(VideoCodec::Vp9))
        .is_err());
}

#[test]
fn explicit_codec_also_controls_direct_desktop_connections() {
    use crate::{CodecSupport, VideoCodec};
    for codec in VideoCodec::ALL {
        let host = start_direct();
        let viewer = start_direct();
        host.set_capture(Box::new(SizeCapture {
            width: 64,
            height: 64,
        }))
        .unwrap();
        pair_for_control(&host, &viewer);
        let mut caps = VideoCodecCaps::h264_full(4096, 4096, "competing hardware");
        caps.additional = VideoCodec::ALL
            .into_iter()
            .filter(|c| *c != VideoCodec::H264)
            .map(|codec| CodecSupport {
                codec,
                encode: true,
                decode: true,
                hardware: false,
            })
            .collect();
        host.set_video_codecs(caps.clone()).unwrap();
        viewer.set_video_codecs(caps).unwrap();
        viewer.set_desktop_codec_default(Some(codec)).unwrap();
        viewer.connect(&host.endpoint_id()).unwrap();
        assert_eq!(
            VideoCodec::from_wire_id(host.video_targets().unwrap()[0].codec),
            Some(codec)
        );
        viewer.shutdown();
        host.shutdown();
    }
    let host = start_direct();
    let viewer = start_direct();
    host.set_capture(Box::new(SizeCapture {
        width: 64,
        height: 64,
    }))
    .unwrap();
    pair_for_control(&host, &viewer);
    let mut caps = VideoCodecCaps::h264_full(4096, 4096, "test");
    caps.additional.push(CodecSupport {
        codec: VideoCodec::Vp9,
        encode: true,
        decode: true,
        hardware: false,
    });
    viewer.set_video_codecs(caps).unwrap();
    viewer
        .set_desktop_codec_default(Some(VideoCodec::Vp9))
        .unwrap();
    assert!(viewer.connect(&host.endpoint_id()).is_err());
    assert!(host.video_targets().unwrap().is_empty());
    viewer.shutdown();
    host.shutdown();
}
