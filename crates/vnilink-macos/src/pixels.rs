//! Copy a ScreenCaptureKit BGRA picture into a tightly packed frame.
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

/// Copy a locked `CVPixelBuffer` into a tightly packed BGRA picture.
#[cfg(target_os = "macos")]
pub fn raw_frame_from_pixel_buffer(
    buffer: &apple_cf::cv::CVPixelBuffer,
    timestamp_us: u64,
) -> Result<RawFrame, Error> {
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
    let guard = buffer
        .lock_read_only()
        .map_err(|_| Error::CaptureRejected("ScreenCaptureKit did not deliver a frame"))?;
    let stride = guard.bytes_per_row();
    let row = usize::from(width) * 4;
    if stride < row {
        return Err(Error::InvalidFrame("frame stride is shorter than one row"));
    }
    // The stream has finished this buffer and this guard is the only mapping.
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
    RawFrame::bgra(width, height, timestamp_us, bgra)
}
