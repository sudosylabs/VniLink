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

    /// Move the planes out without a second copy. Host encoders should use
    /// this instead of `y_plane().to_vec()` + `uv_plane().to_vec()`.
    pub fn into_planes(self) -> (Vec<u8>, Vec<u8>) {
        (self.y, self.uv)
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

/// Convert limited-range NV12 to tightly packed BGRA.
///
/// Inverse of [`bgra_to_nv12`] within integer rounding: each channel lands
/// within 2 of the original. Width and height must be even.
pub fn nv12_to_bgra(
    width: u16,
    height: u16,
    timestamp_us: u64,
    y: &[u8],
    uv: &[u8],
) -> Result<RawFrame, Error> {
    if width == 0 || height == 0 || width % 2 != 0 || height % 2 != 0 {
        return Err(Error::InvalidFrame("NV12 width and height must be even"));
    }
    let (width_usize, height_usize) = (usize::from(width), usize::from(height));
    if y.len() < width_usize * height_usize || uv.len() < width_usize * height_usize / 2 {
        return Err(Error::InvalidFrame("frame is missing pixels"));
    }
    let mut bgra = vec![0u8; width_usize * height_usize * 4];
    // Chroma is shared by a 2x2 block. Derive its RGB contributions once
    // and walk bounded row chunks instead of indexing every pixel separately.
    for ((luma, chroma), output) in y[..width_usize * height_usize]
        .chunks_exact(width_usize * 2)
        .zip(uv[..width_usize * height_usize / 2].chunks_exact(width_usize))
        .zip(bgra.chunks_exact_mut(width_usize * 8))
    {
        let (top, bottom) = luma.split_at(width_usize);
        let (out_top, out_bottom) = output.split_at_mut(width_usize * 4);
        for ((((top, bottom), out_top), out_bottom), uv) in top
            .chunks_exact(2)
            .zip(bottom.chunks_exact(2))
            .zip(out_top.chunks_exact_mut(8))
            .zip(out_bottom.chunks_exact_mut(8))
            .zip(chroma.chunks_exact(2))
        {
            let d = i32::from(uv[0]) - 128;
            let e = i32::from(uv[1]) - 128;
            let red = 409 * e + 128;
            let green = -100 * d - 208 * e + 128;
            let blue = 516 * d + 128;
            let rgb = |luma: u8| {
                let c = 298 * (i32::from(luma) - 16);
                [
                    clamp((c + blue) >> 8),
                    clamp((c + green) >> 8),
                    clamp((c + red) >> 8),
                    255,
                ]
            };
            out_top[..4].copy_from_slice(&rgb(top[0]));
            out_top[4..].copy_from_slice(&rgb(top[1]));
            out_bottom[..4].copy_from_slice(&rgb(bottom[0]));
            out_bottom[4..].copy_from_slice(&rgb(bottom[1]));
        }
    }
    RawFrame::bgra(width, height, timestamp_us, bgra)
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
    // Visit each 2x2 block once. Previously three of its pixels were
    // converted twice: once for their luma, then again for averaged chroma.
    for row in (0..height).step_by(2) {
        let top = frame.row(row as u16);
        let bottom = frame.row((row + 1) as u16);
        let (y_top, y_bottom) = y_plane[row * width..(row + 2) * width].split_at_mut(width);
        let uv_row = &mut uv_plane[row / 2 * width..(row / 2 + 1) * width];
        let sources = top.chunks_exact(8).zip(bottom.chunks_exact(8));
        let luma = y_top.chunks_exact_mut(2).zip(y_bottom.chunks_exact_mut(2));
        for (((top, bottom), (y_top, y_bottom)), uv) in
            sources.zip(luma).zip(uv_row.chunks_exact_mut(2))
        {
            let a = yuv(top[2], top[1], top[0]);
            let b = yuv(top[6], top[5], top[4]);
            let c = yuv(bottom[2], bottom[1], bottom[0]);
            let d = yuv(bottom[6], bottom[5], bottom[4]);
            y_top.copy_from_slice(&[a.0, b.0]);
            y_bottom.copy_from_slice(&[c.0, d.0]);
            uv[0] = ((u16::from(a.1) + u16::from(b.1) + u16::from(c.1) + u16::from(d.1)) / 4) as u8;
            uv[1] = ((u16::from(a.2) + u16::from(b.2) + u16::from(c.2) + u16::from(d.2)) / 4) as u8;
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
    // With eight-bit RGB these coefficients bound Y to 16..=235 and
    // U/V to 16..=240; clamping here repeats work without changing output.
    (y as u8, u as u8, v as u8)
}

fn clamp(value: i32) -> u8 {
    value.clamp(0, 255) as u8
}
