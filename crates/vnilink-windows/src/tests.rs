#[path = "tests/bitstream.rs"]
mod bitstream_tests;
#[path = "tests/decode.rs"]
mod decode_tests;
#[cfg(windows)]
#[path = "tests/encode.rs"]
mod encode_tests;
#[path = "tests/host.rs"]
mod host_tests;
#[path = "tests/outputs.rs"]
mod outputs_tests;
#[path = "tests/plan.rs"]
mod plan_tests;
#[cfg(windows)]
#[path = "tests/recovery.rs"]
mod recovery_tests;
