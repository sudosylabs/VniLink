// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! The `vnilink/fs` request, response, listing and places parsers.
//!
//! The first input byte picks the parser: `decode_request`,
//! `decode_response`, `decode_list_body` or `decode_places_body`. Whatever
//! decodes encodes and decodes again to itself; a response header encodes to
//! its own bytes.
#![no_main]

libfuzzer_sys::fuzz_target!(|input: (u8, &[u8])| {
    vnilink::fuzzing::fs_wire(input.0, input.1);
});
