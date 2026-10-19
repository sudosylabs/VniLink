//! Checked native RGB32 rows, and the camera format nearest a request.
use vnilink_media::{Error, RawFrame};

/// The camera's own size and rate nearest the request, so Media Foundation
/// converts colour only and never scales or letterboxes the picture.
/// Candidates are `(width, height, fps)`; odd or out-of-range ones are skipped.
pub(crate) fn nearest_format(
    candidates: impl IntoIterator<Item = (u32, u32, u32)>,
    width: u16,
    height: u16,
    fps: u16,
) -> Option<(u16, u16, u16)> {
    candidates
        .into_iter()
        .filter_map(|(w, h, f)| {
            let (w, h, f) = (
                u16::try_from(w).ok()?,
                u16::try_from(h).ok()?,
                u16::try_from(f).ok()?,
            );
            (w > 0
                && h > 0
                && w <= 4096
                && h <= 4096
                && w % 2 == 0
                && h % 2 == 0
                && (1..=60).contains(&f))
            .then_some((w, h, f))
        })
        .min_by_key(|&(w, h, f)| {
            u32::from(w.abs_diff(width))
                + u32::from(h.abs_diff(height))
                + u32::from(f.abs_diff(fps)) * 8
        })
}
#[derive(Clone, Copy)]
pub(crate) struct Format {
    pub width: u16,
    pub height: u16,
    pub stride: i32,
}
impl Format {
    pub fn copy(&self, bytes: &[u8], timestamp: u64) -> Result<RawFrame, Error> {
        let row = usize::from(self.width)
            .checked_mul(4)
            .ok_or(Error::InvalidFrame("camera row size"))?;
        let stride = self.stride.unsigned_abs() as usize;
        let height = usize::from(self.height);
        if self.width == 0 || self.height == 0 || stride < row || stride > row + 4096 {
            return Err(Error::InvalidFrame("camera stride is out of range"));
        }
        let needed = stride
            .checked_mul(height)
            .ok_or(Error::InvalidFrame("camera buffer size"))?;
        if bytes.len() < needed || row.checked_mul(height).is_none_or(|n| n > 64 * 1024 * 1024) {
            return Err(Error::InvalidFrame("camera sample is incomplete"));
        }
        let mut pixels = Vec::with_capacity(row * height);
        for y in 0..height {
            let sy = if self.stride < 0 { height - 1 - y } else { y };
            pixels.extend_from_slice(&bytes[sy * stride..sy * stride + row]);
        }
        RawFrame::bgra(self.width, self.height, timestamp, pixels)
    }
    /// Rows addressed from the first scan line by a signed pitch, as
    /// `IMF2DBuffer2::Lock2DSize` reports them: the top row is always at
    /// `scan0`, whichever way the buffer is stored. `memory` is the locked
    /// buffer and `scan0` an offset into it.
    pub fn copy_pitched(
        &self,
        memory: &[u8],
        scan0: usize,
        pitch: isize,
        timestamp: u64,
    ) -> Result<RawFrame, Error> {
        let row = usize::from(self.width)
            .checked_mul(4)
            .ok_or(Error::InvalidFrame("camera row size"))?;
        let height = usize::from(self.height);
        if self.width == 0
            || self.height == 0
            || pitch.unsigned_abs() < row
            || pitch.unsigned_abs() > row + 4096
            || row.checked_mul(height).is_none_or(|n| n > 64 * 1024 * 1024)
        {
            return Err(Error::InvalidFrame("camera pitch is out of range"));
        }
        let mut pixels = Vec::with_capacity(row * height);
        for y in 0..height {
            let start = isize::try_from(scan0)
                .ok()
                .and_then(|scan0| scan0.checked_add(pitch.checked_mul(y as isize)?))
                .and_then(|start| usize::try_from(start).ok())
                .filter(|start| {
                    start
                        .checked_add(row)
                        .is_some_and(|end| end <= memory.len())
                })
                .ok_or(Error::InvalidFrame("camera sample is incomplete"))?;
            pixels.extend_from_slice(&memory[start..start + row]);
        }
        RawFrame::bgra(self.width, self.height, timestamp, pixels)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn top_down_and_bottom_up_rows_remove_native_padding_without_reversing_pixels() {
        let bytes = [
            1, 2, 3, 4, 5, 6, 7, 8, 99, 99, 99, 99, 11, 12, 13, 14, 15, 16, 17, 18, 88, 88, 88, 88,
        ];
        let top = Format {
            width: 2,
            height: 2,
            stride: 12,
        }
        .copy(&bytes, 9)
        .unwrap();
        assert_eq!(
            top.pixels(),
            &[1, 2, 3, 4, 5, 6, 7, 8, 11, 12, 13, 14, 15, 16, 17, 18]
        );
        let bottom = Format {
            width: 2,
            height: 2,
            stride: -12,
        }
        .copy(&bytes, 10)
        .unwrap();
        assert_eq!(
            bottom.pixels(),
            &[11, 12, 13, 14, 15, 16, 17, 18, 1, 2, 3, 4, 5, 6, 7, 8]
        );
        assert_eq!(bottom.timestamp_us(), 10);
    }
    #[test]
    fn pitched_rows_start_at_the_first_scan_line_either_way() {
        // Two rows of one pixel; padding 9s. Stored top-down, then bottom-up.
        let top_down = [1, 1, 1, 1, 9, 9, 2, 2, 2, 2, 9, 9];
        let format = Format {
            width: 1,
            height: 2,
            stride: 0,
        };
        let frame = format.copy_pitched(&top_down, 0, 6, 1).unwrap();
        assert_eq!(frame.pixels(), &[1, 1, 1, 1, 2, 2, 2, 2]);
        // Bottom-up: the first scan line is stored last and the pitch is negative.
        let bottom_up = [2, 2, 2, 2, 9, 9, 1, 1, 1, 1, 9, 9];
        let frame = format.copy_pitched(&bottom_up, 6, -6, 1).unwrap();
        assert_eq!(frame.pixels(), &[1, 1, 1, 1, 2, 2, 2, 2]);
        // A pitch that walks out of the buffer either way is refused.
        assert!(format.copy_pitched(&bottom_up, 0, -6, 1).is_err());
        assert!(format.copy_pitched(&top_down, 6, 6, 1).is_err());
        assert!(format.copy_pitched(&top_down, 0, 3, 1).is_err());
    }
    #[test]
    fn the_nearest_native_format_keeps_the_cameras_shape() {
        // A 4:3 camera asked for 720p gets its own 1280x960, not a letterboxed 1280x720.
        let formats = [
            (640, 480, 30),
            (1280, 960, 30),
            (1920, 1440, 15),
            (1281, 720, 30),
            (5000, 720, 30),
        ];
        assert_eq!(
            nearest_format(formats, 1280, 720, 30),
            Some((1280, 960, 30))
        );
        assert_eq!(
            nearest_format(
                [(1920, 1080, 30), (1280, 720, 60), (1280, 720, 30)],
                1280,
                720,
                30
            ),
            Some((1280, 720, 30))
        );
        assert_eq!(
            nearest_format([(0, 0, 30), (640, 480, 0)], 1280, 720, 30),
            None
        );
    }
    #[test]
    fn malformed_stride_and_short_native_samples_are_refused() {
        for stride in [0, 3, i32::MIN, i32::MAX] {
            assert!(Format {
                width: 2,
                height: 2,
                stride
            }
            .copy(&[0; 16], 0)
            .is_err());
        }
        assert!(Format {
            width: 2,
            height: 2,
            stride: -8
        }
        .copy(&[0; 15], 0)
        .is_err());
    }
}
