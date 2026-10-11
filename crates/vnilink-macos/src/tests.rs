#[cfg(target_os = "macos")]
#[path = "tests/capture.rs"]
mod capture_tests;
#[path = "tests/encode.rs"]
mod encode_tests;
#[path = "tests/host.rs"]
mod host_tests;
#[path = "tests/pixels.rs"]
mod pixels_tests;
#[cfg(target_os = "macos")]
#[path = "tests/system.rs"]
mod system_tests;
#[cfg(target_os = "macos")]
#[path = "tests/videotoolbox.rs"]
mod videotoolbox_tests;
