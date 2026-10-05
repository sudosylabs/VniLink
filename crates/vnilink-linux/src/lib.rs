//! Linux capture and H.264 for VniLink.
//!
//! X11 and the Wayland portal are separate. [`X11Capture`] reads the current
//! X session. [`PortalSession`] is the ScreenCast permission sequence, and
//! [`PortalCapture`] reads BGRA or BGRx from the PipeWire node that sequence
//! starts, including a DMA-BUF mapped through GBM. One started session is
//! not universal unattended access. [`LinuxEncoder`] encodes H.264 with
//! VA-API on the first DRM render node that offers Constrained Baseline,
//! and [`LinuxDecoder`] decodes it back on the first node that offers
//! variable-length decode. Tests do not connect to an X server or a session
//! bus. The encoder test and the DMA-BUF test open a render node when one
//! is present.

#[cfg(target_os = "linux")]
mod dbus;
#[cfg(target_os = "linux")]
mod decode;
#[cfg(target_os = "linux")]
mod dmabuf;
mod encode;
mod host;
mod parameter_set;
#[cfg(target_os = "linux")]
mod pipewire;
mod pixels;
mod portal;
mod syntax;
#[cfg(target_os = "linux")]
mod vaapi;
mod x11;

#[cfg(target_os = "linux")]
pub use decode::LinuxDecoder;
pub use encode::LinuxEncoder;
#[cfg(target_os = "linux")]
pub use vaapi::{h264_decode_available, h264_encode_available};
/// Availability probes report false where VA-API cannot exist. The Linux build
/// above replaces these with the render-node check.
#[cfg(not(target_os = "linux"))]
pub fn h264_encode_available() -> bool {
    false
}
/// See [`h264_encode_available`].
#[cfg(not(target_os = "linux"))]
pub fn h264_decode_available() -> bool {
    false
}
#[cfg(target_os = "linux")]
pub use host::serve_portal;
pub use host::HostSession;
pub use pixels::{raw_frame_from_bgra, raw_frame_from_bgrx};
pub use portal::{PortalPhase, PortalSession, ONE_PORTAL_SESSION_IS_UNIVERSAL};
pub use x11::{raw_frame_from_ximage, X11Capture, X11Image, X11Plan};

#[cfg(target_os = "linux")]
pub use dbus::open_monitor;
#[cfg(target_os = "linux")]
pub use pipewire::{PortalBuffer, PortalCapture, PortalDelivery, PortalFormat};

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
