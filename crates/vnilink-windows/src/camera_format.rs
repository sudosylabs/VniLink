// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! Checked native RGB32 rows, and the camera format nearest a request.
use vnilink_media::{Error, RawFrame};

/// The native mode a capture reads, and the picture it delivers from it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CameraMode {
    /// Index into the candidates: the camera's own mode to open.
    pub native: usize,
    pub width: u16,
    pub height: u16,
    pub fps: u16,
}

/// The camera's mode for a request, keeping the sensor's shape.
///
/// A camera's largest mode is its sensor's shape. Its modes of another
/// shape are often letterboxed by the camera itself (a 4:3 sensor's 720p
/// arrives with black bars), so only modes of the sensor's shape are used:
/// the smallest at least as wide as the request, else the widest, at the
/// rate nearest the request. The picture keeps that shape, at the
/// requested width when the mode is wider, so Media Foundation only scales
/// it down. Candidates are `(width, height, fps)`; odd or out-of-range
/// ones are skipped.
pub(crate) fn choose_mode(
    candidates: &[(u32, u32, u32)],
    width: u16,
    fps: u16,
) -> Option<CameraMode> {
    let valid: Vec<(usize, u32, u32, u16)> = candidates
        .iter()
        .enumerate()
        .filter_map(|(index, &(w, h, f))| {
            let f = u16::try_from(f).ok()?;
            (w > 0
                && h > 0
                && w <= 4096
                && h <= 4096
                && w % 2 == 0
                && h % 2 == 0
                && (1..=60).contains(&f))
            .then_some((index, w, h, f))
        })
        .collect();
    let &(_, sensor_w, sensor_h, _) = valid
        .iter()
        .max_by_key(|(_, w, h, _)| u64::from(*w) * u64::from(*h))?;
    let shaped: Vec<_> = valid
        .into_iter()
        .filter(|(_, w, h, _)| {
            u64::from(*w) * u64::from(sensor_h) == u64::from(*h) * u64::from(sensor_w)
        })
        .collect();
    let wide_enough = shaped
        .iter()
        .filter(|(_, w, ..)| *w >= u32::from(width))
        .map(|(_, w, ..)| *w)
        .min();
    let mode_width = wide_enough.or_else(|| shaped.iter().map(|(_, w, ..)| *w).max())?;
    let &(native, mode_w, mode_h, mode_fps) = shaped
        .iter()
        .filter(|(_, w, ..)| *w == mode_width)
        .min_by_key(|(.., f)| f.abs_diff(fps))?;
    let out_w = mode_w.min(u32::from(width).max(2)) & !1;
    let out_h = ((u64::from(out_w) * u64::from(mode_h) + u64::from(mode_w) / 2) / u64::from(mode_w))
        as u32
        & !1;
    Some(CameraMode {
        native,
        width: u16::try_from(out_w).ok()?,
        height: u16::try_from(out_h.clamp(2, 4096)).ok()?,
        fps: mode_fps,
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
    fn a_camera_keeps_its_sensors_shape_and_is_only_scaled_down() {
        // This laptop's camera: a 4:3 sensor whose 16:9 modes it letterboxes.
        let laptop = [
            (2592, 1944, 30),
            (2592, 1944, 15),
            (2560, 1440, 30),
            (1920, 1080, 30),
            (1280, 720, 30),
            (1280, 720, 15),
            (960, 540, 30),
            (640, 480, 30),
            (640, 360, 30),
            (320, 240, 30),
        ];
        assert_eq!(
            choose_mode(&laptop, 1280, 30),
            Some(CameraMode {
                native: 0,
                width: 1280,
                height: 960,
                fps: 30
            })
        );
        // A 16:9 camera keeps 16:9 and opens the mode it was asked for.
        let wide = [
            (1920, 1080, 30),
            (1280, 720, 60),
            (1280, 720, 30),
            (640, 360, 30),
        ];
        assert_eq!(
            choose_mode(&wide, 1280, 30),
            Some(CameraMode {
                native: 2,
                width: 1280,
                height: 720,
                fps: 30
            })
        );
        // A small camera is never scaled up.
        assert_eq!(
            choose_mode(&[(640, 480, 30), (320, 240, 30)], 1280, 30),
            Some(CameraMode {
                native: 0,
                width: 640,
                height: 480,
                fps: 30
            })
        );
        // Odd, oversized or rateless modes are skipped, and with none left there is no mode.
        assert_eq!(
            choose_mode(
                &[
                    (5000, 3750, 30),
                    (1281, 720, 30),
                    (640, 480, 0),
                    (1280, 960, 15)
                ],
                1280,
                30
            ),
            Some(CameraMode {
                native: 3,
                width: 1280,
                height: 960,
                fps: 15
            })
        );
        assert_eq!(choose_mode(&[(0, 0, 30), (640, 480, 0)], 1280, 30), None);
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
