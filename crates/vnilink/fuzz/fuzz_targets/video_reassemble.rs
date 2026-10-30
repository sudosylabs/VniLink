// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! The viewer's reassembler on a sequence of datagrams: forged ones mixed
//! with honest fragments of frames, some lost and out of order; and, for
//! each honest frame alone, whether enough shards rebuild it.
//!
//! Forged and honest fragments never panic; an honest frame comes back
//! exactly when at least its data count of shards arrived. Whatever
//! completes goes on to the video body parser, `decode_video_body`.
#![no_main]

use arbitrary::Arbitrary;

#[derive(Debug, Arbitrary)]
enum Datagrams {
    Forged(Vec<u8>),
    Frame {
        body: Vec<u8>,
        seq: u32,
        max_datagram: u16,
        /// Parity share in 255ths of the most a frame carries.
        protection: u8,
        kept: Vec<bool>,
        order: Vec<u16>,
    },
}

libfuzzer_sys::fuzz_target!(|input: Vec<Datagrams>| {
    let mut datagrams = Vec::new();
    for item in input {
        match item {
            Datagrams::Forged(bytes) => datagrams.push(bytes),
            Datagrams::Frame {
                body,
                seq,
                max_datagram,
                protection,
                kept,
                order,
            } => {
                let max_datagram = usize::from(max_datagram);
                let protection = f64::from(protection) / 255.0 * vnilink::fuzzing::MAX_PROTECTION;
                vnilink::fuzzing::rebuild(&body, seq, max_datagram, protection, &kept, &order);
                let fragments = vnilink::fuzzing::fragments(&body, seq, max_datagram, protection);
                datagrams.extend(
                    fragments
                        .into_iter()
                        .enumerate()
                        .filter(|(index, _)| kept.get(*index).copied().unwrap_or(true))
                        .map(|(_, fragment)| fragment),
                );
            }
        }
    }
    vnilink::fuzzing::reassemble(datagrams.iter().map(Vec::as_slice));
});
