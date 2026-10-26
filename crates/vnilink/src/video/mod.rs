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
mod viewer;

pub(crate) use counters::{VideoCounters, VideoCounts};
pub(crate) use datagram::{
    fragment, fragmented_size, parity_count, FragmentCounts, Losses, Reassembler, Unfinished,
    FRAG_HEADER, MAX_PROTECTION,
};
pub(crate) use frame::micros_since;
pub(crate) use frame::{
    check_video_len, decode_video_body, encode_video_body, VideoBody, VIDEO_HEADER,
};
pub use frame::{EncodedFrame, FrameRole, FrameTiming, Region, MAX_REGIONS};
pub(crate) use queue::{KeyframeFlag, QueueTelemetry, VideoInbox, VideoOutbox, VideoPush};
pub use queue::{RecordingVideoTap, VideoSubscription};
#[cfg(test)]
pub(crate) use queue::{VideoQueue, INBOX_MAX_AGE, VIDEO_QUEUE};
pub(crate) use recovery::KeyframeRequests;
pub(crate) use report::{Congestion, ReportTally, VideoReport, ViewerLoss, REPORT_LEN};
pub(crate) use viewer::{Order, ViewerPictures};

/// Largest encoded payload, not counting the video header before it.
pub const MAX_VIDEO_FRAME: usize = 1_048_576;

/// How long a still picture may hold the link: a change right after it
/// waits behind it.
pub const STILL_DRAIN: std::time::Duration = std::time::Duration::from_millis(250);

/// How long one encoded picture may hold the link: every encoder's rate
/// control buffer. A change of the whole screen then arrives sharp in one
/// picture rather than smeared over seconds of refinement, at the cost of
/// holding the pictures after it this long at most on a link no faster than
/// the bitrate. FFmpeg's NVENC default, which RustDesk keeps, is two
/// seconds; one picture's interval left a changed screen coarse while
/// anything else on it moved.
pub const PICTURE_DRAIN: std::time::Duration = std::time::Duration::from_millis(250);

/// The rate control buffer for `bitrate_bps`, in bits: [`PICTURE_DRAIN`]
/// of it.
pub fn picture_buffer_bits(bitrate_bps: u32) -> u32 {
    let bits = u64::from(bitrate_bps) * PICTURE_DRAIN.as_millis() as u64 / 1000;
    u32::try_from(bits).unwrap_or(u32::MAX).max(1)
}

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

#[cfg(test)]
mod tests;
