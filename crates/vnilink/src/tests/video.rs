use std::sync::Arc;
use std::time::{Duration, Instant};

use iroh::RelayMode;

use crate::core::{Core, CoreConfig, Listen};
use crate::error::Error;
use crate::video::{
    decode_video_body, encode_video_body, EncodedFrame, VideoPush, VideoQueue, MAX_VIDEO_FRAME,
    VIDEO_QUEUE,
};

fn start_direct() -> Core {
    let core = Core::boot(
        CoreConfig {
            listen: Listen::Loopback,
            ..CoreConfig::default()
        },
        RelayMode::Disabled,
    )
    .expect("loopback core");
    core.set_video_codecs(crate::VideoCodecCaps::h264_full(4096, 4096, "test"))
        .expect("flags");
    core
}

#[test]
fn video_api_rejects_a_second_subscriber_and_a_missing_session() {
    let core = start_direct();
    assert!(!core.capabilities().desktop_capture);
    assert!(matches!(
        core.publish_video(1, EncodedFrame::h264(true, 1, vec![1]).unwrap()),
        Err(Error::NoSession)
    ));
    let first = core.subscribe_video().unwrap();
    assert!(matches!(core.subscribe_video(), Err(Error::Busy)));
    drop(first);
    assert!(core.subscribe_video().is_ok());
}

fn frame(keyframe: bool, timestamp_us: u64) -> EncodedFrame {
    EncodedFrame::h264(keyframe, timestamp_us, vec![1]).unwrap()
}

#[test]
fn an_overflow_episode_requests_one_keyframe_until_it_arrives() {
    let mut queue = VideoQueue::new();
    assert_eq!(queue.push(frame(true, 1)), VideoPush::Queued);
    // Fill to the bound with predicted frames.
    for index in 0..VIDEO_QUEUE - 1 {
        assert_eq!(
            queue.push(frame(false, 2 + index as u64)),
            VideoPush::Queued
        );
    }
    // The next predicted frame overflows: exactly one keyframe request for
    // the whole episode, then silence until the keyframe arrives.
    assert_eq!(queue.push(frame(false, 99)), VideoPush::KeyframeNeeded);
    for timestamp in 100..110u64 {
        assert_eq!(
            queue.push(frame(false, timestamp)),
            VideoPush::StillNeedsKeyframe
        );
    }
    assert_eq!(queue.push(frame(true, 110)), VideoPush::Queued);
    assert_eq!(queue.pop().unwrap().timestamp_us, 110);
}

#[test]
fn predicted_frames_drop_until_the_next_keyframe() {
    let mut queue = VideoQueue::new();
    assert_eq!(queue.push(frame(true, 1)), VideoPush::Queued);
    for index in 0..VIDEO_QUEUE - 1 {
        assert_eq!(
            queue.push(frame(false, 2 + index as u64)),
            VideoPush::Queued
        );
    }
    assert_eq!(queue.len(), VIDEO_QUEUE);
    assert_eq!(queue.push(frame(false, 99)), VideoPush::KeyframeNeeded);
    assert_eq!(queue.len(), 1);
    let kept = queue.pop().unwrap();
    assert!(kept.keyframe);
    assert_eq!(kept.timestamp_us, 1);
    assert!(queue.dropped() >= 1);
    assert_eq!(queue.push(frame(false, 100)), VideoPush::StillNeedsKeyframe);
    assert_eq!(queue.push(frame(true, 101)), VideoPush::Queued);
    assert_eq!(queue.len(), 1);
    let dropped = queue.dropped();
    assert_eq!(queue.push(frame(true, 102)), VideoPush::Queued);
    assert!(queue.dropped() >= dropped + 1);
    assert_eq!(queue.len(), 1);
    let only = queue.pop().unwrap();
    assert!(only.keyframe);
    assert_eq!(only.timestamp_us, 102);
    assert!(queue.pop().is_none());
}

#[test]
fn a_viewer_queue_older_than_its_budget_starts_again_from_a_keyframe() {
    let start = Instant::now();
    let frame = |keyframe| EncodedFrame::h264(keyframe, 1, vec![0x41]).unwrap();
    let mut queue = VideoQueue::new().with_max_age(crate::video::INBOX_MAX_AGE);
    assert_eq!(queue.push_at(frame(true), start), VideoPush::Queued);
    assert_eq!(
        queue.push_at(frame(false), start + Duration::from_millis(100)),
        VideoPush::Queued,
        "two frames within the budget are jitter, not delay"
    );
    // The window has not taken the first frame for longer than the budget:
    // the predicted frames go, and a keyframe is wanted.
    assert_eq!(
        queue.push_at(frame(false), start + Duration::from_millis(151)),
        VideoPush::KeyframeNeeded
    );
    assert_eq!(queue.len(), 1, "the buffered keyframe stays");
    assert_eq!(
        queue.push_at(frame(true), start + Duration::from_millis(160)),
        VideoPush::Queued
    );
    // A queue without a budget is bounded by depth only.
    let mut deep = VideoQueue::new();
    deep.push_at(frame(true), start);
    assert_eq!(
        deep.push_at(frame(false), start + Duration::from_secs(5)),
        VideoPush::Queued
    );
}

#[test]
fn a_frame_carries_its_capture_to_send_time_and_its_queued_time() {
    let captured = Instant::now() - Duration::from_millis(40);
    let frame = EncodedFrame::h264(true, 1, vec![0x65])
        .unwrap()
        .with_captured_at(captured);
    let sent = decode_video_body(&encode_video_body(&frame)).unwrap();
    assert!(
        (40_000..10_000_000).contains(&sent.timing.host_us),
        "capture to send was {} µs",
        sent.timing.host_us
    );
    assert_eq!(
        sent.timing.captured_at, None,
        "the capture instant stays on the host"
    );
    // A received frame passed on keeps the time it arrived with.
    let passed = decode_video_body(&encode_video_body(&sent)).unwrap();
    assert_eq!(passed.timing.host_us, sent.timing.host_us);
    // Without a capture instant the time is unknown: zero.
    let unknown = EncodedFrame::h264(true, 1, vec![0x65]).unwrap();
    assert_eq!(
        decode_video_body(&encode_video_body(&unknown))
            .unwrap()
            .timing
            .host_us,
        0
    );

    let mut queue = VideoQueue::new();
    queue.push_at(sent, Instant::now() - Duration::from_millis(25));
    let popped = queue.pop().unwrap();
    assert!(
        popped.timing.queued_us >= 25_000,
        "queued {} µs",
        popped.timing.queued_us
    );
}

/// A changed picture says where it changed; more places than a frame
/// carries, or a picture that is not a change, say nothing.
#[test]
fn a_changed_frame_carries_where_it_changed() {
    use crate::{Region, MAX_REGIONS};
    let clock = Region {
        x: 1800,
        y: 1040,
        width: 96,
        height: 24,
    };
    let frame = EncodedFrame::h264(false, 5, vec![0x41, 2])
        .unwrap()
        .with_regions(Some(vec![clock]));
    let back = decode_video_body(&encode_video_body(&frame)).unwrap();
    assert_eq!(back.regions, Some(vec![clock]));
    assert_eq!(back.payload, vec![0x41, 2]);
    // None changed: an empty list is a statement too.
    let none = frame.clone().with_regions(Some(Vec::new()));
    assert_eq!(
        decode_video_body(&encode_video_body(&none))
            .unwrap()
            .regions,
        Some(Vec::new())
    );
    let many = frame
        .clone()
        .with_regions(Some(vec![clock; MAX_REGIONS + 1]));
    assert_eq!(many.regions, None);
    assert_eq!(
        decode_video_body(&encode_video_body(&many))
            .unwrap()
            .regions,
        None
    );
    // A repeat of the screen says nothing of regions, whatever it holds.
    let repeat = frame.clone().unchanged();
    assert_eq!(
        decode_video_body(&encode_video_body(&repeat))
            .unwrap()
            .regions,
        None
    );
    // Regions on a repeat, and a count past the limit or past the body, are
    // malformed.
    let mut body = encode_video_body(&frame);
    body[1] |= 4;
    assert!(matches!(decode_video_body(&body), Err(Error::Malformed)));
    let mut body = encode_video_body(&frame);
    body[30] = (MAX_REGIONS + 1) as u8;
    assert!(matches!(decode_video_body(&body), Err(Error::Malformed)));
    let mut body = encode_video_body(&frame);
    body[30] = 2;
    assert!(matches!(decode_video_body(&body), Err(Error::Malformed)));
}

#[test]
fn h264_frames_round_trip_and_reject_a_bad_payload() {
    assert!(matches!(
        EncodedFrame::h264(true, 1, Vec::new()),
        Err(Error::InvalidSetting("encoded frame is empty"))
    ));
    assert!(matches!(
        EncodedFrame::h264(true, 1, vec![0; MAX_VIDEO_FRAME + 1]),
        Err(Error::InvalidSetting("encoded frame is larger than 1 MiB"))
    ));
    let small = EncodedFrame::h264(false, u64::MAX, vec![0x7F]).unwrap();
    assert_eq!(
        decode_video_body(&encode_video_body(&small)).unwrap(),
        small
    );
    let stamped = EncodedFrame::h264(true, 9, vec![0x65, 1])
        .unwrap()
        .with_source(3, 7);
    let back = decode_video_body(&encode_video_body(&stamped)).unwrap();
    assert_eq!(back, stamped);
    assert_eq!((back.display_id, back.revision), (3, 7));

    let mut flags = encode_video_body(&EncodedFrame::h264(true, 7, vec![4]).unwrap());
    // Bits past the keyframe, role and region bits are ignored.
    flags[1] = 0xF1;
    assert!(decode_video_body(&flags).unwrap().keyframe);
    flags[1] = 0xF0;
    assert!(!decode_video_body(&flags).unwrap().keyframe);

    assert!(matches!(
        decode_video_body(&[
            9, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 1
        ]),
        Err(Error::Unsupported("this video codec"))
    ));
    assert!(matches!(decode_video_body(&[1, 0]), Err(Error::Malformed)));
    assert!(matches!(
        decode_video_body(&[1, 1, 0, 0, 0, 0, 0, 0, 0, 0]),
        Err(Error::Malformed)
    ));
    // The old 10-byte header is too short to carry a source stamp.
    assert!(matches!(
        decode_video_body(&[1, 1, 0, 0, 0, 0, 0, 0, 0, 0, 1]),
        Err(Error::Malformed)
    ));

    let mut mutated = EncodedFrame::h264(true, 1, vec![0xAB, 0xCD]).unwrap();
    let rendered = format!("{mutated:?}");
    assert!(rendered.contains("payload_len"));
    assert!(!rendered.contains("171"));
    assert!(!rendered.contains("205"));
    mutated.payload.clear();
    assert!(matches!(
        mutated.check(),
        Err(Error::InvalidSetting("encoded frame is empty"))
    ));
    mutated.payload.resize(MAX_VIDEO_FRAME + 1, 0);
    assert!(matches!(
        mutated.check(),
        Err(Error::InvalidSetting("encoded frame is larger than 1 MiB"))
    ));
}

/// A reader told when a frame is queued need not poll; once it is gone, the
/// network task calls nothing.
#[test]
fn a_subscription_is_woken_when_a_frame_is_queued_until_it_is_dropped() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let inbox = Arc::new(crate::video::VideoInbox::new());
    let subscription = crate::VideoSubscription::new(Arc::clone(&inbox), Arc::new(()));
    let woken = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&woken);
    subscription.on_ready(move || {
        counter.fetch_add(1, Ordering::Relaxed);
    });
    let frame = |keyframe| EncodedFrame::h264(keyframe, 1, vec![0x65]).unwrap();
    inbox.push(frame(true));
    assert_eq!(woken.load(Ordering::Relaxed), 1);
    assert!(subscription.try_recv().is_some());
    // A frame dropped while a keyframe is owed wakes no one.
    inbox.lost();
    inbox.push(frame(false));
    assert_eq!(woken.load(Ordering::Relaxed), 1);
    drop(subscription);
    inbox.push(frame(true));
    assert_eq!(
        woken.load(Ordering::Relaxed),
        1,
        "a dropped reader was woken"
    );
}
