use std::time::Duration;

use crate::channel::{ChannelSlot, FrameChannel};
use crate::error::Error;

fn slot(width: u16, height: u16) -> ChannelSlot {
    ChannelSlot {
        width,
        height,
        stride: usize::from(width) * 4,
        bytes: vec![7; usize::from(width) * usize::from(height) * 4].into(),
        captured_at: None,
    }
}

#[test]
fn the_latest_push_wins_and_the_second_receive_waits() {
    let channel = FrameChannel::new();
    channel.push(slot(2, 2));
    channel.push(slot(4, 4));
    let frame = channel.recv_timeout(Duration::from_secs(1), || {}).unwrap();
    assert_eq!((frame.width, frame.height), (4, 4));
    assert!(matches!(
        channel.recv_timeout(Duration::from_millis(20), || {}),
        Err(Error::CaptureRejected(
            "the capture did not deliver a frame"
        ))
    ));
}

#[test]
fn the_first_failure_sticks_and_beats_later_pushes() {
    let channel = FrameChannel::new();
    channel.fail("first");
    channel.fail("second");
    channel.push(slot(2, 2));
    assert_eq!(channel.failure(), Some("first"));
    assert!(matches!(
        channel.recv_timeout(Duration::from_secs(1), || {}),
        Err(Error::Unsupported("first"))
    ));
}

#[test]
fn an_empty_channel_times_out() {
    let channel = FrameChannel::default();
    assert!(channel.failure().is_none());
    assert!(matches!(
        channel.recv_timeout(Duration::from_millis(20), || {}),
        Err(Error::CaptureRejected(
            "the capture did not deliver a frame"
        ))
    ));
}

#[test]
fn cached_receives_pump_changes_and_do_not_hide_failures() {
    let channel = FrameChannel::new();
    assert!(matches!(
        channel.recv_latest(Duration::ZERO, || {}),
        Err(Error::CaptureRejected(
            "the capture did not deliver a frame"
        ))
    ));
    channel.push(slot(2, 2));
    let changed = channel
        .recv_latest(Duration::ZERO, || channel.push(slot(4, 4)))
        .unwrap();
    assert_eq!((changed.width, changed.height), (4, 4));
    assert!(matches!(
        channel.recv_latest(Duration::ZERO, || channel.fail("stream closed")),
        Err(Error::Unsupported("stream closed"))
    ));
}

/// A compositor may deliver the same screen again; storing it would say the
/// screen changed. A changed pixel, or another size, is stored.
#[test]
fn the_same_picture_again_is_not_a_change() {
    let channel = FrameChannel::new();
    assert!(channel.push_if_changed(slot(4, 4)));
    let pushed = channel.pushed_at();
    let first = channel.recv_timeout(Duration::from_secs(1), || {}).unwrap();
    // The same pixels in a wider stride are the same picture.
    let mut padded = vec![0; 4 * 32];
    for y in 0..4 {
        padded[y * 32..y * 32 + 16].copy_from_slice(&first.bytes[y * 16..y * 16 + 16]);
    }
    let again = ChannelSlot {
        stride: 32,
        bytes: padded.into(),
        ..slot(4, 4)
    };
    assert!(!channel.push_if_changed(again));
    assert_eq!(channel.pushed_at(), pushed);
    assert!(channel
        .recv_timeout(Duration::from_millis(20), || {})
        .is_err());
    let mut changed = vec![7; 4 * 4 * 4];
    changed[37] = 8;
    assert!(channel.push_if_changed(ChannelSlot {
        bytes: changed.into(),
        ..slot(4, 4)
    }));
    assert!(channel.pushed_at() > pushed);
    assert!(channel.push_if_changed(slot(2, 2)));
}
