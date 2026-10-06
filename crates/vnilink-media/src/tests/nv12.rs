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
    let nv12 = bgra_to_nv12(&black).unwrap();
    assert!(nv12.y_plane().iter().all(|sample| *sample == 16));
    assert!(nv12.uv_plane().iter().all(|sample| *sample == 128));
    let white = solid(2, 2, [255, 255, 255, 255]);
    let nv12 = bgra_to_nv12(&white).unwrap();
    assert!(nv12.y_plane().iter().all(|sample| *sample == 235));
    assert_eq!(nv12.uv_plane().len(), 2);
    let rendered = format!("{nv12:?}");
    assert!(!rendered.contains("235"));
    assert!(matches!(
        bgra_to_nv12(&solid(3, 2, [0, 0, 0, 255])),
        Err(Error::InvalidFrame("NV12 width and height must be even"))
    ));
}

#[test]
fn nv12_returns_within_rounding_of_the_original_picture() {
    for pixel in [
        [255, 0, 0, 255],
        [0, 255, 0, 255],
        [0, 0, 255, 255],
        [13, 77, 200, 255],
    ] {
        let frame = solid(4, 4, pixel);
        let nv12 = bgra_to_nv12(&frame).unwrap();
        let back = nv12_to_bgra(4, 4, 10, nv12.y_plane(), nv12.uv_plane()).unwrap();
        assert_eq!((back.width(), back.height()), (4, 4));
        for y in 0..4 {
            let row = back.row(y);
            for chunk in row.chunks_exact(4) {
                for channel in 0..3 {
                    let drift = (i16::from(chunk[channel]) - i16::from(pixel[channel])).abs();
                    assert!(drift <= 2, "channel drifted by {drift}");
                }
                assert_eq!(chunk[3], 255);
            }
        }
    }
    assert!(matches!(
        nv12_to_bgra(2, 2, 0, &[0; 3], &[0; 2]),
        Err(Error::InvalidFrame("frame is missing pixels"))
    ));
    assert!(matches!(
        nv12_to_bgra(3, 2, 0, &[0; 12], &[0; 6]),
        Err(Error::InvalidFrame("NV12 width and height must be even"))
    ));
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
        let actual = bgra_to_nv12(&frame).unwrap();
        let reference = |x: usize, y: usize| {
            let pixel = &frame.row(y as u16)[x * 4..][..4];
            let (r, g, b) = (
                i32::from(pixel[2]),
                i32::from(pixel[1]),
                i32::from(pixel[0]),
            );
            [
                (((66 * r + 129 * g + 25 * b + 128) >> 8) + 16).clamp(0, 255),
                (((-38 * r - 74 * g + 112 * b + 128) >> 8) + 128).clamp(0, 255),
                (((112 * r - 94 * g - 18 * b + 128) >> 8) + 128).clamp(0, 255),
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
fn nv12_decode_matches_scalar_for_every_luma_and_varied_chroma() {
    // Include out-of-range samples and both rows of every chroma block.
    for (width, height) in [(2usize, 2usize), (6, 4), (256, 18)] {
        let y: Vec<u8> = (0..width * height).map(|i| i as u8).collect();
        let uv: Vec<u8> = (0..width * height / 2).map(|i| (i * 31) as u8).collect();
        let frame = nv12_to_bgra(width as u16, height as u16, 123, &y, &uv).unwrap();
        assert_eq!(frame.timestamp_us(), 123);
        for row in 0..height {
            for col in 0..width {
                let c = i32::from(y[row * width + col]) - 16;
                let index = row / 2 * width + col / 2 * 2;
                let d = i32::from(uv[index]) - 128;
                let e = i32::from(uv[index + 1]) - 128;
                let expected = [
                    ((298 * c + 516 * d + 128) >> 8).clamp(0, 255) as u8,
                    ((298 * c - 100 * d - 208 * e + 128) >> 8).clamp(0, 255) as u8,
                    ((298 * c + 409 * e + 128) >> 8).clamp(0, 255) as u8,
                    255,
                ];
                assert_eq!(&frame.row(row as u16)[col * 4..col * 4 + 4], &expected);
            }
        }
    }
}
