//! `FileRequest::decode` on the bytes that open a `vnilink/files/1` stream.
#![no_main]

libfuzzer_sys::fuzz_target!(|bytes: &[u8]| {
    vnilink::fuzzing::file_request(bytes);
});
