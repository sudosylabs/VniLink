// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! A frame on a side stream, as any peer can send one on a service's pipe,
//! a Desktop's clipboard pipe or a file request.
//!
//! Feeds the shared length-prefixed reader. A whole frame within its limit
//! reads back exactly; a longer one, or one cut short, is refused.
#![no_main]

libfuzzer_sys::fuzz_target!(|input: (u16, &[u8])| {
    let (limit, bytes) = input;
    vnilink::fuzzing::sidestream_frame(limit, bytes);
});
