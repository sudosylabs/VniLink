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
use vnilink_media::{Error, FrameSource, FrameWait, PublishError, Quality, RawFrame};

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

/// Copy portal frames into `core` until `stop` is set or the hosted session ends.
///
/// The caller has already started `capture`. This function does not open a
/// portal. `stop` is read between frames, so a frame already being copied is
/// finished first. The encoder profile follows the negotiated session codec:
/// High when Hello/Accept agreed it, else Constrained Baseline.
#[cfg(target_os = "linux")]
pub fn serve_portal(
    capture: &crate::pipewire::PortalCapture,
    core: &vnilink::Core,
    session_id: u64,
    quality: Quality,
    stop: &AtomicBool,
) -> Result<(), PublishError> {
    let codec = core.host_codec(session_id).unwrap_or(vnilink::CODEC_H264);
    let matrix = core.host_matrix(session_id).unwrap_or_default();
    vnilink_media::serve_capture(
        capture,
        core,
        session_id,
        quality,
        stop,
        move |w, h, fps, bps| LinuxEncoder::open_codec(w, h, fps, bps, codec, matrix),
    )
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
