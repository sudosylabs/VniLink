//! Windows capture and H.264 for VniLink.
//!
//! [`WindowsCapture`] uses Desktop Duplication on the interactive desktop of
//! this session. [`WindowsEncoder`] uses the Media Foundation H.264 transform
//! and expects NV12. The transform's length-prefixed samples are turned into
//! Annex-B before they leave this crate. Neither Win32 path is compiled on
//! another operating system, and neither one reaches the secure UAC desktop.
//! A build on macOS or Linux does not execute the Win32 code.

mod bitstream;
mod decode;
mod encode;
mod host;
mod plan;

#[cfg(windows)]
mod capture;

pub use decode::WindowsDecoder;
pub use encode::WindowsEncoder;
pub use plan::{WindowsPlan, SECURE_DESKTOP_SUPPORTED};

#[cfg(windows)]
pub use capture::WindowsCapture;
#[cfg(windows)]
pub use host::serve_windows;
pub use host::HostSession;

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
