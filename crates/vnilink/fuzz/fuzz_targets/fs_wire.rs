// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! The `vnilink/fs` request, response, listing and places parsers.
#![no_main]

libfuzzer_sys::fuzz_target!(|input: (u8, &[u8])| {
    vnilink::fuzzing::fs_wire(input.0, input.1);
});
