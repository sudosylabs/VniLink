// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

use crate::colour::{VideoSignal, YuvMatrix};
use crate::error::Error;
use crate::frame::RawFrame;
use crate::nv12::{bgra_to_nv12, nv12_to_bgra};

fn solid(width: u16, height: u16, pixel: [u8; 4]) -> RawFrame {
    let mut bgra = Vec::with_capacity(usize::from(width) * usize::from(height) * 4);
    for _ in 0..usize::from(width) * usize::from(height) {
        bgra.extend_from_slice(&pixel);
    }
    RawFrame::bgra(width, height, 10, bgra).unwrap()
}

#[test]
fn nv12_uses_limited_range_and_rejects_odd_sizes() {
    let black = solid(2, 2, [0, 0, 0, 255]);
    let nv12 = bgra_to_nv12(&black, YuvMatrix::Bt709).unwrap();
    assert!(nv12.y_plane().iter().all(|sample| *sample == 16));
    assert!(nv12.uv_plane().iter().all(|sample| *sample == 128));
    let white = solid(2, 2, [255, 255, 255, 255]);
    let nv12 = bgra_to_nv12(&white, YuvMatrix::Bt709).unwrap();
    assert!(nv12.y_plane().iter().all(|sample| *sample == 235));
    assert_eq!(nv12.uv_plane().len(), 2);
    let rendered = format!("{nv12:?}");
    assert!(!rendered.contains("235"));
    // BT.709: red is Y' 63, Cb 102, Cr 240; grey has no chroma.
    let red = bgra_to_nv12(&solid(2, 2, [0, 0, 255, 255]), YuvMatrix::Bt709).unwrap();
    assert!(red.y_plane().iter().all(|sample| *sample == 63));
    assert_eq!(red.uv_plane(), [102, 240]);
    let grey = bgra_to_nv12(&solid(2, 2, [90, 90, 90, 255]), YuvMatrix::Bt709).unwrap();
    assert_eq!(grey.uv_plane(), [128, 128]);
    assert!(matches!(
        bgra_to_nv12(&solid(3, 2, [0, 0, 0, 255]), YuvMatrix::Bt709),
        Err(Error::InvalidFrame("NV12 width and height must be even"))
    ));
}

#[test]
fn nv12_returns_within_rounding_of_the_original_picture() {
    for (matrix, pixel) in [YuvMatrix::Bt709, YuvMatrix::Bt601]
        .into_iter()
        .flat_map(|matrix| {
            [
                [255, 0, 0, 255],
                [0, 255, 0, 255],
                [0, 0, 255, 255],
                [13, 77, 200, 255],
            ]
            .map(move |pixel| (matrix, pixel))
        })
    {
        let frame = solid(4, 4, pixel);
        let nv12 = bgra_to_nv12(&frame, matrix).unwrap();
        let back = nv12_to_bgra(
            4,
            4,
            10,
            nv12.y_plane(),
            nv12.uv_plane(),
            VideoSignal::stream(matrix),
        )
        .unwrap();
        assert_eq!((back.width(), back.height()), (4, 4));
        for y in 0..4 {
            for chunk in back.row(y).chunks_exact(4) {
                for channel in 0..3 {
                    let drift = (i16::from(chunk[channel]) - i16::from(pixel[channel])).abs();
                    assert!(drift <= 2, "{matrix:?} channel drifted by {drift}");
                }
                assert_eq!(chunk[3], 255);
            }
        }
    }
    let bt709 = VideoSignal::stream(YuvMatrix::Bt709);
    assert!(matches!(
        nv12_to_bgra(2, 2, 0, &[0; 3], &[0; 2], bt709),
        Err(Error::InvalidFrame("frame is missing pixels"))
    ));
    assert!(matches!(
        nv12_to_bgra(3, 2, 0, &[0; 12], &[0; 6], bt709),
        Err(Error::InvalidFrame("NV12 width and height must be even"))
    ));
}

#[test]
fn nv12_in_one_matrix_read_as_the_other_is_visibly_wrong() {
    let frame = solid(4, 4, [40, 120, 200, 255]);
    let nv12 = bgra_to_nv12(&frame, YuvMatrix::Bt601).unwrap();
    let wrong = nv12_to_bgra(
        4,
        4,
        0,
        nv12.y_plane(),
        nv12.uv_plane(),
        VideoSignal::stream(YuvMatrix::Bt709),
    )
    .unwrap();
    assert!(wrong.row(0)[..3]
        .iter()
        .zip(&frame.row(0)[..3])
        .any(|(a, b)| a.abs_diff(*b) > 6));
}

#[test]
fn nv12_matches_scalar_conversion_for_varied_pixels_and_row_padding() {
    // The reference computes each pixel independently, then averages rounded
    // chroma samples. Optimizations must preserve both that rounding and stride.
    for (width, height, padding) in [(2, 2, 0), (6, 4, 12), (32, 18, 20)] {
        let stride = width * 4 + padding;
        let mut pixels = vec![0xa5; stride * height];
        let mut seed = 0x1234_5678u32;
        for row in 0..height {
            for channel in &mut pixels[row * stride..row * stride + width * 4] {
                seed ^= seed << 13;
                seed ^= seed >> 17;
                seed ^= seed << 5;
                *channel = seed as u8;
            }
        }
        let frame = RawFrame::with_stride(width as u16, height as u16, stride, 0, pixels).unwrap();
        let actual = bgra_to_nv12(&frame, YuvMatrix::Bt709).unwrap();
        let reference = |x: usize, y: usize| {
            let pixel = &frame.row(y as u16)[x * 4..][..4];
            let (r, g, b) = (
                i32::from(pixel[2]),
                i32::from(pixel[1]),
                i32::from(pixel[0]),
            );
            [
                (((47 * r + 157 * g + 16 * b + 128) >> 8) + 16).clamp(0, 255),
                (((-26 * r - 86 * g + 112 * b + 128) >> 8) + 128).clamp(0, 255),
                (((112 * r - 102 * g - 10 * b + 128) >> 8) + 128).clamp(0, 255),
            ]
        };
        for row in 0..height {
            for col in 0..width {
                assert_eq!(
                    actual.y_plane()[row * width + col],
                    reference(col, row)[0] as u8
                );
                if row % 2 == 0 && col % 2 == 0 {
                    for channel in 1..=2 {
                        let sum: i32 = [(0, 0), (0, 1), (1, 0), (1, 1)]
                            .iter()
                            .map(|(dy, dx)| reference(col + dx, row + dy)[channel])
                            .sum();
                        assert_eq!(
                            actual.uv_plane()[row / 2 * width + col + channel - 1],
                            (sum / 4) as u8
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn nv12_decode_follows_the_signal_for_every_luma_and_varied_chroma() {
    // (signal, luma scale, luma offset, Cr->R, Cb->G, Cr->G, Cb->B), x256.
    let signal = |matrix, full_range| VideoSignal {
        primaries: matrix,
        transfer: matrix,
        matrix,
        full_range,
    };
    let cases = [
        (signal(1, false), 298, 16, 459, -55, -136, 541),
        (signal(1, true), 256, 0, 403, -48, -120, 475),
        (signal(6, false), 298, 16, 409, -100, -208, 516),
        (signal(5, true), 256, 0, 359, -88, -183, 454),
        // Unspecified is shown as BT.709.
        (signal(2, false), 298, 16, 459, -55, -136, 541),
    ];
    // Include out-of-range samples and both rows of every chroma block.
    for (width, height) in [(2usize, 2usize), (6, 4), (256, 18)] {
        let y: Vec<u8> = (0..width * height).map(|i| i as u8).collect();
        let uv: Vec<u8> = (0..width * height / 2).map(|i| (i * 31) as u8).collect();
        for (signal, luma, offset, cr_red, cb_green, cr_green, cb_blue) in cases {
            let frame = nv12_to_bgra(width as u16, height as u16, 123, &y, &uv, signal).unwrap();
            assert_eq!(frame.timestamp_us(), 123);
            for row in 0..height {
                for col in 0..width {
                    let c = luma * (i32::from(y[row * width + col]) - offset);
                    let index = row / 2 * width + col / 2 * 2;
                    let d = i32::from(uv[index]) - 128;
                    let e = i32::from(uv[index + 1]) - 128;
                    let expected = [
                        ((c + cb_blue * d + 128) >> 8).clamp(0, 255) as u8,
                        ((c + cb_green * d + cr_green * e + 128) >> 8).clamp(0, 255) as u8,
                        ((c + cr_red * e + 128) >> 8).clamp(0, 255) as u8,
                        255,
                    ];
                    assert_eq!(
                        &frame.row(row as u16)[col * 4..col * 4 + 4],
                        &expected,
                        "{signal:?}"
                    );
                }
            }
        }
    }
}
