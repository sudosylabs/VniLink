//! Publish one monitor into a hosted session.
//!
//! The generic [`HostSession`] opens the VideoToolbox encoder at the even size
//! that fits the session quality. This module only pins that choice to
//! [`MacEncoder`], reads ScreenCaptureKit frames as the [`FrameSource`], and
//! serves an already-open capture. It does not show a permission dialog, and it
//! does not draw. Frame bytes are not logged.

#[cfg(target_os = "macos")]
use std::sync::atomic::AtomicBool;

pub use vnilink_media::HostSession;
#[cfg(target_os = "macos")]
use vnilink_media::{Error, FrameSource, PublishError, Quality, RawFrame};

#[cfg(target_os = "macos")]
use crate::encode::MacEncoder;

#[cfg(target_os = "macos")]
impl FrameSource for crate::capture::ScreenCapture {
    fn size(&self) -> (u16, u16) {
        self.size()
    }

    fn next_frame(&self, timestamp_us: u64) -> Result<RawFrame, Error> {
        self.next_frame(timestamp_us)
    }
}

/// Copy ScreenCaptureKit frames into `core` until `stop` is set or the hosted
/// session ends.
///
/// The caller has already started `capture`. This function does not ask for
/// Screen Recording permission. `stop` is read between frames, so a frame
/// already being copied is finished first. The encoder profile follows the
/// negotiated session codec.
#[cfg(target_os = "macos")]
pub fn serve_stream(
    capture: &crate::capture::ScreenCapture,
    core: &vnilink::Core,
    session_id: u64,
    quality: Quality,
    stop: &AtomicBool,
) -> Result<(), PublishError> {
    let high = core
        .host_codec(session_id)
        .is_ok_and(|codec| codec == vnilink::CODEC_H264_HIGH);
    vnilink_media::serve_capture(
        capture,
        core,
        session_id,
        quality,
        stop,
        move |w, h, fps, bps| MacEncoder::open(w, h, fps, bps, high),
    )
}
