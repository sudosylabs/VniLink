// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! `ControlMessage::decode` on any frame a peer sends on the control stream.
#![no_main]

libfuzzer_sys::fuzz_target!(|bytes: &[u8]| {
    vnilink::fuzzing::control_message(bytes);
});
