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

/// Rects ScreenCaptureKit reports become the frame's changes without a pixel
/// read, rounded outwards. They are in the units of the frame's content rect,
/// which on a scaled display is half the picture's pixels or less: they are
/// scaled by the picture's size over the content rect's. With no content rect,
/// or no rects, the pixels say.
#[test]
fn reported_rects_are_scaled_from_the_content_rect_to_the_picture() {
    use crate::pixels::reported_changes;
    let rects = [[10.5, 20.0, 30.0, 40.25]];
    let region = |x, y, width, height| {
        Some(vec![vnilink::Region {
            x,
            y,
            width,
            height,
        }])
    };
    // A display with no scale, captured at its size.
    let unscaled = reported_changes(&rects, Some((1920.0, 1080.0)), (1920, 1080)).unwrap();
    assert_eq!(unscaled.regions, region(10, 20, 31, 41));
    // Scale 2, captured at the display's pixels: 3840 by 2160 from 1920 by 1080.
    let full = reported_changes(&rects, Some((1920.0, 1080.0)), (3840, 2160)).unwrap();
    assert_eq!(full.regions, region(21, 40, 60, 81));
    // Scale 2, delivered at 1920 by 1080: the content rect is 960 by 540.
    let delivered = reported_changes(&rects, Some((960.0, 540.0)), (1920, 1080)).unwrap();
    assert_eq!(delivered.regions, region(21, 40, 60, 81));
    assert!(reported_changes(&rects, None, (1920, 1080)).is_none());
    assert!(reported_changes(&rects, Some((0.0, 540.0)), (1920, 1080)).is_none());
    assert!(reported_changes(&[], Some((1920.0, 1080.0)), (1920, 1080)).is_none());
}
