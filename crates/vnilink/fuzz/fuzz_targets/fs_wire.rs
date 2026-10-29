//! The `vnilink/fs` request, response, listing and places parsers.
#![no_main]

libfuzzer_sys::fuzz_target!(|input: (u8, &[u8])| {
    vnilink::fuzzing::fs_wire(input.0, input.1);
});
