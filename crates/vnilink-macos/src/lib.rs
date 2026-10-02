//! macOS video for VniLink.
//!
//! [`MacEncoder`] and [`MacDecoder`] use VideoToolbox on synthetic BGRA
//! frames. [`ScreenPlan`] is the one-monitor ScreenCaptureKit configuration.
//! [`ScreenCapture::start`] lists shareable content and starts a stream; tests
//! do not call it, and this crate has not captured the screen.
//!
//! VideoToolbox emits length-prefixed NALs. Keyframes are converted to Annex-B
//! with SPS and PPS in front so another operating system can decode them.

#[cfg(target_os = "macos")]
mod capture;
#[cfg(target_os = "macos")]
mod decode;
#[cfg(target_os = "macos")]
mod encode;

#[cfg(target_os = "macos")]
pub use capture::{ScreenCapture, ScreenPlan, FILEVAULT_PREBOOT_SUPPORTED};
#[cfg(target_os = "macos")]
pub use decode::MacDecoder;
#[cfg(target_os = "macos")]
pub use encode::MacEncoder;

#[cfg(all(test, target_os = "macos"))]
#[path = "tests.rs"]
mod tests;
