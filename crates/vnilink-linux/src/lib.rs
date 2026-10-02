//! Linux capture for VniLink.
//!
//! X11 and the Wayland portal are separate. [`X11Capture`] reads the current
//! X session. [`PortalSession`] is the ScreenCast permission sequence; one
//! started session is not universal unattended access. H.264 encode is VA-API
//! and is unavailable in a build that has not opened a hardware encoder.
//! Tests do not connect to an X server or a session bus.

#[cfg(target_os = "linux")]
mod dbus;
mod encode;
mod portal;
mod x11;

pub use encode::LinuxEncoder;
pub use portal::{PortalPhase, PortalSession, ONE_PORTAL_SESSION_IS_UNIVERSAL};
pub use x11::{raw_frame_from_ximage, X11Capture, X11Image, X11Plan};

#[cfg(target_os = "linux")]
pub use dbus::open_monitor;

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
