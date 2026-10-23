//! Linux capture and hardware video for VniLink.
//!
//! X11 and the Wayland portal are separate. [`X11Capture`] reads the current
//! X session. [`PortalSession`] is the ScreenCast permission sequence, and
//! [`PortalCapture`] reads BGRA or BGRx from the PipeWire node that sequence
//! starts using shared-memory buffers. One started session is
//! not universal unattended access. [`LinuxEncoder`] encodes H.264 and
//! H.265 with VA-API on the first DRM render node that offers the
//! negotiated codec (for H.264, High when the session agrees it, else
//! Constrained Baseline); [`video_codec_caps`] offers H.265 only once a
//! picture encoded on the GPU decodes. [`LinuxDecoder`] decodes H.264 on
//! the first node that offers variable-length decode. Tests do not connect to an X server or a session
//! bus. The encoder test and the DMA-BUF test open a render node when one
//! is present.

#[cfg(target_os = "linux")]
mod dbus;
#[cfg(target_os = "linux")]
mod decode;
// GBM import remains an isolated hardware experiment, not live portal capture.
// The camera continuity logic (identity, hold, retirement) is platform-free:
// tests build it everywhere while V4L2 capture stays Linux-only.
#[cfg(any(target_os = "linux", test))]
mod camera;
#[cfg(all(target_os = "linux", test))]
mod dmabuf;
#[cfg(target_os = "linux")]
pub use camera::camera_devices;
#[cfg(any(target_os = "linux", test))]
pub use camera::LinuxCameraFactory;
#[cfg(target_os = "linux")]
mod codecs;
mod encode;
#[cfg(any(target_os = "linux", test))]
mod hevc_parameter_set;
mod host;
mod owners;
mod parameter_set;
#[cfg(target_os = "linux")]
mod pipewire;
mod pixels;
mod pointer;
mod portal;
mod syntax;
#[cfg(target_os = "linux")]
mod vaapi;
#[cfg(target_os = "linux")]
mod wayland;
mod x11;

#[cfg(target_os = "linux")]
pub use codecs::video_codec_caps;
#[cfg(target_os = "linux")]
pub use decode::LinuxDecoder;
pub use encode::LinuxEncoder;
/// The caps probe reports nothing where VA-API cannot exist. The Linux build
/// above replaces this with the render-node check.
#[cfg(not(target_os = "linux"))]
pub fn video_codec_caps() -> vnilink::VideoCodecCaps {
    vnilink::VideoCodecCaps::default()
}
pub use host::HostSession;
#[cfg(target_os = "linux")]
pub use host::{serve_desktop, serve_portal};
pub use owners::{
    stale_owners, topology_signature, wanted_owners, GeometryFingerprint, SourceOwner, SourceOwners,
};
pub use pixels::{raw_frame_from_bgra, raw_frame_from_bgrx};
pub use pointer::{
    display_for_frame, display_for_frame_id, display_for_picture, place_on_display, pointer,
};
pub use portal::{PortalPhase, PortalSession, ONE_PORTAL_SESSION_IS_UNIVERSAL};
pub use x11::{raw_frame_from_ximage, X11Capture, X11Image, X11Plan};
mod snapshot;
pub use snapshot::LinuxSnapshot;

#[cfg(target_os = "linux")]
pub use dbus::open_monitor;
#[cfg(target_os = "linux")]
pub use pipewire::{PortalBuffer, PortalCapture, PortalDelivery, PortalFormat};
#[cfg(target_os = "linux")]
pub use wayland::{WaylandCapture, WaylandInput};

#[cfg(test)]
#[path = "tests.rs"]
mod tests;

mod cursor;

#[cfg(target_os = "linux")]
mod power;
#[cfg(target_os = "linux")]
pub use power::LinuxPowerFactory;
#[cfg(target_os = "linux")]
mod system;
#[cfg(target_os = "linux")]
pub use system::LinuxSystemActions;

#[cfg(target_os = "linux")]
mod privacy;
