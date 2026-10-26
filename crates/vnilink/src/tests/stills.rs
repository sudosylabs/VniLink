//! Still pictures: an AV1 4:4:4 picture of a still screen, carried outside
//! the stream's codec only where both sides agreed to it.

use crate::video::{EncodedFrame, VideoInbox, VideoPush, VideoQueue};
use crate::FrameRole;
use std::time::{Duration, Instant};

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
