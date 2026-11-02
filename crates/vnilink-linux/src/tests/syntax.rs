// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

use crate::syntax::rbsp;

#[test]
fn an_emulation_prevention_byte_after_two_zeros_is_removed() {
    assert_eq!(rbsp(&[0x65, 0, 0, 3, 1]), [0x65, 0, 0, 1]);
}

#[test]
fn an_emulation_prevention_byte_after_a_longer_run_of_zeros_is_removed() {
    // The rule is the two bytes before the 03, however many zeros come first.
    assert_eq!(rbsp(&[0, 0, 0, 3, 1]), [0, 0, 0, 1]);
}

#[test]
fn a_three_not_after_two_zeros_is_kept() {
    assert_eq!(rbsp(&[0, 3, 0, 1, 3]), [0, 3, 0, 1, 3]);
}

#[test]
fn a_run_of_zeros_longer_than_a_byte_counts_is_read_whole() {
    // VA-API encoders end some frames with hundreds of zero bytes.
    let mut nal = vec![0u8; 300];
    nal.extend([3, 1]);
    let mut read = vec![0u8; 300];
    read.push(1);
    assert_eq!(rbsp(&nal), read);
}
