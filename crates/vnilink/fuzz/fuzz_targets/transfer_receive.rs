// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! The receiver's stream parser on a sender's stream: an honest one for a
//! small file, then edited, and cut into arbitrary chunks.
#![no_main]

use arbitrary::Arbitrary;

#[derive(Debug, Arbitrary)]
struct Input {
    file: Vec<u8>,
    /// Segments of 1 KiB to 8 KiB, so a small file has several.
    segment_shift: u8,
    edits: Vec<Edit>,
    chunks: Vec<u16>,
}

#[derive(Debug, Arbitrary)]
enum Edit {
    Flip { at: u16, bit: u8 },
    Cut { at: u16 },
    Insert { at: u16, bytes: Vec<u8> },
    Append { bytes: Vec<u8> },
}

libfuzzer_sys::fuzz_target!(|input: Input| {
    let segment = 1024u64 << (input.segment_shift % 4);
    let mut stream = vnilink::fuzzing::honest_stream(&input.file, segment);
    for edit in input.edits {
        let at = |at: u16| usize::from(at) % (stream.len() + 1);
        match edit {
            Edit::Flip { at: index, bit } => {
                if !stream.is_empty() {
                    let index = usize::from(index) % stream.len();
                    stream[index] ^= 1 << (bit % 8);
                }
            }
            Edit::Cut { at: index } => stream.truncate(at(index)),
            Edit::Insert { at: index, bytes } => {
                let index = at(index);
                stream.splice(index..index, bytes);
            }
            Edit::Append { bytes } => stream.extend(bytes),
        }
    }
    let chunks: Vec<usize> = input.chunks.into_iter().map(usize::from).collect();
    vnilink::fuzzing::receive_stream(&input.file, segment, &stream, &chunks);
});
