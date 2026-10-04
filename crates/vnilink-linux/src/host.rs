//! Publish one monitor into a hosted session.
//!
//! The generic [`HostSession`] opens the VA-API encoder at the even size that
//! fits the session quality. This module only pins that choice to
//! [`LinuxEncoder`], reads portal frames as the [`FrameSource`], and serves an
//! already-open portal capture. It does not show a permission dialog, and it
//! does not draw. Frame bytes are not logged.

#[cfg(target_os = "linux")]
use std::sync::atomic::AtomicBool;

pub use vnilink_media::HostSession;
#[cfg(target_os = "linux")]
use vnilink_media::{Error, FrameSource, PublishError, Quality, RawFrame};

#[cfg(target_os = "linux")]
use crate::encode::LinuxEncoder;

#[cfg(target_os = "linux")]
impl FrameSource for crate::pipewire::PortalCapture {
    fn size(&self) -> (u16, u16) {
        self.size()
    }

    fn next_frame(&self, timestamp_us: u64) -> Result<RawFrame, Error> {
        self.next_frame(timestamp_us)
    }
}

/// Copy portal frames into `core` until `stop` is set or the hosted session ends.
///
/// The caller has already started `capture`. This function does not open a
/// portal. `stop` is read between frames, so a frame already being copied is
/// finished first.
#[cfg(target_os = "linux")]
pub fn serve_portal(
    capture: &crate::pipewire::PortalCapture,
    core: &vnilink::Core,
    session_id: u64,
    quality: Quality,
    stop: &AtomicBool,
) -> Result<(), PublishError> {
    vnilink_media::serve_capture(capture, core, session_id, quality, stop, LinuxEncoder::open)
}
