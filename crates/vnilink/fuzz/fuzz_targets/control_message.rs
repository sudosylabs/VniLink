//! `ControlMessage::decode` on any frame a peer sends on the control stream.
#![no_main]

libfuzzer_sys::fuzz_target!(|bytes: &[u8]| {
    vnilink::fuzzing::control_message(bytes);
});
