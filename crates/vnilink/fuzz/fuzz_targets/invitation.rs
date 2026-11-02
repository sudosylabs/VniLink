// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! An invitation token as anyone may hand over, and the time it is opened.
//!
//! Feeds the invitation parser and its checks. An invitation that opens has
//! not expired and does not outlast the longest lifetime allowed.
#![no_main]

libfuzzer_sys::fuzz_target!(|input: (u64, &str)| {
    vnilink::fuzzing::invitation(input.1, input.0);
});
