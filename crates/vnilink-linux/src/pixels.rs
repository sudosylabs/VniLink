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
