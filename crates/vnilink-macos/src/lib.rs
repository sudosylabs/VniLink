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

mod capture;
#[cfg(target_os = "macos")]
mod decode;
mod encode;
mod host;
mod pixels;

#[cfg(target_os = "macos")]
pub use capture::ScreenCapture;
pub use capture::{ScreenPlan, FILEVAULT_PREBOOT_SUPPORTED};
#[cfg(target_os = "macos")]
pub use decode::MacDecoder;
pub use encode::MacEncoder;
#[cfg(target_os = "macos")]
pub use host::serve_stream;
pub use host::HostSession;
pub use pixels::raw_frame_from_bgra;
#[cfg(target_os = "macos")]
pub use pixels::raw_frame_from_pixel_buffer;

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
