// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! ScreenCaptureKit BGRA pictures: kept in their IOSurface, or copied into
//! a tightly packed frame.
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

/// A ScreenCaptureKit picture still in its IOSurface. VideoToolbox encodes
/// it there ([`crate::MacEncoder`]); anything else that reads its pixels
/// gets them copied out once ([`vnilink_media::Pixels`]).
#[cfg(target_os = "macos")]
pub struct MacPicture {
    buffer: apple_cf::cv::CVPixelBuffer,
    size: (u16, u16),
}

#[cfg(target_os = "macos")]
impl MacPicture {
    /// A single-plane BGRA buffer, as the capture asks ScreenCaptureKit for.
    pub fn new(buffer: apple_cf::cv::CVPixelBuffer) -> Result<Self, Error> {
        let size = bgra_size(&buffer)?;
        Ok(Self { buffer, size })
    }

    pub fn pixel_buffer(&self) -> &apple_cf::cv::CVPixelBuffer {
        &self.buffer
    }
}

#[cfg(target_os = "macos")]
impl vnilink_media::NativePicture for MacPicture {
    fn size(&self) -> (u16, u16) {
        self.size
    }

    fn read_back(&self) -> Result<Vec<u8>, Error> {
        packed_bgra(&self.buffer, self.size)
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

/// Where ScreenCaptureKit says a frame changed, as `[x, y, width, height]`
/// rects, without reading a pixel. The rects are in the units of the frame's
/// content rect, `content` (its width and height): points of the picture,
/// which on a scaled display are half its pixels, or fewer once the picture
/// is delivered smaller than the display. They are scaled by the picture's
/// size over `content`. `None` when there are no rects or no usable content
/// rect: the pixels say.
#[cfg(any(target_os = "macos", test))]
pub(crate) fn reported_changes(
    rects: &[[f64; 4]],
    content: Option<(f64, f64)>,
    (width, height): (u16, u16),
) -> Option<vnilink_media::Changes> {
    let (content_width, content_height) = content?;
    if rects.is_empty() || content_width <= 0.0 || content_height <= 0.0 {
        return None;
    }
    let across = f64::from(width) / content_width;
    let down = f64::from(height) / content_height;
    Some(vnilink_media::Changes::of(
        rects.iter().map(|[x, y, w, h]| {
            // Outwards to whole pixels: a fraction of one has changed.
            let (left, top) = ((x * across).floor(), (y * down).floor());
            [
                left as i64,
                top as i64,
                ((x + w) * across).ceil() as i64 - left as i64,
                ((y + h) * down).ceil() as i64 - top as i64,
            ]
        }),
        u32::from(width),
        u32::from(height),
    ))
}

/// Where the picture changed between two BGRA pixel buffers: nothing for
/// the same picture. ScreenCaptureKit marks a frame complete at the
/// full frame rate whether or not anything on the screen changed; when it
/// attaches no dirty rects ([`reported_changes`]) only the pixels say. Both stay where they
/// are: on Apple silicon the buffers are in shared memory and are read in
/// place. `None` when either cannot be read, or the sizes differ.
#[cfg(target_os = "macos")]
pub(crate) fn picture_change(
    left: &apple_cf::cv::CVPixelBuffer,
    right: &apple_cf::cv::CVPixelBuffer,
) -> Option<vnilink_media::Changes> {
    let (Ok(size), Ok(other)) = (bgra_size(left), bgra_size(right)) else {
        return None;
    };
    if size != other {
        return None;
    }
    let (Ok(left), Ok(right)) = (left.lock_read_only(), right.lock_read_only()) else {
        return None;
    };
    // SAFETY: the stream has finished both buffers and each guard holds a read-only lock for as long as its slice is used, so the mapped bytes stay allocated, initialised and unmodified. Nothing here mutates them.
    let (Some(left_bytes), Some(right_bytes)) =
        (unsafe { left.as_slice() }, unsafe { right.as_slice() })
    else {
        return None;
    };
    vnilink_media::changed_tiles(
        (left_bytes, left.bytes_per_row()),
        (right_bytes, right.bytes_per_row()),
        size.0,
        size.1,
    )
}

/// Copy a locked `CVPixelBuffer` into a tightly packed BGRA picture.
#[cfg(target_os = "macos")]
pub fn raw_frame_from_pixel_buffer(
    buffer: &apple_cf::cv::CVPixelBuffer,
    timestamp_us: u64,
) -> Result<RawFrame, Error> {
    let (width, height) = bgra_size(buffer)?;
    RawFrame::bgra(
        width,
        height,
        timestamp_us,
        packed_bgra(buffer, (width, height))?,
    )
}

/// The size of a single-plane BGRA buffer; any other is refused.
#[cfg(target_os = "macos")]
fn bgra_size(buffer: &apple_cf::cv::CVPixelBuffer) -> Result<(u16, u16), Error> {
    const BGRA: u32 = 0x4247_5241;
    if buffer.is_planar() || buffer.pixel_format() != BGRA {
        return Err(Error::Unsupported("this ScreenCaptureKit pixel format"));
    }
    let width = u16::try_from(buffer.width())
        .map_err(|_| Error::InvalidFrame("frame width and height must be non-zero"))?;
    let height = u16::try_from(buffer.height())
        .map_err(|_| Error::InvalidFrame("frame width and height must be non-zero"))?;
    if width == 0 || height == 0 {
        return Err(Error::InvalidFrame(
            "frame width and height must be non-zero",
        ));
    }
    Ok((width, height))
}

/// The buffer's rows copied into tightly packed BGRA.
#[cfg(target_os = "macos")]
fn packed_bgra(
    buffer: &apple_cf::cv::CVPixelBuffer,
    (width, height): (u16, u16),
) -> Result<Vec<u8>, Error> {
    let guard = buffer
        .lock_read_only()
        .map_err(|_| Error::CaptureRejected("ScreenCaptureKit did not deliver a frame"))?;
    let stride = guard.bytes_per_row();
    let row = usize::from(width) * 4;
    if stride < row {
        return Err(Error::InvalidFrame("frame stride is shorter than one row"));
    }
    // SAFETY: the stream has finished this buffer and the read-only lock `guard` holds keeps the mapped bytes allocated, initialised and unmodified until `source` is last used.
    let source = unsafe { guard.as_slice() }.ok_or(Error::CaptureRejected(
        "ScreenCaptureKit did not deliver a frame",
    ))?;
    let mut bgra = vec![0u8; row * usize::from(height)];
    for y in 0..usize::from(height) {
        let src_start = y * stride;
        let dst_start = y * row;
        let end = src_start + row;
        if end > source.len() {
            return Err(Error::InvalidFrame("frame is missing pixels"));
        }
        bgra[dst_start..dst_start + row].copy_from_slice(&source[src_start..end]);
    }
    Ok(bgra)
}
