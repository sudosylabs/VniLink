// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! One BGRA picture. The bytes are screen content: do not log them.

use std::fmt;
use std::time::Instant;

use crate::error::Error;
use crate::pixels::{NativePicture, Pixels};

/// Tightly packed or strided BGRA8, origin at the top left.
///
/// A frame never changes once made, so its clones share its pixels: a 4K
/// picture is 33 MB, and the capture channel, the shared capture's
/// consumers and an idle source's repeats all hold the same one. The
/// pixels may still be the platform's ([`RawFrame::native`]), for an
/// encoder that reads them there.
pub struct RawFrame {
    width: u16,
    height: u16,
    stride: usize,
    timestamp_us: u64,
    captured_at: Option<Instant>,
    changed: Option<Changes>,
    bgra: Pixels,
}

impl RawFrame {
    /// A frame whose rows are packed `width * 4` bytes apart.
    pub fn bgra(width: u16, height: u16, timestamp_us: u64, bgra: Vec<u8>) -> Result<Self, Error> {
        let stride = usize::from(width).saturating_mul(4);
        Self::with_stride(width, height, stride, timestamp_us, bgra)
    }

    pub fn with_stride(
        width: u16,
        height: u16,
        stride: usize,
        timestamp_us: u64,
        bgra: Vec<u8>,
    ) -> Result<Self, Error> {
        Self::shared(width, height, stride, timestamp_us, Pixels::from(bgra))
    }

    /// A packed frame over a picture the platform holds. Its size is the
    /// picture's; reading its pixels copies them out once.
    pub fn native(timestamp_us: u64, picture: Box<dyn NativePicture>) -> Result<Self, Error> {
        let (width, height) = picture.size();
        Self::shared(
            width,
            height,
            usize::from(width) * 4,
            timestamp_us,
            Pixels::native(picture),
        )
    }

    /// A frame over pixels another holder keeps too, without copying them.
    pub fn shared(
        width: u16,
        height: u16,
        stride: usize,
        timestamp_us: u64,
        bgra: Pixels,
    ) -> Result<Self, Error> {
        if width == 0 || height == 0 {
            return Err(Error::InvalidFrame(
                "frame width and height must be non-zero",
            ));
        }
        let row = usize::from(width).saturating_mul(4);
        if stride < row {
            return Err(Error::InvalidFrame("frame stride is shorter than one row"));
        }
        match bgra.native_picture() {
            // Its pixels are read back packed; checking their length would read them.
            Some(picture) => {
                if picture.size() != (width, height) || stride != row {
                    return Err(Error::InvalidFrame("frame does not match its picture"));
                }
            }
            None => {
                if bgra.len() < stride.saturating_mul(usize::from(height)) {
                    return Err(Error::InvalidFrame("frame is missing pixels"));
                }
            }
        }
        Ok(Self {
            width,
            height,
            stride,
            timestamp_us,
            captured_at: None,
            changed: None,
            bgra,
        })
    }

    pub fn width(&self) -> u16 {
        self.width
    }

    pub fn height(&self) -> u16 {
        self.height
    }

    pub fn stride(&self) -> usize {
        self.stride
    }

    pub fn timestamp_us(&self) -> u64 {
        self.timestamp_us
    }

    /// Retimestamp an owned cached picture without copying its pixels again.
    pub fn with_timestamp(mut self, timestamp_us: u64) -> Self {
        self.timestamp_us = timestamp_us;
        self
    }

    /// When the OS captured these pixels. `None` means the source cannot say,
    /// and the time it was read stands in.
    pub fn captured_at(&self) -> Option<Instant> {
        self.captured_at
    }

    pub fn with_captured_at(mut self, captured_at: Option<Instant>) -> Self {
        self.captured_at = captured_at;
        self
    }

    /// Where its source says the picture changed since the one it
    /// delivered before; `None` when the source cannot say, which counts as
    /// all of it.
    pub fn changed(&self) -> Option<&Changes> {
        self.changed.as_ref()
    }

    pub fn with_changed(mut self, changed: Option<Changes>) -> Self {
        self.changed = changed;
        self
    }

    /// The pixels, to hand to another frame or holder without a copy.
    pub fn shared_pixels(&self) -> Pixels {
        self.bgra.clone()
    }

    /// The platform's picture, for an encoder that reads it in place.
    pub fn native_picture(&self) -> Option<&dyn NativePicture> {
        self.bgra.native_picture()
    }

    pub fn pixels(&self) -> &[u8] {
        &self.bgra
    }

    pub fn row(&self, y: u16) -> &[u8] {
        let start = usize::from(y) * self.stride;
        let row = usize::from(self.width) * 4;
        &self.bgra[start..start + row]
    }
}

impl Clone for RawFrame {
    fn clone(&self) -> Self {
        Self {
            width: self.width,
            height: self.height,
            stride: self.stride,
            timestamp_us: self.timestamp_us,
            captured_at: self.captured_at,
            changed: self.changed.clone(),
            bgra: self.bgra.clone(),
        }
    }
}

impl fmt::Debug for RawFrame {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RawFrame")
            .field("width", &self.width)
            .field("height", &self.height)
            .field("stride", &self.stride)
            .field("timestamp_us", &self.timestamp_us)
            .field("pixels", &self.bgra)
            .finish()
    }
}

/// Even output size that fits inside the quality limit and keeps the aspect ratio.
///
/// H.264 sample dimensions are even. A size that rounds below 2 by 2 is rejected.
pub fn encoded_size(
    src_width: u16,
    src_height: u16,
    max_width: u16,
    max_height: u16,
) -> Result<(u16, u16), Error> {
    if src_width == 0 || src_height == 0 || max_width == 0 || max_height == 0 {
        return Err(Error::InvalidSetting(
            "encoded size must be at least 2 by 2",
        ));
    }
    let mut width = u32::from(src_width);
    let mut height = u32::from(src_height);
    let max_width = u32::from(max_width);
    let max_height = u32::from(max_height);
    if width > max_width || height > max_height {
        let width_tighter =
            u64::from(width) * u64::from(max_height) > u64::from(height) * u64::from(max_width);
        if width_tighter {
            height = height * max_width / width;
            width = max_width;
        } else {
            width = width * max_height / height;
            height = max_height;
        }
    }
    width &= !1;
    height &= !1;
    if width < 2 || height < 2 {
        return Err(Error::InvalidSetting(
            "encoded size must be at least 2 by 2",
        ));
    }
    Ok((width as u16, height as u16))
}

/// Nearest-neighbor scale into a tightly packed BGRA frame.
pub fn scale_bgra(frame: &RawFrame, dst_width: u16, dst_height: u16) -> Result<RawFrame, Error> {
    if dst_width == 0 || dst_height == 0 {
        return Err(Error::InvalidFrame(
            "frame width and height must be non-zero",
        ));
    }
    if dst_width == frame.width
        && dst_height == frame.height
        && frame.stride == row_bytes(frame.width)
    {
        return Ok(frame.clone());
    }
    let mut bgra = vec![0u8; usize::from(dst_width) * usize::from(dst_height) * 4];
    for y in 0..usize::from(dst_height) {
        let src_y = y * usize::from(frame.height) / usize::from(dst_height);
        let src_row = src_y * frame.stride;
        for x in 0..usize::from(dst_width) {
            let src_x = x * usize::from(frame.width) / usize::from(dst_width);
            let from = src_row + src_x * 4;
            let to = (y * usize::from(dst_width) + x) * 4;
            bgra[to..to + 4].copy_from_slice(&frame.bgra[from..from + 4]);
        }
    }
    RawFrame::bgra(dst_width, dst_height, frame.timestamp_us, bgra)
}

fn row_bytes(width: u16) -> usize {
    usize::from(width) * 4
}

/// Where a picture changed since the one before it: how much of it, and
/// where, as long as a frame can carry the places.
#[derive(Debug, Clone, PartialEq)]
pub struct Changes {
    /// The share of the picture, from 0 to 1; overlaps count twice, so it
    /// is never below what changed.
    pub share: f32,
    /// The changed parts, in the picture's pixels; `None` when they were
    /// more than [`vnilink::MAX_REGIONS`].
    pub regions: Option<Vec<vnilink::Region>>,
}

impl Changes {
    /// Nothing changed.
    pub fn none() -> Self {
        Self {
            share: 0.0,
            regions: Some(Vec::new()),
        }
    }

    /// The changes `rects` make to a `width` by `height` picture, each as
    /// `[x, y, width, height]` and clipped to it; empty ones say nothing.
    pub fn of(rects: impl IntoIterator<Item = [i64; 4]>, width: u32, height: u32) -> Self {
        let area = u64::from(width) * u64::from(height);
        let mut covered = 0u64;
        let mut regions = Some(Vec::new());
        for [x, y, w, h] in rects {
            let left = x.clamp(0, i64::from(width));
            let top = y.clamp(0, i64::from(height));
            let right = x.saturating_add(w).clamp(0, i64::from(width));
            let bottom = y.saturating_add(h).clamp(0, i64::from(height));
            if right <= left || bottom <= top {
                continue;
            }
            covered += ((right - left) * (bottom - top)) as u64;
            regions = regions
                .filter(|regions: &Vec<vnilink::Region>| regions.len() < vnilink::MAX_REGIONS)
                .map(|mut regions| {
                    regions.push(vnilink::Region {
                        x: left as u16,
                        y: top as u16,
                        width: (right - left) as u16,
                        height: (bottom - top) as u16,
                    });
                    regions
                });
        }
        Self {
            share: if area == 0 {
                1.0
            } else {
                (covered.min(area) as f64 / area as f64) as f32
            },
            regions,
        }
    }

    /// The changes of two pictures in turn.
    pub fn join(self, after: Self) -> Self {
        Self {
            share: (self.share + after.share).min(1.0),
            regions: self
                .regions
                .zip(after.regions)
                .and_then(|(mut regions, after)| {
                    regions.extend(after);
                    (regions.len() <= vnilink::MAX_REGIONS).then_some(regions)
                }),
        }
    }
}

/// The side of the tiles [`changed_tiles`] compares, in pixels.
const CHANGE_TILE: usize = 64;

/// Where two BGRA pictures of `width` by `height` differ, by
/// [`CHANGE_TILE`]-pixel tiles, `left` and `right` each with its stride:
/// [`Changes::none`] for the same picture. Changed tiles side by side make
/// one region. Row padding is not compared; `None` when either is too short
/// for its rows.
pub fn changed_tiles(
    (left, left_stride): (&[u8], usize),
    (right, right_stride): (&[u8], usize),
    width: u16,
    height: u16,
) -> Option<Changes> {
    let (width, height) = (usize::from(width), usize::from(height));
    let columns = width.div_ceil(CHANGE_TILE);
    let rows = height.div_ceil(CHANGE_TILE);
    let mut changed = vec![false; columns * rows];
    let row_bytes = width * 4;
    for y in 0..height {
        let left_row = left.get(y * left_stride..y * left_stride + row_bytes)?;
        let right_row = right.get(y * right_stride..y * right_stride + row_bytes)?;
        let band = &mut changed[y / CHANGE_TILE * columns..][..columns];
        for (column, tile) in band.iter_mut().enumerate() {
            if *tile {
                continue;
            }
            let from = column * CHANGE_TILE * 4;
            let to = (from + CHANGE_TILE * 4).min(row_bytes);
            *tile = left_row[from..to] != right_row[from..to];
        }
    }
    let mut runs = Vec::new();
    for row in 0..rows {
        let mut column = 0;
        while column < columns {
            if !changed[row * columns + column] {
                column += 1;
                continue;
            }
            let start = column;
            while column < columns && changed[row * columns + column] {
                column += 1;
            }
            runs.push([
                (start * CHANGE_TILE) as i64,
                (row * CHANGE_TILE) as i64,
                ((column - start) * CHANGE_TILE) as i64,
                CHANGE_TILE as i64,
            ]);
        }
    }
    Some(Changes::of(runs, width as u32, height as u32))
}
