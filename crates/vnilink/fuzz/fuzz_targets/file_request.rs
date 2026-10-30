// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! `FileRequest::decode` on the bytes that open a `vnilink/files/1` stream.
#![no_main]

libfuzzer_sys::fuzz_target!(|bytes: &[u8]| {
    vnilink::fuzzing::file_request(bytes);
});
