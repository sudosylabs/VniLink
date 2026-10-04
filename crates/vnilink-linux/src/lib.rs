//! Linux capture for VniLink.
//!
//! X11 and the Wayland portal are separate. [`X11Capture`] reads the current
//! X session. [`PortalSession`] is the ScreenCast permission sequence, and
//! [`PortalCapture`] reads BGRA or BGRx from the PipeWire node that sequence
//! starts, including a DMA-BUF mapped through GBM. One started session is
//! not universal unattended access. [`LinuxEncoder`] encodes H.264 with
//! VA-API on the first DRM render node that offers Constrained Baseline. Tests do not
//! connect to an X server or a session bus. The encoder test and the DMA-BUF
//! test open a render node when one is present.

#[cfg(target_os = "linux")]
mod dbus;
#[cfg(target_os = "linux")]
mod dmabuf;
mod encode;
mod host;
mod parameter_set;
#[cfg(target_os = "linux")]
mod pipewire;
mod pixels;
mod portal;
#[cfg(target_os = "linux")]
mod vaapi;
mod x11;

pub use encode::LinuxEncoder;
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
