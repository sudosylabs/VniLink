// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

use vnilink_media::Error;

use crate::pixels::{raw_frame_from_bgra, raw_frame_from_bgrx};

#[test]
fn bgrx_rows_become_opaque_bgra() {
    let pixels = [1u8, 2, 3, 0, 4, 5, 6, 9, 7, 8, 9, 0, 0, 0, 0, 0];
    let frame = raw_frame_from_bgrx(2, 2, 8, 9, &pixels).unwrap();
    assert_eq!(frame.timestamp_us(), 9);
    assert_eq!(frame.row(0), &[1, 2, 3, 255, 4, 5, 6, 255]);
    assert!(matches!(
        raw_frame_from_bgra(2, 2, 8, 0, &[0, 1, 2, 3]),
        Err(Error::InvalidFrame("frame is missing pixels"))
    ));
}
