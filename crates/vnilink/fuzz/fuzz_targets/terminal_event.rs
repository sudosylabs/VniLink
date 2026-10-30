// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! The events a host's terminal sends.
//!
//! Feeds the terminal stream's event decoder. An event decodes only from the
//! bytes it encodes to.
#![no_main]

libfuzzer_sys::fuzz_target!(|bytes: &[u8]| {
    vnilink::fuzzing::terminal_event(bytes);
});
