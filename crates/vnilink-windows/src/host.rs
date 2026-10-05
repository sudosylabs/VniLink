//! Publish one monitor into a hosted session.
//!
//! The generic [`HostSession`] opens the Media Foundation encoder at the even
//! size that fits the session quality. This module only pins that choice to
//! [`WindowsEncoder`], reads Desktop Duplication frames as the [`FrameSource`],
//! and serves an already-open capture. It does not reach the secure desktop,
//! and it does not draw. Frame bytes are not logged.

#[cfg(windows)]
use std::sync::atomic::AtomicBool;
#[cfg(windows)]
use std::time::Duration;

pub use vnilink_media::HostSession;
#[cfg(windows)]
use vnilink_media::{Error, FrameSource, PublishError, Quality, RawFrame};

#[cfg(windows)]
use crate::encode::WindowsEncoder;

#[cfg(windows)]
const FRAME_POLL_INTERVAL: Duration = Duration::from_millis(10);

#[cfg(windows)]
impl FrameSource for crate::capture::WindowsCapture {
    fn size(&self) -> (u16, u16) {
        self.size()
    }

    /// Polls Desktop Duplication until a newer frame arrives. The zero-timeout
    /// acquire never waits itself; this loop is the wait. An unchanged desktop
    /// produces no frames, which is normal: the wait has no deadline, and the
    /// serve loop notices `stop` once a frame arrives.
    fn next_frame(&self, timestamp_us: u64) -> Result<RawFrame, Error> {
        loop {
            if let Some(frame) = self.poll_frame(timestamp_us)? {
                return Ok(frame);
            }
            std::thread::sleep(FRAME_POLL_INTERVAL);
        }
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
    vnilink_media::serve_capture(
        capture,
        core,
        session_id,
        quality,
        stop,
        WindowsEncoder::open,
    )
}
