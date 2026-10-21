//! A viewer's loss reports as the host decodes and smooths them.
#![no_main]

libfuzzer_sys::fuzz_target!(|reports: Vec<[u8; 20]>| {
    vnilink::fuzzing::video_reports(&reports);
});
