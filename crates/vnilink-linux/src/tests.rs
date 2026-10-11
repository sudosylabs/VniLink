#[path = "tests/decode.rs"]
mod decode_tests;
#[path = "tests/encode.rs"]
mod encode_tests;
#[path = "tests/host.rs"]
mod host_tests;
#[path = "tests/owners.rs"]
mod owners_tests;
#[path = "tests/parameter_set.rs"]
mod parameter_set_tests;
#[cfg(target_os = "linux")]
#[path = "tests/pipewire.rs"]
mod pipewire_tests;
#[path = "tests/pixels.rs"]
mod pixels_tests;
#[path = "tests/portal.rs"]
mod portal_tests;
#[cfg(target_os = "linux")]
#[path = "tests/system.rs"]
mod system_tests;
#[cfg(target_os = "linux")]
#[path = "tests/vaapi.rs"]
mod vaapi_tests;
#[path = "tests/x11.rs"]
mod x11_tests;
