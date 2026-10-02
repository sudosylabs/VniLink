//! BGRA to limited-range NV12. Media Foundation and VA-API take this layout.
//! The planes are screen content: do not log them.

use std::fmt;

use crate::error::Error;
use crate::frame::RawFrame;

/// Even-sized NV12. `y` is full resolution. `uv` is interleaved 4:2:0.
pub struct Nv12 {
    width: u16,
    height: u16,
    y: Vec<u8>,
    uv: Vec<u8>,
}

impl Nv12 {
    pub fn width(&self) -> u16 {
        self.width
    }

    pub fn height(&self) -> u16 {
        self.height
    }

    pub fn y_plane(&self) -> &[u8] {
        &self.y
    }

    pub fn uv_plane(&self) -> &[u8] {
        &self.uv
    }
}

impl fmt::Debug for Nv12 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Nv12")
            .field("width", &self.width)
            .field("height", &self.height)
            .field("y_len", &self.y.len())
            .field("uv_len", &self.uv.len())
            .finish()
    }
}

/// Convert BGRA to BT.601 limited-range NV12.
///
/// Width and height must be even. Chroma is the average of each 2 by 2 block.
pub fn bgra_to_nv12(frame: &RawFrame) -> Result<Nv12, Error> {
    if frame.width() % 2 != 0 || frame.height() % 2 != 0 {
        return Err(Error::InvalidFrame("NV12 width and height must be even"));
    }
    let width = usize::from(frame.width());
    let height = usize::from(frame.height());
    let mut y_plane = vec![0u8; width * height];
    let mut uv_plane = vec![0u8; width * height / 2];
    for row in 0..height {
        let src = frame.row(row as u16);
        for col in 0..width {
            let px = &src[col * 4..col * 4 + 4];
            let (y, u, v) = yuv(px[2], px[1], px[0]);
            y_plane[row * width + col] = y;
            if row % 2 == 0 && col % 2 == 0 {
                let mut u_sum = i32::from(u);
                let mut v_sum = i32::from(v);
                for (dy, dx) in [(0, 1), (1, 0), (1, 1)] {
                    let other = frame.row((row + dy) as u16);
                    let sample = &other[(col + dx) * 4..(col + dx) * 4 + 4];
                    let (_, ou, ov) = yuv(sample[2], sample[1], sample[0]);
                    u_sum += i32::from(ou);
                    v_sum += i32::from(ov);
                }
                let uv_index = (row / 2) * width + col;
                uv_plane[uv_index] = (u_sum / 4) as u8;
                uv_plane[uv_index + 1] = (v_sum / 4) as u8;
            }
        }
    }
    Ok(Nv12 {
        width: frame.width(),
        height: frame.height(),
        y: y_plane,
        uv: uv_plane,
    })
}

fn yuv(red: u8, green: u8, blue: u8) -> (u8, u8, u8) {
    let red = i32::from(red);
    let green = i32::from(green);
    let blue = i32::from(blue);
    let y = ((66 * red + 129 * green + 25 * blue + 128) >> 8) + 16;
    let u = ((-38 * red - 74 * green + 112 * blue + 128) >> 8) + 128;
    let v = ((112 * red - 94 * green - 18 * blue + 128) >> 8) + 128;
    (clamp(y), clamp(u), clamp(v))
}

fn clamp(value: i32) -> u8 {
    value.clamp(0, 255) as u8
}
