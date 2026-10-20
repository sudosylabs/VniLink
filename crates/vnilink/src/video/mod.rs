//! Encoded video for one monitor.
//!
//! The platform captures, encodes, and draws. This module carries H.264 bytes,
//! drops predicted frames until the next keyframe, and does not look inside the
//! payload. Those bytes are screen content: do not log them.

mod codec;
pub use codec::{CodecSupport, VideoCodec};
mod counters;
mod datagram;
mod frame;
mod queue;
mod recovery;

pub(crate) use counters::{VideoCounters, VideoCounts};
pub(crate) use datagram::{fragment, Losses, Reassembler, FRAG_HEADER};
pub(crate) use frame::micros_since;
pub(crate) use frame::{check_video_len, decode_video_body, encode_video_body};
pub use frame::{EncodedFrame, FrameTiming};
pub(crate) use queue::{KeyframeFlag, QueueTelemetry, VideoInbox, VideoOutbox, VideoPush};
pub use queue::{RecordingVideoTap, VideoSubscription};
#[cfg(test)]
pub(crate) use queue::{VideoQueue, INBOX_MAX_AGE, VIDEO_QUEUE};
pub(crate) use recovery::KeyframeRequests;

/// Largest encoded payload, not counting the video header before it.
pub const MAX_VIDEO_FRAME: usize = 1_048_576;

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
}
