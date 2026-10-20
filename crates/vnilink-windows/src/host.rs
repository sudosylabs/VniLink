//! Publish one monitor into a hosted session.
//!
//! The generic [`HostSession`] opens the Media Foundation encoder at the even
//! size that fits the session quality. This module only pins that choice to
//! [`WindowsEncoder`], reads Desktop Duplication frames as the [`FrameSource`],
//! and serves an already-open capture. It does not reach the secure desktop,
//! and it does not draw. Frame bytes are not logged.

#[cfg(windows)]
use std::sync::atomic::AtomicBool;
pub use vnilink_media::HostSession;
#[cfg(windows)]
use vnilink_media::{Error, FrameSource, PublishError, Quality, RawFrame};

#[cfg(windows)]
use crate::encode::WindowsEncoder;

#[cfg(windows)]
impl FrameSource for crate::capture::WindowsCapture {
    fn size(&self) -> (u16, u16) {
        self.size()
    }

    /// A zero-timeout native acquire, with cached pixels for an idle desktop.
    fn poll_frame(&self, timestamp_us: u64) -> Result<Option<RawFrame>, Error> {
        self.poll_frame(timestamp_us)
    }

    fn attested_at(&self) -> Option<std::time::Instant> {
        crate::capture::WindowsCapture::attested_at(self)
    }

    fn attested_display(&self) -> &str {
        "0"
    }

    fn cursor(&self) -> Option<vnilink::CursorState> {
        crate::capture::WindowsCapture::cursor(self)
    }
}

/// Copy Desktop Duplication frames into `core` until `stop` is set or the
/// hosted session ends.
///
/// The caller has already opened `capture`. This function never reaches the
/// secure desktop. `stop` is read between frames, so a frame already being
/// copied is finished first.
#[cfg(windows)]
pub fn serve_windows(
    capture: &crate::capture::WindowsCapture,
    core: &vnilink::Core,
    session_id: u64,
    quality: Quality,
    stop: &AtomicBool,
) -> Result<(), PublishError> {
    let codec = core.host_codec(session_id).unwrap_or(vnilink::CODEC_H264);
    vnilink_media::serve_capture(
        capture,
        core,
        session_id,
        quality,
        stop,
        move |w, h, fps, bps| WindowsEncoder::open_codec(w, h, fps, bps, codec),
    )
}
