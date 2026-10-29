//! The System service's operation, support and privacy replies.
#![no_main]

libfuzzer_sys::fuzz_target!(|input: (u8, &[u8])| {
    vnilink::fuzzing::system_reply(input.0, input.1);
});
