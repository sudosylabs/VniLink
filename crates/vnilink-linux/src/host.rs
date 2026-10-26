//! Publish one monitor into a hosted Desktop service.
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
use vnilink_media::{Error, FrameSource, FrameWait, PublishError, RawFrame};

#[cfg(target_os = "linux")]
use crate::encode::LinuxEncoder;

#[cfg(target_os = "linux")]
impl FrameSource for crate::pipewire::PortalCapture {
    fn size(&self) -> (u16, u16) {
        self.size()
    }

    fn poll_frame(&self, timestamp_us: u64) -> Result<Option<RawFrame>, Error> {
        self.poll_frame(timestamp_us)
    }

    fn wait_frame(&self, deadline: std::time::Instant) -> FrameWait {
        FrameWait::from_arrived(crate::pipewire::PortalCapture::wait_frame(self, deadline))
    }

    fn attested_at(&self) -> Option<std::time::Instant> {
        crate::pipewire::PortalCapture::attested_at(self)
    }

    fn attested_display(&self) -> &str {
        "portal"
    }
}

/// Encode an existing X11/portal source for one admitted Desktop owner.
/// This does not request or replace a Wayland permission grant.
#[cfg(target_os = "linux")]
pub fn serve_desktop<C: FrameSource>(
    capture: &C,
    core: &vnilink::Core,
    service_id: u64,
    generation: u64,
    stop: &AtomicBool,
) -> Result<(), PublishError> {
    let info = core.hosted_desktop_info(service_id, generation)?;
    let (codec, matrix) = (info.codec, info.matrix);
    vnilink_media::serve_desktop_capture(
        capture,
        core,
        service_id,
        generation,
        stop,
        move |w, h, fps, bps| LinuxEncoder::open_codec(w, h, fps, bps, codec, matrix),
    )
}
