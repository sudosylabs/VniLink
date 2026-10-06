//! Encoded video for one monitor.
//!
//! The platform captures, encodes, and draws. This module carries H.264 bytes,
//! drops predicted frames until the next keyframe, and does not look inside the
//! payload. Those bytes are screen content: do not log them.

mod datagram;
mod frame;
mod queue;

pub(crate) use datagram::{fragment, Reassembler, FRAG_HEADER};
pub use frame::EncodedFrame;
pub(crate) use frame::{check_video_len, decode_video_body, encode_video_body};
pub use queue::VideoSubscription;
pub(crate) use queue::{KeyframeFlag, VideoInbox, VideoOutbox, VideoPush};
#[cfg(test)]
pub(crate) use queue::{VideoQueue, VIDEO_QUEUE};

/// Largest encoded payload, not counting the 10-byte video header.
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
