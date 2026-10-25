//! Still pictures: an AV1 4:4:4 picture of a still screen, carried outside
//! the stream's codec only where both sides agreed to it.

use super::support::{pair_for_control, start_direct};
use crate::video::{EncodedFrame, VideoInbox, VideoPush, VideoQueue};
use crate::{Core, Error, FrameRole, ServiceKind, SessionPhase, VideoCodec, WindowDecoder};
use std::time::{Duration, Instant};

fn wait(mut condition: impl FnMut() -> bool) {
    let until = Instant::now() + Duration::from_secs(8);
    while !condition() {
        assert!(Instant::now() < until, "condition timed out");
        std::thread::sleep(Duration::from_millis(10));
    }
}

struct Capture {
    stills: bool,
}
impl crate::HostCapture for Capture {
    fn monitor(&self) -> (u16, u16) {
        (1920, 1080)
    }
    fn stills(&self) -> bool {
        self.stills
    }
}

/// A window that decodes H.264, and AV1 at 4:4:4 when `shows_stills`.
fn window(core: &Core, shows_stills: bool) {
    core.set_window_decoders(vec![
        WindowDecoder {
            codec: VideoCodec::H264,
            hardware: false,
            chroma444: false,
            bt601: false,
        },
        WindowDecoder {
            codec: VideoCodec::Av1,
            hardware: false,
            chroma444: shows_stills,
            bt601: false,
        },
    ])
    .unwrap();
}

/// A neutral desktop opened between a host whose capture sends stills or
/// not and a viewer whose window shows them or not.
fn open(
    host_sends: bool,
    viewer_shows: bool,
) -> (Core, Core, crate::RemoteDesktopService, crate::ServiceInfo) {
    let host = start_direct();
    let controller = start_direct();
    host.set_capture(Box::new(Capture { stills: host_sends }))
        .unwrap();
    window(&controller, viewer_shows);
    pair_for_control(&host, &controller);
    let pending = controller
        .begin_connect_services(&host.endpoint_id())
        .unwrap();
    wait(|| controller.controller_session(pending.id).unwrap().phase == SessionPhase::Active);
    let parent = controller.controller_session(pending.id).unwrap();
    let owner = controller
        .attach_controller_service(parent.id, parent.generation, ServiceKind::Desktop)
        .unwrap();
    let desktop = controller
        .open_desktop_service(owner.id, owner.generation)
        .unwrap();
    let remote = host
        .hosted_services()
        .unwrap()
        .into_iter()
        .find(|s| s.kind == ServiceKind::Desktop)
        .unwrap();
    (host, controller, desktop, remote)
}

#[test]
fn stills_are_agreed_only_when_the_window_shows_them_and_the_host_sends_them() {
    for (sends, shows) in [(true, true), (true, false), (false, true), (false, false)] {
        let (host, _controller, desktop, remote) = open(sends, shows);
        let agreed = sends && shows;
        assert_eq!(
            desktop.desktop_info().unwrap().stills,
            agreed,
            "{sends} {shows}"
        );
        assert_eq!(
            host.hosted_desktop_info(remote.id, remote.generation)
                .unwrap()
                .stills,
            agreed,
            "{sends} {shows}"
        );
        desktop.close();
    }
}

#[test]
fn an_agreed_still_reaches_the_viewer_outside_the_streams_codec() {
    for stream in [false, true] {
        let (host, _controller, desktop, remote) = open(true, true);
        if stream {
            host.force_desktop_stream(remote.id, remote.generation);
        }
        let video = desktop.subscribe_video().unwrap();
        let publish = |frame| host.publish_desktop_video(remote.id, remote.generation, frame);
        publish(EncodedFrame::h264(true, 1, vec![7; 2_000]).unwrap()).unwrap();
        let first = video.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(first.role, FrameRole::Changed);
        publish(EncodedFrame::still(2, vec![9; 20_000]).unwrap()).unwrap();
        let still = video.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(
            (still.role, still.codec),
            (FrameRole::Still, VideoCodec::Av1)
        );
        assert_eq!(still.payload, vec![9; 20_000]);
        publish(
            EncodedFrame::h264(false, 3, vec![8; 500])
                .unwrap()
                .unchanged(),
        )
        .unwrap();
        let after = video.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(after.role, FrameRole::Unchanged, "the stream went on");
        let pipeline = host
            .diagnostic_snapshot()
            .unwrap()
            .pipelines
            .into_iter()
            .find(|p| p.hosted)
            .unwrap();
        assert_eq!((pipeline.stills_sent, pipeline.still_bytes), (1, 20_000));
        assert_eq!(
            pipeline.keyframes_sent, 1,
            "a still is not the stream's keyframe"
        );
        desktop.close();
    }
}

#[test]
fn a_still_is_refused_where_it_was_not_agreed() {
    let (host, _controller, desktop, remote) = open(true, false);
    assert!(matches!(
        host.publish_desktop_video(
            remote.id,
            remote.generation,
            EncodedFrame::still(1, vec![1; 100]).unwrap()
        ),
        Err(Error::Unsupported(_))
    ));
    desktop.close();
}

#[test]
fn a_still_waits_in_turn_and_owes_nothing() {
    let picture = |keyframe, at| EncodedFrame::h264(keyframe, at, vec![1]).unwrap();
    let still = |at| EncodedFrame::still(at, vec![2]).unwrap();
    let mut queue = VideoQueue::new();
    assert_eq!(queue.push(picture(true, 1)), VideoPush::Queued);
    assert_eq!(
        queue.push(still(2)),
        VideoPush::Queued,
        "behind the picture before it"
    );
    assert_eq!(queue.pop().unwrap().timestamp_us, 1);
    assert_eq!(queue.pop().unwrap().role, FrameRole::Still);
    // A full queue leaves a still out rather than drop the stream's frames.
    for at in 0..crate::video::VIDEO_QUEUE as u64 {
        queue.push(picture(at == 0, 10 + at));
    }
    assert_eq!(queue.push(still(30)), VideoPush::Skipped);
    while queue.pop().is_some() {}
    // A loss leaves the stream waiting for a keyframe; a still neither
    // answers it nor lets the predicted frames through.
    queue.lost();
    assert_eq!(queue.push(still(4)), VideoPush::Queued);
    assert_eq!(queue.push(picture(false, 5)), VideoPush::StillNeedsKeyframe);
    // A keyframe replaces a still waiting before it, uncounted.
    let dropped = queue.dropped();
    assert_eq!(queue.push(picture(true, 6)), VideoPush::Queued);
    assert_eq!(queue.dropped(), dropped);
    assert_eq!(queue.len(), 1);
    assert_eq!(queue.pop().unwrap().timestamp_us, 6);
}

#[test]
fn a_still_is_shown_but_never_recorded() {
    let inbox = VideoInbox::new();
    let tap = inbox.recording_tap().unwrap();
    inbox.push(EncodedFrame::h264(true, 1, vec![1]).unwrap());
    inbox.push(EncodedFrame::still(2, vec![2]).unwrap());
    assert_eq!(inbox.try_pop().unwrap().timestamp_us, 1);
    assert_eq!(inbox.try_pop().unwrap().role, FrameRole::Still);
    let recorded: Vec<_> = std::iter::from_fn(|| tap.try_recv())
        .map(|frame| frame.timestamp_us)
        .collect();
    assert_eq!(recorded, [1]);
}

#[test]
fn a_still_fits_what_the_link_drains_in_a_moment() {
    // 8 Mb/s carries 250 kB in the quarter second a still may hold it.
    assert_eq!(crate::video::still_drain(8_000_000), 250_000);
}

/// A still is made to the size the send gate admits: its datagrams, their
/// prefixes and its parity included. Sized to the drain itself, every still
/// on a 12 Mb/s link was refused (374,989 encoded bytes, 422,786 to 557,116
/// on the wire, against 375,000).
#[test]
fn a_still_made_to_its_budget_is_not_refused_for_its_framing() {
    let cost = |payload: usize, max: usize, protection: f64| {
        let (datagrams, bytes) =
            crate::video::fragmented_size(crate::video::VIDEO_HEADER + payload, max, protection);
        bytes + datagrams * 20
    };
    let wire = crate::video::still_drain(12_000_000);
    for max in [1180, 1432] {
        for protection in [0.1, 0.33, crate::video::MAX_PROTECTION] {
            let payload = crate::video::still_payload(wire, max, 20, protection);
            assert!(cost(payload, max, protection) <= wire);
            assert!(
                cost(payload + 1, max, protection) > wire,
                "the largest that fits"
            );
            assert!(payload < wire);
        }
    }
    // A link too slow for any picture takes none, and none exceeds a frame.
    assert_eq!(crate::video::still_payload(10, 1180, 20, 0.1), 0);
    assert!(crate::video::still_payload(usize::MAX / 4, 1180, 20, 0.0) <= crate::MAX_VIDEO_FRAME);
}

/// An idle link's round trip is its floor while its window is still sized
/// for the loaded link: the rate is the window over the round trip the link
/// showed lately, and a lone spike is not that.
#[test]
fn the_link_carries_its_window_over_its_loaded_round_trip() {
    use crate::video::LinkRate;
    let start = Instant::now();
    let at = |ms: u64| start + Duration::from_millis(ms);
    let mut link = LinkRate::default();
    assert_eq!(link.bits_per_second(), None, "nothing read yet");
    // A second of motion at 30 fps on 10 Mb/s with 270 ms of round trip
    // under load: a 337.5 kB window.
    for frame in 0..30 {
        link.observe(337_500, Duration::from_millis(270), at(frame * 33));
    }
    assert_eq!(link.bits_per_second(), Some(10_000_000));
    // Then still for two seconds: the queue drains and the round trip
    // falls to its floor, but the rate does not rise.
    for second in 1..=2 {
        link.observe(337_500, Duration::from_millis(20), at(1_000 * second));
    }
    assert_eq!(link.bits_per_second(), Some(10_000_000));
    // A smaller window after a loss lowers it at once.
    link.observe(168_750, Duration::from_millis(20), at(2_500));
    assert_eq!(link.bits_per_second(), Some(5_000_000));
    // Long after the load, the idle round trip is what the link shows.
    link.observe(25_000, Duration::from_millis(20), at(15_000));
    assert_eq!(link.bits_per_second(), Some(10_000_000));
    // A lone spike on an idle link is not its load.
    let mut idle = LinkRate::default();
    for frame in 0..40 {
        idle.observe(13_200, Duration::from_millis(1), at(frame * 33));
    }
    idle.observe(13_200, Duration::from_millis(30), at(1_400));
    assert_eq!(idle.bits_per_second(), Some(105_600_000));
    link.reset();
    assert_eq!(link.bits_per_second(), None, "a new path is unknown");
}
