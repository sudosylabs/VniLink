//! Cursor shapes and positions as a host sends them.
#![no_main]

libfuzzer_sys::fuzz_target!(|bytes: &[u8]| {
    vnilink::fuzzing::cursor_state(bytes);
});
