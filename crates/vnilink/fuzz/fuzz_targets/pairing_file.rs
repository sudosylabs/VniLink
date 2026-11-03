// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! A pairing file as anyone may send, and the time it is opened.
//!
//! Feeds the pairing file reader and the invitation inside it. A file that
//! opens carries a token that, enclosed again, opens to the same peer,
//! digits and expiry.
#![no_main]

libfuzzer_sys::fuzz_target!(|input: (u64, &[u8])| {
    vnilink::fuzzing::pairing_file(input.1, input.0);
});
