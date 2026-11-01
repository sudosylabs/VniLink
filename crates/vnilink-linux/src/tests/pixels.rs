// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

use vnilink_media::Error;

use crate::pixels::{raw_frame_from_bgra, raw_frame_from_bgrx, raw_frame_from_order, ByteOrder};

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

#[test]
fn other_byte_orders_become_bgra() {
    let rgba = [1u8, 2, 3, 4, 5, 6, 7, 8];
    let frame = raw_frame_from_order(ByteOrder::Rgba, 2, 1, 8, 0, &rgba).unwrap();
    assert_eq!(
        frame.row(0),
        &[3, 2, 1, 4, 7, 6, 5, 8],
        "RGBA keeps its alpha"
    );
    let frame = raw_frame_from_order(ByteOrder::Rgbx, 2, 1, 8, 0, &rgba).unwrap();
    assert_eq!(
        frame.row(0),
        &[3, 2, 1, 255, 7, 6, 5, 255],
        "RGBx is opaque"
    );
    let frame = raw_frame_from_order(ByteOrder::Xbgr, 2, 1, 8, 0, &rgba).unwrap();
    assert_eq!(
        frame.row(0),
        &[2, 3, 4, 255, 6, 7, 8, 255],
        "xBGR is opaque"
    );
    assert!(matches!(
        raw_frame_from_order(ByteOrder::Rgba, 2, 1, 4, 0, &rgba),
        Err(Error::InvalidFrame("frame stride is shorter than one row"))
    ));
}
