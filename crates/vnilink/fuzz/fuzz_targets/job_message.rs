//! The JSON job-control messages either side sends.
#![no_main]

libfuzzer_sys::fuzz_target!(|input: (u8, &[u8])| {
    vnilink::fuzzing::job_message(input.0, input.1);
});
