// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! A viewer's loss reports as the host decodes and smooths them.
#![no_main]

libfuzzer_sys::fuzz_target!(|reports: Vec<[u8; 20]>| {
    vnilink::fuzzing::video_reports(&reports);
});
