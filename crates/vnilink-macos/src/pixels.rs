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

/// The share of the picture that changed between two BGRA pixel buffers:
/// 0 for the same picture. ScreenCaptureKit marks a frame complete at the
/// full frame rate whether or not anything on the screen changed, and
/// attaches no dirty rects, so only the pixels say. Both stay where they
/// are: on Apple silicon the buffers are in shared memory and are read in
/// place. `None` when either cannot be read, or the sizes differ.
#[cfg(target_os = "macos")]
pub(crate) fn picture_change(
    left: &apple_cf::cv::CVPixelBuffer,
    right: &apple_cf::cv::CVPixelBuffer,
) -> Option<f32> {
    let (Ok(size), Ok(other)) = (bgra_size(left), bgra_size(right)) else {
        return None;
    };
    if size != other {
        return None;
    }
    let (Ok(left), Ok(right)) = (left.lock_read_only(), right.lock_read_only()) else {
        return None;
    };
    // The stream has finished both buffers; these guards only read them.
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
    Ok(bgra)
}
