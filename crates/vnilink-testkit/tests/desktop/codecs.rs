// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! What two engines agree a desktop's pictures are: the codec, its limits
//! and the picture quality, at connect and afterwards.

use expect_test::expect;

use vnilink::{
    CodecSupport, EncodedFrame, Error, Quality, RemoteDesktopService, ServiceKind, VideoCodec,
    VideoCodecCaps,
};
use vnilink_testkit::desktop::{next_picture, MonitorCapture};
use vnilink_testkit::{traced, Engine, Pair, TestResult};

use super::streams::SCREEN;

/// The size of the host's screen where only the codec matters.
const SMALL: (u16, u16) = (64, 64);

/// A host serving a screen of `monitor`, paired with a viewer, both
/// decoding and encoding H.264 up to 4K, as the suites' engines do.
fn pair_with_screen(monitor: (u16, u16)) -> Result<Pair, Error> {
    let pair = Pair::for_desktop();
    pair.host
        .core
        .set_capture(Box::new(MonitorCapture::new(monitor)))?;
    Ok(pair)
}

/// A viewer's Desktop service on a new services connection to the
/// pair's host.
fn open_desktop(pair: &Pair) -> Result<RemoteDesktopService, Error> {
    let connection = pair.connect_services();
    pair.controller.open_desktop(&connection)
}

/// A 1080p monitor with 640x480 codec limits admits the Desktop, and both
/// sides record the quality clamped to them.
#[test]
fn admission_clamps_quality_to_host_codec_caps() -> TestResult {
    let trace = traced();
    let pair = pair_with_screen(SCREEN)?;
    let host = &pair.host.core;
    host.set_video_codecs(VideoCodecCaps::h264_full(640, 480, "test"))?;
    let desktop = open_desktop(&pair)?;
    // The Desktop starts at the dialled rate, held to what the picture can
    // use: the 1080p monitor's proportions inside 640x480 are 640x360,
    // which at 30 fps is 1,105 kb/s as H.264. It is not the viewer's limit.
    let agreed = Quality::new(1_105, 30, 640, 480)?;
    assert_eq!(
        desktop.desktop_info()?.quality,
        agreed,
        "the viewer records the clamped quality"
    );
    let (id, generation) = hosted_desktop(&pair);
    assert_eq!(
        host.hosted_desktop_info(id, generation)?.quality,
        agreed,
        "the host records the clamped quality"
    );
    assert_eq!(
        host.video_codecs()?.device,
        "test",
        "the host keeps the codecs it was given"
    );
    assert!(
        EncodedFrame::h264(true, 1, vec![1]).is_ok(),
        "the wire still carries Baseline-capable frames"
    );
    vnilink_testkit::assert_story(
        expect![[r#"
        controller:
          session: session phase from=connecting generation=1 role=Controller session=1 to=active
    "#]],
        || trace.story(&["session", "service"]),
    );
    Ok(())
}

/// The host's Desktop service for the pair's viewer: its id and generation.
fn hosted_desktop(pair: &Pair) -> (u64, u64) {
    let service = pair.host.serving(ServiceKind::Desktop, &pair.controller);
    (service.id, service.generation)
}

/// Saved quality defaults start the Desktops opened afterwards, on both
/// sides, and leave an open Desktop's quality as it was.
#[test]
fn saved_quality_defaults_start_new_desktops_and_leave_live_limits_alone() -> TestResult {
    let trace = traced();
    let controller = Engine::start("controller");
    let hosts = [Engine::start("host"), Engine::start("second-host")];
    for host in &hosts {
        host.core
            .set_capture(Box::new(MonitorCapture::new(SCREEN)))?;
        host.pair_for_control(&controller)?;
    }
    let first = controller.open_desktop(&controller.connect_services(&hosts[0])?)?;
    let started = first.desktop_info()?.quality;
    let defaults = Quality::new(1500, 24, 1280, 720)?;
    controller.core.set_desktop_quality_defaults(defaults)?;
    assert_eq!(
        first.desktop_info()?.quality,
        started,
        "new defaults leave an open Desktop's quality"
    );
    let second = controller.open_desktop(&controller.connect_services(&hosts[1])?)?;
    assert_eq!(
        second.desktop_info()?.quality,
        defaults,
        "a Desktop opened afterwards starts at the defaults"
    );
    let hosted = hosts[1].serving(ServiceKind::Desktop, &controller);
    assert_eq!(
        hosts[1]
            .core
            .hosted_desktop_info(hosted.id, hosted.generation)?
            .quality,
        defaults,
        "its host starts at the defaults too"
    );
    vnilink_testkit::assert_story(
        expect![[r#"
        controller:
          session: session phase from=connecting generation=1 role=Controller session=1 to=active
          session: session phase from=connecting generation=1 role=Controller session=2 to=active
    "#]],
        || trace.story(&["session", "service"]),
    );
    Ok(())
}

/// For each codec beyond H.264, chosen automatically or explicitly, a
/// Desktop service agrees it, keeps it when the viewer's default changes,
/// refuses a picture in another codec and carries one in its own.
#[test]
fn auto_and_explicit_codec_survive_owned_desktop_transport_and_reject_mismatched_publisher(
) -> TestResult {
    let trace = traced();
    for codec in [
        VideoCodec::H265,
        VideoCodec::Vp8,
        VideoCodec::Vp9,
        VideoCodec::Av1,
    ] {
        for explicit in [false, true] {
            let pair = pair_with_screen(SMALL)?;
            let (host, viewer) = (&pair.host.core, &pair.controller.core);
            assert_eq!(
                viewer.desktop_codec_default()?,
                None,
                "a viewer chooses no codec by default"
            );
            if explicit {
                viewer.set_desktop_codec_default(Some(codec))?;
            }
            let only = VideoCodecCaps {
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
            // Chosen explicitly, the codec wins over H.264 in hardware.
            let caps = if explicit {
                VideoCodecCaps {
                    additional: only.additional,
                    ..VideoCodecCaps::h264_full(4096, 4096, "competing hardware")
                }
            } else {
                only
            };
            host.set_video_codecs(caps.clone())?;
            viewer.set_video_codecs(caps)?;
            host.set_service_sharing(ServiceKind::Desktop, true)?;
            let connection = pair.connect_services();
            let desktop = pair.controller.open_desktop(&connection)?;
            assert_eq!(
                desktop.desktop_info()?.codec,
                codec.wire_id(),
                "the desktop agrees {codec:?} (explicit: {explicit})"
            );
            viewer.set_desktop_codec_default(Some(VideoCodec::H264))?;
            assert_eq!(
                desktop.desktop_info()?.codec,
                codec.wire_id(),
                "an open desktop keeps its codec when the default changes"
            );
            let video = desktop.subscribe_video()?;
            let target = host
                .video_targets()?
                .into_iter()
                .next()
                .ok_or("the host's Desktop service is a video target")?;
            assert!(
                matches!(
                    host.publish_desktop_video(
                        target.id,
                        target.generation,
                        EncodedFrame::h264(true, 0, vec![1])?
                    ),
                    Err(Error::InvalidSetting(_))
                ),
                "a picture in another codec is refused"
            );
            let packet = EncodedFrame::new(codec, true, 33_333, vec![1, 2, 3, 4])?;
            host.publish_desktop_video(target.id, target.generation, packet.clone())?;
            let received = next_picture(&video, "the picture in the agreed codec");
            assert_eq!(
                (received.codec, received.timestamp_us, received.payload),
                (codec, packet.timestamp_us, packet.payload),
                "the picture arrives in its codec, as published"
            );
            desktop.close();
            pair.host.wait_released(Some(ServiceKind::Desktop));
        }
    }
    trace.wait_for_records("service phase", 16);
    vnilink_testkit::assert_story(
        expect![[r#"
        controller:
          service: service phase from=active generation=1 kind=Desktop service=1 to=closed: OwnerClose
          service: service phase from=active generation=1 kind=Desktop service=1 to=closed: OwnerClose
          service: service phase from=active generation=1 kind=Desktop service=1 to=closed: OwnerClose
          service: service phase from=active generation=1 kind=Desktop service=1 to=closed: OwnerClose
          service: service phase from=active generation=1 kind=Desktop service=1 to=closed: OwnerClose
          service: service phase from=active generation=1 kind=Desktop service=1 to=closed: OwnerClose
          service: service phase from=active generation=1 kind=Desktop service=1 to=closed: OwnerClose
          service: service phase from=active generation=1 kind=Desktop service=1 to=closed: OwnerClose
        host:
          service: service phase from=active generation=1 kind=Desktop service=1 to=closed: PeerClose
          service: service phase from=active generation=1 kind=Desktop service=1 to=closed: PeerClose
          service: service phase from=active generation=1 kind=Desktop service=1 to=closed: PeerClose
          service: service phase from=active generation=1 kind=Desktop service=1 to=closed: PeerClose
          service: service phase from=active generation=1 kind=Desktop service=1 to=closed: PeerClose
          service: service phase from=active generation=1 kind=Desktop service=1 to=closed: PeerClose
          service: service phase from=active generation=1 kind=Desktop service=1 to=closed: PeerClose
          service: service phase from=active generation=1 kind=Desktop service=1 to=closed: PeerClose
    "#]],
        || trace.story(&["service"]),
    );
    Ok(())
}

/// A viewer's explicit codec governs its Desktop even where the host has
/// H.264 in hardware: the host encodes what the viewer chose, and a codec
/// the host cannot encode refuses the Desktop.
#[test]
fn explicit_codec_controls_the_desktop_and_one_the_host_cannot_encode_refuses_it() -> TestResult {
    let trace = traced();
    for codec in VideoCodec::ALL {
        let pair = pair_with_screen(SMALL)?;
        let (host, viewer) = (&pair.host.core, &pair.controller.core);
        let mut caps = VideoCodecCaps::h264_full(4096, 4096, "competing hardware");
        caps.additional = VideoCodec::ALL
            .into_iter()
            .filter(|other| *other != VideoCodec::H264)
            .map(|codec| CodecSupport {
                codec,
                encode: true,
                decode: true,
                hardware: false,
            })
            .collect();
        host.set_video_codecs(caps.clone())?;
        viewer.set_video_codecs(caps)?;
        viewer.set_desktop_codec_default(Some(codec))?;
        let desktop = open_desktop(&pair)?;
        assert_eq!(
            host.video_targets()?
                .first()
                .and_then(|target| VideoCodec::from_wire_id(target.codec)),
            Some(codec),
            "the host encodes the viewer's explicit codec"
        );
        desktop.close();
        pair.host.wait_released(Some(ServiceKind::Desktop));
    }
    let pair = pair_with_screen(SMALL)?;
    let (host, viewer) = (&pair.host.core, &pair.controller.core);
    let mut caps = VideoCodecCaps::h264_full(4096, 4096, "test");
    caps.additional.push(CodecSupport {
        codec: VideoCodec::Vp9,
        encode: true,
        decode: true,
        hardware: false,
    });
    viewer.set_video_codecs(caps)?;
    viewer.set_desktop_codec_default(Some(VideoCodec::Vp9))?;
    assert!(
        open_desktop(&pair).is_err(),
        "a codec the host cannot encode refuses the Desktop"
    );
    assert!(
        host.video_targets()?.is_empty(),
        "a refused Desktop leaves the host nothing to encode"
    );
    vnilink_testkit::assert_story(
        expect![[r#"
        controller:
          service: service phase from=active generation=1 kind=Desktop service=1 to=closed: OwnerClose
          service: service phase from=active generation=1 kind=Desktop service=1 to=closed: OwnerClose
          service: service phase from=active generation=1 kind=Desktop service=1 to=closed: OwnerClose
          service: service phase from=active generation=1 kind=Desktop service=1 to=closed: OwnerClose
          service: service phase from=active generation=1 kind=Desktop service=1 to=closed: OwnerClose
        host:
          service: service phase from=active generation=1 kind=Desktop service=1 to=closed: PeerClose
          service: service phase from=active generation=1 kind=Desktop service=1 to=closed: PeerClose
          service: service phase from=active generation=1 kind=Desktop service=1 to=closed: PeerClose
          service: service phase from=active generation=1 kind=Desktop service=1 to=closed: PeerClose
          service: service phase from=active generation=1 kind=Desktop service=1 to=closed: PeerClose
    "#]],
        || trace.story(&["service"]),
    );
    Ok(())
}
