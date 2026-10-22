//! Windows capture and H.264 for VniLink.
//!
//! [`WindowsCapture`] uses Desktop Duplication on the interactive desktop of
//! this session. [`WindowsEncoder`] uses a Media Foundation H.264 transform,
//! hardware where one opens, and expects NV12. The transform's
//! length-prefixed samples are turned into Annex-B before they leave this
//! crate. Neither Win32 path is compiled on another operating system, and
//! neither one reaches the secure UAC desktop.
//! A build on macOS or Linux does not execute the Win32 code.

mod bitstream;
mod codecs;
#[cfg(any(windows, test))]
mod cursor;
mod decode;
mod encode;
mod host;
#[cfg(any(windows, test))]
mod idle;
mod plan;
#[cfg(windows)]
mod transform;
#[cfg(windows)]
mod transform_events;

#[cfg(windows)]
mod capture;
#[cfg(windows)]
mod gpu;
#[cfg(windows)]
pub use gpu::GpuDevice;
#[cfg(any(windows, test))]
mod shells;
#[cfg(windows)]
mod terminal;
#[cfg(any(windows, test))]
mod terminal_io;
#[cfg(windows)]
pub use terminal::{automatic_of, automatic_shell, available_shells, WindowsTerminalFactory};

pub use codecs::probe_video_codecs;
pub use decode::WindowsDecoder;
pub use encode::WindowsEncoder;
pub use plan::{WindowsPlan, SECURE_DESKTOP_SUPPORTED};

#[cfg(windows)]
pub use capture::WindowsCapture;
#[cfg(windows)]
mod snapshot;
#[cfg(windows)]
pub use host::serve_windows;
pub use host::HostSession;
#[cfg(windows)]
pub use snapshot::WindowsSnapshot;

#[cfg(test)]
#[path = "tests.rs"]
mod tests;

#[cfg(windows)]
mod power;
#[cfg(windows)]
pub use power::WindowsPowerFactory;
#[cfg(windows)]
mod system;
#[cfg(windows)]
pub use system::WindowsSystemActions;

#[cfg(windows)]
mod camera;
#[cfg(any(windows, test))]
mod camera_format;
#[cfg(windows)]
pub use camera::{camera_devices, WindowsCameraFactory};

#[cfg(windows)]
mod privacy;
