// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! The JSON job-control messages either side sends.
//!
//! The first input byte picks the message type. A message re-encodes to JSON
//! that decodes to itself, and a manifest that passes validation can be
//! summed.
#![no_main]

libfuzzer_sys::fuzz_target!(|input: (u8, &[u8])| {
    vnilink::fuzzing::job_message(input.0, input.1);
});
