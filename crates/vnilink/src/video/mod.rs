//! Encoded video for one monitor.
//!
//! The platform captures, encodes, and draws. This module carries H.264 bytes,
//! drops predicted frames until the next keyframe, and does not look inside the
//! payload. Those bytes are screen content: do not log them.

mod codec;
pub use codec::{CodecSupport, VideoCodec, YuvMatrix};
mod condition;
pub use condition::{Condition, ConditionEntry, ConditionSet, Evidence};
pub(crate) use condition::{Conditions, HostState, Observation};
mod counters;
mod datagram;
mod frame;
mod link_rate;
pub(crate) use link_rate::LinkRate;
mod queue;
mod recovery;
mod report;

pub(crate) use counters::{VideoCounters, VideoCounts};
#[cfg(test)]
pub(crate) use datagram::MAX_PARTIAL_BYTES;
pub(crate) use datagram::{
    fragment, fragmented_size, parity_count, FragmentCounts, Losses, Reassembler, FRAG_HEADER,
    MAX_PROTECTION,
};
pub(crate) use frame::micros_since;
pub(crate) use frame::{check_video_len, decode_video_body, encode_video_body, VIDEO_HEADER};
pub use frame::{EncodedFrame, FrameRole, FrameTiming};
pub(crate) use queue::{KeyframeFlag, QueueTelemetry, VideoInbox, VideoOutbox, VideoPush};
pub use queue::{RecordingVideoTap, VideoSubscription};
#[cfg(test)]
pub(crate) use queue::{VideoQueue, INBOX_MAX_AGE, VIDEO_QUEUE};
pub(crate) use recovery::KeyframeRequests;
pub(crate) use report::{Congestion, ReportTally, VideoReport, ViewerLoss, REPORT_LEN};

/// Largest encoded payload, not counting the video header before it.
pub const MAX_VIDEO_FRAME: usize = 1_048_576;

/// How long a still picture may hold the link: a change right after it
/// waits behind it.
pub const STILL_DRAIN: std::time::Duration = std::time::Duration::from_millis(250);

/// The bytes a link carrying `bits_per_second` drains within
/// [`STILL_DRAIN`]: what a still picture, and anything queued ahead of it,
/// may take on the wire.
pub(crate) fn still_drain(bits_per_second: u64) -> usize {
    let bytes = (bits_per_second / 8).saturating_mul(STILL_DRAIN.as_millis() as u64) / 1000;
    usize::try_from(bytes).unwrap_or(usize::MAX)
}

/// The largest still picture, in encoded bytes, whose datagrams take at
/// most `wire` bytes: with its frame header and `protection`'s parity, cut
/// into datagrams of `max_datagram` bytes, each behind `prefix` bytes. A
/// still is a keyframe, so it carries parity on any link; a picture the
/// size of the drain itself would be refused once fragmented.
pub(crate) fn still_payload(
    wire: usize,
    max_datagram: usize,
    prefix: usize,
    protection: f64,
) -> usize {
    let cost = |payload: usize| {
        let (datagrams, bytes) =
            fragmented_size(frame::VIDEO_HEADER + payload, max_datagram, protection);
        bytes + datagrams * prefix
    };
    if cost(0) > wire {
        return 0;
    }
    // The cost grows with the payload: the largest that fits, by halving.
    let (mut fits, mut over) = (0, wire.min(MAX_VIDEO_FRAME) + 1);
    while over - fits > 1 {
        let middle = fits + (over - fits) / 2;
        if cost(middle) <= wire {
            fits = middle;
        } else {
            over = middle;
        }
    }
    fits
}

/// Monitor size advertised for capture.
///
/// The platform owns capture and encoding and passes bytes to
/// [`crate::Core::publish_video`]. `monitor` must not call back into [`crate::Core`].
pub trait HostCapture: Send {
    fn monitor(&self) -> (u16, u16);
    /// True when the source can provide a lossless cursor outside video.
    fn separate_cursor(&self) -> bool {
        false
    }
    /// True when the host sends a viewer that shows them AV1 4:4:4 still
    /// pictures of a still screen ([`crate::FrameRole::Still`]).
    fn stills(&self) -> bool {
        false
    }
}
