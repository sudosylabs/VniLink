//! Clipboard frames, file selections included, as a peer sends them.
#![no_main]

libfuzzer_sys::fuzz_target!(|payload: &[u8]| {
    vnilink::fuzzing::clipboard_frame(payload);
});
