// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! The latency strip read back at other scales, softened and beside a copy.

use crate::measure::timecode::{age, cells, find, CELLS, VALUE_BITS};
use crate::RawFrame;

/// A white picture with each (value, left, top, cell width) strip drawn at a
/// fractional cell width, the way a scaled stream shows it.
fn picture(width: u16, height: u16, strips: &[(u32, f32, usize, f32)]) -> RawFrame {
    let (w, h) = (usize::from(width), usize::from(height));
    let mut pixels = vec![255u8; w * h * 4];
    for &(value, left, top, cell) in strips {
        let strip = cells(value);
        let rows = (cell * 1.5).ceil() as usize;
        for y in top..(top + rows).min(h) {
            for x in 0..w {
                let at = (x as f32 + 0.5 - left) / cell;
                if at >= 0.0 && (at as usize) < CELLS && strip[at as usize] {
                    pixels[(y * w + x) * 4..(y * w + x) * 4 + 3].fill(0);
                }
            }
        }
    }
    RawFrame::bgra(width, height, 0, pixels).unwrap()
}

/// A 3-tap horizontal blur, as a decoder's chroma and quantisation smear edges.
fn soften(frame: &RawFrame) -> RawFrame {
    let w = usize::from(frame.width());
    let mut out = frame.pixels().to_vec();
    for y in 0..usize::from(frame.height()) {
        let row = frame.row(y as u16);
        for x in 1..w - 1 {
            for c in 0..3 {
                let sum = u16::from(row[(x - 1) * 4 + c])
                    + 2 * u16::from(row[x * 4 + c])
                    + u16::from(row[(x + 1) * 4 + c]);
                out[(y * w + x) * 4 + c] = (sum / 4) as u8;
            }
        }
    }
    RawFrame::bgra(frame.width(), frame.height(), 0, out).unwrap()
}

#[test]
fn a_strip_reads_back_at_its_own_and_a_scaled_size() {
    let frame = picture(
        1600,
        200,
        &[(0xA_5C3E, 40.0, 10, 30.0), (0x0_0F0F, 300.0, 120, 17.37)],
    );
    let mut found = find(&soften(&frame), 3);
    found.sort_by_key(|code| code.y);
    assert_eq!(
        found.iter().map(|code| code.value).collect::<Vec<_>>(),
        vec![0xA_5C3E, 0x0_0F0F]
    );
    assert!((found[0].cell - 30.0).abs() < 0.5, "{found:?}");
    assert!((found[1].cell - 17.37).abs() < 0.5, "{found:?}");
    assert!((found[0].x as f32 - 40.0).abs() <= 1.0);
}

#[test]
fn a_damaged_strip_is_not_read_as_another_value() {
    let mut pixels = picture(1600, 60, &[(0x1_2345, 40.0, 10, 30.0)])
        .pixels()
        .to_vec();
    // Paint one data cell's twin: the bit and its inverse now agree.
    let twin = 40 + 30 * (24 + 3) + 15;
    for y in 10..55 {
        pixels[(y * 1600 + twin) * 4..(y * 1600 + twin) * 4 + 3].fill(0);
    }
    let frame = RawFrame::bgra(1600, 60, 0, pixels).unwrap();
    assert!(find(&frame, 1).is_empty());
}

#[test]
fn a_blank_or_tiny_picture_has_no_strip() {
    assert!(find(&picture(400, 40, &[]), 1).is_empty());
    // Cells under two pixels are not read.
    assert!(find(&picture(400, 40, &[(5, 10.0, 5, 1.5)]), 1).is_empty());
}

#[test]
fn age_counts_back_across_the_wrap() {
    let top = (1 << VALUE_BITS) - 1;
    assert_eq!(age(10, 4), 6);
    assert_eq!(age(3, top), 4);
    assert_eq!(cells(top + 1), cells(0));
}

/// The same strip `test/tooling/latency.test.mjs` expects from the stimulus.
#[test]
fn the_stimulus_and_the_reader_share_one_layout() {
    let strip: String = cells(0xA_5C3E)
        .iter()
        .map(|&dark| if dark { '1' } else { '0' })
        .collect();
    assert_eq!(strip, "1010101001011100001111100101101000111100000111");
}
