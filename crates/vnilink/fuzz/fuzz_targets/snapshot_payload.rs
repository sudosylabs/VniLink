//! The reply a host writes for one screen snapshot.
#![no_main]

libfuzzer_sys::fuzz_target!(|bytes: &[u8]| {
    vnilink::fuzzing::snapshot_payload(bytes);
});
