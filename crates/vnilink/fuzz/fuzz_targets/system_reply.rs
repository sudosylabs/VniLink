// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! The System service's operation, support and privacy replies.
#![no_main]

libfuzzer_sys::fuzz_target!(|input: (u8, &[u8])| {
    vnilink::fuzzing::system_reply(input.0, input.1);
});
