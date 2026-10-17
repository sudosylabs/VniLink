//! macOS video for VniLink.
//!
//! [`MacEncoder`] and [`MacDecoder`] use VideoToolbox on synthetic BGRA
//! frames. [`ScreenPlan`] is the one-monitor ScreenCaptureKit configuration.
//! [`ScreenCapture::start`] lists shareable content and starts a stream; tests
//! do not call it, and this crate has not captured the screen.
//! [`HostSession`] paces captured pictures and publishes Annex-B for the
//! hosted session. [`serve_stream`] copies an open capture until the session
//! ends.
//!
//! VideoToolbox emits length-prefixed NALs. Keyframes are converted to Annex-B
//! with SPS and PPS in front so another operating system can decode them.

#[cfg(target_os = "macos")]
mod camera;
#[cfg(target_os = "macos")]
pub use camera::{camera_devices, microphone_authorized, MacCameraFactory};
mod capture;
#[cfg(target_os = "macos")]
mod decode;
mod encode;
mod host;
mod pixels;

pub use capture::{display_scale, ScreenPlan, FILEVAULT_PREBOOT_SUPPORTED};
#[cfg(target_os = "macos")]
pub use capture::{list_displays, ScreenCapture};
#[cfg(target_os = "macos")]
mod snapshot;
#[cfg(target_os = "macos")]
pub use decode::MacDecoder;
#[cfg(target_os = "macos")]
pub use encode::probe_hevc;
pub use encode::{MacEncoder, MacVideoEncoder};
pub use host::HostSession;
#[cfg(target_os = "macos")]
pub use host::{serve_desktop, serve_shared, serve_stream};
pub use pixels::raw_frame_from_bgra;
#[cfg(target_os = "macos")]
pub use pixels::raw_frame_from_pixel_buffer;
#[cfg(target_os = "macos")]
pub use snapshot::MacSnapshot;

#[cfg(test)]
#[path = "tests.rs"]
mod tests;

#[cfg(target_os = "macos")]
mod power;
#[cfg(target_os = "macos")]
pub use power::MacPowerFactory;
#[cfg(target_os = "macos")]
mod privacy;
#[cfg(target_os = "macos")]
mod system;
#[cfg(target_os = "macos")]
pub use system::MacSystemActions;

#[cfg(target_os = "macos")]
mod audio;
#[cfg(target_os = "macos")]
pub use audio::{system_audio_devices, MacSystemAudio};
