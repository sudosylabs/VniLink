//! Checked native RGB32 rows. Negative MF stride denotes bottom-up storage.
use vnilink_media::{Error, RawFrame};
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
