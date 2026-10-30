// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

use vnilink_media::Error;

use crate::pixels::raw_frame_from_bgra;

#[test]
fn strided_rows_are_packed_without_padding() {
    let pixels = [1u8, 2, 3, 255, 4, 5, 6, 255, 0, 0];
    let frame = raw_frame_from_bgra(2, 1, 10, 9, &pixels).unwrap();
    assert_eq!(frame.timestamp_us(), 9);
    assert_eq!(frame.row(0), &[1, 2, 3, 255, 4, 5, 6, 255]);
    assert!(matches!(
        raw_frame_from_bgra(2, 2, 8, 0, &[0, 1, 2, 3]),
        Err(Error::InvalidFrame("frame is missing pixels"))
    ));
}
