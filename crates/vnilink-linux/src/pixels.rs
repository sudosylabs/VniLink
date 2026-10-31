// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! Copy a PipeWire video buffer into a tightly packed BGRA picture.
//!
//! The bytes are screen content: do not log them.

use vnilink_media::{Error, RawFrame};

/// 32-bit little-endian BGRA with a row stride. Rows may include padding.
pub fn raw_frame_from_bgra(
    width: u16,
    height: u16,
    stride: usize,
    timestamp_us: u64,
    bytes: &[u8],
) -> Result<RawFrame, Error> {
    RawFrame::with_stride(width, height, stride, timestamp_us, bytes.to_vec())
}

/// BGRx is BGRA with an unused last byte. The copy stores alpha as 255.
pub fn raw_frame_from_bgrx(
    width: u16,
    height: u16,
    stride: usize,
    timestamp_us: u64,
    bytes: &[u8],
) -> Result<RawFrame, Error> {
    if width == 0 || height == 0 {
        return Err(Error::InvalidFrame(
            "frame width and height must be non-zero",
        ));
    }
    let row = usize::from(width) * 4;
    if stride < row {
        return Err(Error::InvalidFrame("frame stride is shorter than one row"));
    }
    let needed = stride.saturating_mul(usize::from(height));
    if bytes.len() < needed {
        return Err(Error::InvalidFrame("frame is missing pixels"));
    }
    let mut packed = vec![0u8; row * usize::from(height)];
    for y in 0..usize::from(height) {
        let src = &bytes[y * stride..y * stride + row];
        let dst = &mut packed[y * row..(y + 1) * row];
        dst.copy_from_slice(src);
        for pixel in dst.chunks_exact_mut(4) {
            pixel[3] = 255;
        }
    }
    RawFrame::bgra(width, height, timestamp_us, packed)
}

/// A 32-bit pixel's bytes in memory, for formats the screen cast portal may
/// offer besides BGRA and BGRx.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ByteOrder {
    /// R, G, B, A.
    Rgba,
    /// R, G, B, unused.
    Rgbx,
    /// Unused, B, G, R.
    Xbgr,
}

/// Rows of `order` pixels as tightly packed BGRA. The unused byte becomes
/// opaque alpha; RGBA keeps its own.
pub fn raw_frame_from_order(
    order: ByteOrder,
    width: u16,
    height: u16,
    stride: usize,
    timestamp_us: u64,
    bytes: &[u8],
) -> Result<RawFrame, Error> {
    if width == 0 || height == 0 {
        return Err(Error::InvalidFrame(
            "frame width and height must be non-zero",
        ));
    }
    let row = usize::from(width) * 4;
    if stride < row {
        return Err(Error::InvalidFrame("frame stride is shorter than one row"));
    }
    let needed = stride.saturating_mul(usize::from(height));
    if bytes.len() < needed {
        return Err(Error::InvalidFrame("frame is missing pixels"));
    }
    let mut packed = vec![0u8; row * usize::from(height)];
    for y in 0..usize::from(height) {
        let src = bytes[y * stride..y * stride + row].chunks_exact(4);
        let dst = packed[y * row..(y + 1) * row].chunks_exact_mut(4);
        for (src, dst) in src.zip(dst) {
            let (b, g, r, a) = match order {
                ByteOrder::Rgba => (src[2], src[1], src[0], src[3]),
                ByteOrder::Rgbx => (src[2], src[1], src[0], 255),
                ByteOrder::Xbgr => (src[1], src[2], src[3], 255),
            };
            dst.copy_from_slice(&[b, g, r, a]);
        }
    }
    RawFrame::bgra(width, height, timestamp_us, packed)
}
