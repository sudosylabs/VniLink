// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! Clipboard frames, file selections included, as a peer sends them.
#![no_main]

libfuzzer_sys::fuzz_target!(|payload: &[u8]| {
    vnilink::fuzzing::clipboard_frame(payload);
});
