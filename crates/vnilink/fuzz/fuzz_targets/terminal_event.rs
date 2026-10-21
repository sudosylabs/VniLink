//! The events a host's terminal sends.
#![no_main]

libfuzzer_sys::fuzz_target!(|bytes: &[u8]| {
    vnilink::fuzzing::terminal_event(bytes);
});
