//! One BGRA picture. The bytes are screen content: do not log them.

use std::fmt;

use crate::error::Error;

/// Tightly packed or strided BGRA8, origin at the top left.
pub struct RawFrame {
    width: u16,
    height: u16,
    stride: usize,
    timestamp_us: u64,
    bgra: Vec<u8>,
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
        if width == 0 || height == 0 {
            return Err(Error::InvalidFrame(
                "frame width and height must be non-zero",
            ));
        }
        let row = usize::from(width).saturating_mul(4);
        if stride < row {
            return Err(Error::InvalidFrame("frame stride is shorter than one row"));
        }
        let needed = stride.saturating_mul(usize::from(height));
        if bgra.len() < needed {
            return Err(Error::InvalidFrame("frame is missing pixels"));
        }
        Ok(Self {
            width,
            height,
            stride,
            timestamp_us,
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
            .field("bytes", &self.bgra.len())
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
