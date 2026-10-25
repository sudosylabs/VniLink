//! BGRA to limited-range NV12 in the session's matrix
//! ([`crate::VideoSignal::stream`]), and back from NV12 in the colour a stream
//! says. Media Foundation and VA-API take this layout. The software codecs
//! take and return planar I420 through the same conversions.
//! The planes are screen content: do not log them.

use std::fmt;

use crate::colour::{ToRgb, ToYuv, VideoSignal, YuvMatrix};
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

/// Convert NV12 in the colour `signal` names to tightly packed BGRA.
///
/// With [`crate::VideoSignal::stream`] this is the inverse of [`bgra_to_nv12`]
/// within integer rounding: each channel lands within 2 of the original.
/// Width and height must be even.
pub fn nv12_to_bgra(
    width: u16,
    height: u16,
    timestamp_us: u64,
    y: &[u8],
    uv: &[u8],
    signal: VideoSignal,
) -> Result<RawFrame, Error> {
    if width == 0 || height == 0 || width % 2 != 0 || height % 2 != 0 {
        return Err(Error::InvalidFrame("NV12 width and height must be even"));
    }
    let (width_usize, height_usize) = (usize::from(width), usize::from(height));
    if y.len() < width_usize * height_usize || uv.len() < width_usize * height_usize / 2 {
        return Err(Error::InvalidFrame("frame is missing pixels"));
    }
    let bgra = yuv420_to_bgra(
        width_usize,
        height_usize,
        |row| &y[row * width_usize..(row + 1) * width_usize],
        |row| {
            uv[row * width_usize..(row + 1) * width_usize]
                .chunks_exact(2)
                .map(|pair| (pair[0], pair[1]))
        },
        signal,
    );
    RawFrame::bgra(width, height, timestamp_us, bgra)
}

/// Convert planar I420 in the colour `signal` names to tightly packed BGRA.
/// Each plane is read through its own stride, as decoders return them.
///
/// The caller checks that width and height are even and that every plane
/// holds its rows at its stride.
pub(crate) fn i420_to_bgra(
    width: u16,
    height: u16,
    timestamp_us: u64,
    [y, u, v]: [&[u8]; 3],
    [y_stride, u_stride, v_stride]: [usize; 3],
    signal: VideoSignal,
) -> Result<RawFrame, Error> {
    let (width_usize, height_usize) = (usize::from(width), usize::from(height));
    let half = width_usize / 2;
    let bgra = yuv420_to_bgra(
        width_usize,
        height_usize,
        |row| &y[row * y_stride..row * y_stride + width_usize],
        |row| {
            u[row * u_stride..row * u_stride + half]
                .iter()
                .copied()
                .zip(v[row * v_stride..row * v_stride + half].iter().copied())
        },
        signal,
    );
    RawFrame::bgra(width, height, timestamp_us, bgra)
}

/// Tightly packed BGRA from 4:2:0 rows: `luma(row)` is one row of Y,
/// `chroma(row)` the Cb, Cr pairs of one half-height row.
fn yuv420_to_bgra<'a, C>(
    width: usize,
    height: usize,
    luma: impl Fn(usize) -> &'a [u8],
    chroma: impl Fn(usize) -> C,
    signal: VideoSignal,
) -> Vec<u8>
where
    C: Iterator<Item = (u8, u8)>,
{
    let to_rgb = ToRgb::of(signal);
    let mut bgra = vec![0u8; width * height * 4];
    // Chroma is shared by a 2x2 block. Derive its RGB contributions once
    // and walk bounded row chunks instead of indexing every pixel separately.
    for (pair, output) in bgra.chunks_exact_mut(width * 8).enumerate() {
        let (top, bottom) = (luma(pair * 2), luma(pair * 2 + 1));
        let (out_top, out_bottom) = output.split_at_mut(width * 4);
        for ((((top, bottom), out_top), out_bottom), (cb, cr)) in top
            .chunks_exact(2)
            .zip(bottom.chunks_exact(2))
            .zip(out_top.chunks_exact_mut(8))
            .zip(out_bottom.chunks_exact_mut(8))
            .zip(chroma(pair))
        {
            let d = i32::from(cb) - 128;
            let e = i32::from(cr) - 128;
            let red = to_rgb.cr_red * e + 128;
            let green = to_rgb.cb_green * d + to_rgb.cr_green * e + 128;
            let blue = to_rgb.cb_blue * d + 128;
            let rgb = |luma: u8| {
                let c = to_rgb.luma * (i32::from(luma) - to_rgb.luma_offset);
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
    bgra
}

/// Convert BGRA to limited-range NV12 in the session's `matrix`.
///
/// Width and height must be even. Chroma is the average of each 2 by 2 block.
pub fn bgra_to_nv12(frame: &RawFrame, matrix: YuvMatrix) -> Result<Nv12, Error> {
    if frame.width() % 2 != 0 || frame.height() % 2 != 0 {
        return Err(Error::InvalidFrame("NV12 width and height must be even"));
    }
    let width = usize::from(frame.width());
    let mut y = vec![0u8; width * usize::from(frame.height())];
    let mut uv = vec![0u8; y.len() / 2];
    bgra_to_yuv420(frame, ToYuv::of(matrix), &mut y, |row, column, cb, cr| {
        uv[row * width + column * 2..][..2].copy_from_slice(&[cb, cr]);
    });
    Ok(Nv12 {
        width: frame.width(),
        height: frame.height(),
        y,
        uv,
    })
}

/// Convert BGRA to limited-range planar I420 (Y, Cb, Cr) in `signal`'s
/// matrix, as the software encoders take it.
///
/// Width and height must be even. Chroma is the average of each 2 by 2 block.
pub(crate) fn bgra_to_i420(
    frame: &RawFrame,
    signal: VideoSignal,
) -> Result<(Vec<u8>, Vec<u8>, Vec<u8>), Error> {
    if frame.width() % 2 != 0 || frame.height() % 2 != 0 {
        return Err(Error::InvalidFrame("software YUV dimensions must be even"));
    }
    let half = usize::from(frame.width()) / 2;
    let mut y = vec![0u8; usize::from(frame.width()) * usize::from(frame.height())];
    let mut u = vec![0u8; y.len() / 4];
    let mut v = vec![0u8; y.len() / 4];
    bgra_to_yuv420(
        frame,
        ToYuv::of(signal.yuv_matrix()),
        &mut y,
        |row, column, cb, cr| {
            u[row * half + column] = cb;
            v[row * half + column] = cr;
        },
    );
    Ok((y, u, v))
}

/// Writes `frame`'s luma into `y` and hands each 2x2 block's averaged
/// chroma to `chroma(half_row, half_column, cb, cr)`. The caller checks that
/// width and height are even.
fn bgra_to_yuv420(
    frame: &RawFrame,
    to_yuv: ToYuv,
    y: &mut [u8],
    mut chroma: impl FnMut(usize, usize, u8, u8),
) {
    let width = usize::from(frame.width());
    // Visit each 2x2 block once, converting each pixel once.
    for (pair, luma) in y.chunks_exact_mut(width * 2).enumerate() {
        let top = frame.row((pair * 2) as u16);
        let bottom = frame.row((pair * 2 + 1) as u16);
        let (y_top, y_bottom) = luma.split_at_mut(width);
        let sources = top.chunks_exact(8).zip(bottom.chunks_exact(8));
        let rows = y_top.chunks_exact_mut(2).zip(y_bottom.chunks_exact_mut(2));
        for (column, ((top, bottom), (y_top, y_bottom))) in sources.zip(rows).enumerate() {
            let a = to_yuv.convert(top[2], top[1], top[0]);
            let b = to_yuv.convert(top[6], top[5], top[4]);
            let c = to_yuv.convert(bottom[2], bottom[1], bottom[0]);
            let d = to_yuv.convert(bottom[6], bottom[5], bottom[4]);
            y_top.copy_from_slice(&[a.0, b.0]);
            y_bottom.copy_from_slice(&[c.0, d.0]);
            let average =
                |values: [u8; 4]| (values.iter().map(|&v| u16::from(v)).sum::<u16>() / 4) as u8;
            chroma(
                pair,
                column,
                average([a.1, b.1, c.1, d.1]),
                average([a.2, b.2, c.2, d.2]),
            );
        }
    }
}

/// Limited-range Y'CbCr of one pixel in `matrix`.
pub(crate) fn yuv(matrix: YuvMatrix, red: u8, green: u8, blue: u8) -> (u8, u8, u8) {
    ToYuv::of(matrix).convert(red, green, blue)
}

fn clamp(value: i32) -> u8 {
    value.clamp(0, 255) as u8
}
