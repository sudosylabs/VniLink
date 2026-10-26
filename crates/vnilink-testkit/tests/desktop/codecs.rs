//! What two engines agree a desktop's pictures are: the codec, its limits
//! and the picture quality, at connect and afterwards.

use expect_test::expect;
use std::time::Duration;

use vnilink::{
    CodecSupport, CoreEvent, EncodedFrame, Error, EventSubscription, Quality, ServiceKind,
    SessionRole, VideoCodec, VideoCodecCaps,
};
use vnilink_testkit::desktop::{next_picture, MonitorCapture};
use vnilink_testkit::{traced, wait_for, Engine, Pair, TestResult, DEFAULT_WAIT};

use super::lifecycle::pair;
use super::streams::SCREEN;

/// The size of the host's screen where only the codec matters.
const SMALL: (u16, u16) = (64, 64);

/// A host serving a screen of `monitor`, paired with a viewer, both
/// decoding and encoding H.264 up to 4K, as the suites' engines do.
fn pair_with_screen(monitor: (u16, u16)) -> Result<Pair, Error> {
    let pair = Pair::for_desktop();
    pair.host
        .core
        .set_capture(Box::new(MonitorCapture(monitor)))?;
    Ok(pair)
}

/// The next quality update `events` brings: the session it names and the
/// quality.
fn quality_update(events: &EventSubscription) -> (u64, Quality) {
    wait_for("a quality update", DEFAULT_WAIT, || {
        match events.recv_timeout(Duration::from_millis(20))? {
            CoreEvent::QualityUpdated { id, quality } => Some((id, quality)),
            _ => None,
        }
    })
}

/// A 1080p monitor with 640x480 codec limits admits the session, and both
/// sides record the quality clamped to them.
#[test]
fn admission_clamps_quality_to_host_codec_caps() -> TestResult {
    let trace = traced();
    let pair = pair_with_screen(SCREEN)?;
    let host = &pair.host.core;
    host.set_video_codecs(VideoCodecCaps::h264_full(640, 480, "test"))?;
    let session = pair.connect_control();
    // The session starts at the dialled rate, held to what the picture can
    // use: the 1080p monitor's proportions inside 640x480 are 640x360,
    // which at 30 fps is 1,105 kb/s as H.264. It is not the viewer's limit.
    let agreed = Quality::new(1_105, 30, 640, 480)?;
    assert_eq!(
        session.quality,
        Some(agreed),
        "the viewer records the clamped quality"
    );
    assert_eq!(
        host.snapshot()?
            .incoming
            .ok_or("the host's session")?
            .quality,
        Some(agreed),
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
    expect![[r#"
        controller:
          session: session phase from=connecting generation=1 role=Controller session=1 to=active
    "#]]
    .assert_eq(&trace.story(&["session", "stream", "encode"]));
    Ok(())
}

/// A quality edit for a stale generation is refused on either side and
/// changes nothing; for the current one it applies.
#[test]
fn quality_edit_rejects_stale_generation_without_changing_live_limits() -> TestResult {
    let trace = traced();
    let pair = pair_with_screen(SCREEN)?;
    let (host, controller) = (&pair.host.core, &pair.controller.core);
    let session = pair.connect_control();
    let chosen = Quality::new(1000, 20, 1280, 720)?;
    assert!(
        matches!(
            controller.update_session_quality_for_generation(
                session.id,
                session.generation + 1,
                SessionRole::Controller,
                chosen,
            ),
            Err(Error::StaleSession)
        ),
        "the viewer refuses an edit for a stale generation"
    );
    assert_eq!(
        controller.controller_session(session.id)?.quality,
        session.quality,
        "a refused edit leaves the viewer's quality"
    );
    let incoming = host.snapshot()?.incoming.ok_or("the host's session")?;
    assert!(
        matches!(
            host.update_session_quality_for_generation(
                incoming.id,
                incoming.generation + 1,
                SessionRole::Host,
                chosen,
            ),
            Err(Error::StaleSession)
        ),
        "the host refuses an edit for a stale generation"
    );
    assert_eq!(
        host.snapshot()?
            .incoming
            .ok_or("the host's session")?
            .quality,
        incoming.quality,
        "a refused edit leaves the host's quality"
    );
    host.update_session_quality_for_generation(
        incoming.id,
        incoming.generation,
        SessionRole::Host,
        chosen,
    )?;
    controller.update_session_quality_for_generation(
        session.id,
        session.generation,
        SessionRole::Controller,
        chosen,
    )?;
    assert_eq!(
        controller.controller_session(session.id)?.quality,
        Some(chosen),
        "an edit for the current generation applies"
    );
    expect![[r#"
        controller:
          session: session phase from=connecting generation=1 role=Controller session=1 to=active
    "#]]
    .assert_eq(&trace.story(&["session", "stream", "encode"]));
    Ok(())
}

/// Saved quality defaults start the connections opened afterwards, on both
/// sides, and leave an open connection's quality as it was.
#[test]
fn saved_quality_defaults_start_new_connections_and_leave_live_limits_alone() -> TestResult {
    let trace = traced();
    let controller = Engine::start("controller");
    let hosts = [Engine::start("host"), Engine::start("second-host")];
    for host in &hosts {
        host.core.set_capture(Box::new(MonitorCapture(SCREEN)))?;
        pair(host, &controller)?;
    }
    let first = controller.connect_control(&hosts[0])?;
    let defaults = Quality::new(1500, 24, 1280, 720)?;
    controller.core.set_desktop_quality_defaults(defaults)?;
    assert_eq!(
        controller.core.controller_session(first.id)?.quality,
        first.quality,
        "new defaults leave an open connection's quality"
    );
    let second = controller.connect_control(&hosts[1])?;
    assert_eq!(
        second.quality,
        Some(defaults),
        "a connection opened afterwards starts at the defaults"
    );
    assert_eq!(
        hosts[1]
            .core
            .snapshot()?
            .incoming
            .ok_or("the second host's session")?
            .quality,
        Some(defaults),
        "its host starts at the defaults too"
    );
    expect![[r#"
        controller:
          session: session phase from=connecting generation=1 role=Controller session=1 to=active
          session: session phase from=connecting generation=1 role=Controller session=2 to=active
    "#]]
    .assert_eq(&trace.story(&["session", "stream", "encode"]));
    Ok(())
}

/// The quality the viewer chooses reaches the host, the host may set its
/// own, each side tells its application, and an edit for no session is
/// refused.
#[test]
fn quality_updates_propagate_from_controller_to_host() -> TestResult {
    let trace = traced();
    let pair = Pair::for_desktop();
    let (host, controller) = (&pair.host.core, &pair.controller.core);
    let host_events = host.subscribe()?;
    let controller_events = controller.subscribe()?;
    let session = pair.connect_control();
    let incoming = host.snapshot()?.incoming.ok_or("the host's session")?;
    assert!(
        session.quality.is_some(),
        "negotiation agrees a quality at connect"
    );
    assert_eq!(
        incoming.quality, session.quality,
        "both sides record the agreed quality"
    );
    let quality = Quality::new(1_500, 30, 1280, 720)?;
    controller.update_session_quality(session.id, SessionRole::Controller, quality)?;
    assert_eq!(
        controller
            .snapshot()?
            .outgoing
            .ok_or("the viewer's session")?
            .quality,
        Some(quality),
        "the viewer applies its own edit at once"
    );
    // The viewer queues the change; the host applies it on receipt.
    wait_for(
        "the host to apply the viewer's quality",
        DEFAULT_WAIT,
        || (host.snapshot().ok()?.incoming?.quality == Some(quality)).then_some(()),
    );
    let host_quality = Quality::new(800, 15, 640, 480)?;
    host.update_session_quality(incoming.id, SessionRole::Host, host_quality)?;
    assert_eq!(
        host.snapshot()?
            .incoming
            .ok_or("the host's session")?
            .quality,
        Some(host_quality),
        "the host applies its own edit at once"
    );
    assert!(
        matches!(
            host.update_session_quality(incoming.id + 1, SessionRole::Host, quality),
            Err(Error::NoSession)
        ),
        "the host refuses an edit for no session"
    );
    assert!(
        matches!(
            controller.update_session_quality(0, SessionRole::Controller, quality),
            Err(Error::NoSession)
        ),
        "the viewer refuses an edit for no session"
    );
    assert_eq!(
        quality_update(&host_events),
        (incoming.id, quality),
        "the host tells its application the viewer's quality"
    );
    assert_eq!(
        quality_update(&controller_events),
        (session.id, quality),
        "the viewer tells its application its quality"
    );
    expect![[r#"
        controller:
          session: session phase from=connecting generation=1 role=Controller session=1 to=active
    "#]]
    .assert_eq(&trace.story(&["session", "stream", "encode"]));
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
                .find(|target| target.service)
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
    "#]].assert_eq(&trace.story(&["service"]));
    Ok(())
}

/// A viewer's explicit codec governs a desktop session too: the host
/// encodes what the viewer chose, and a codec the host cannot encode
/// refuses the session.
#[test]
fn explicit_codec_also_controls_direct_desktop_connections() -> TestResult {
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
        pair.connect_control();
        assert_eq!(
            host.video_targets()?
                .first()
                .and_then(|target| VideoCodec::from_wire_id(target.codec)),
            Some(codec),
            "the host encodes the viewer's explicit codec"
        );
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
        pair.controller.connect_control(&pair.host).is_err(),
        "a codec the host cannot encode refuses the session"
    );
    assert!(
        host.video_targets()?.is_empty(),
        "a refused session leaves the host nothing to encode"
    );
    expect![[r#"
        controller:
          session: session phase from=connecting generation=1 role=Controller session=1 to=active
          session: session phase from=active generation=1 role=Controller session=1 to=closed
          session: session phase from=connecting generation=1 role=Controller session=1 to=active
          session: session phase from=active generation=1 role=Controller session=1 to=closed
          session: session phase from=connecting generation=1 role=Controller session=1 to=active
          session: session phase from=active generation=1 role=Controller session=1 to=closed
          session: session phase from=connecting generation=1 role=Controller session=1 to=active
          session: session phase from=active generation=1 role=Controller session=1 to=closed
          session: session phase from=connecting generation=1 role=Controller session=1 to=active
          session: session phase from=active generation=1 role=Controller session=1 to=closed
          session: session failed failure=unsupported reason=the host cannot share its desktop session=1
          session: session phase from=connecting generation=0 role=Controller session=1 to=failed
        host:
          session: session phase from=active generation=1 role=Host session=1 to=closed
          session: session phase from=active generation=1 role=Host session=1 to=closed
          session: session phase from=active generation=1 role=Host session=1 to=closed
          session: session phase from=active generation=1 role=Host session=1 to=closed
          session: session phase from=active generation=1 role=Host session=1 to=closed
    "#]].assert_eq(&trace.story(&["session"]));
    Ok(())
}
