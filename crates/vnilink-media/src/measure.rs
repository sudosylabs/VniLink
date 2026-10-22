//! Pictures and comparisons for encoder measurements on each platform, so
//! VideoToolbox and Media Foundation are measured on the same screen.

use crate::RawFrame;

pub mod glass;
pub mod timecode;

/// A page of text-like glyphs from a fixed alphabet on white, with a
/// coloured sidebar and one line that grows by a glyph a picture, like
/// typing. Dense enough that a keyframe costs many pictures' bytes.
pub fn text_page(width: u16, height: u16, index: u64) -> RawFrame {
    let (w, h) = (usize::from(width), usize::from(height));
    let mut pixels = vec![255u8; w * h * 4];
    let mut seed = 0x1234_5678u32;
    let mut next = move || {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        seed
    };
    let alphabet: Vec<u32> = (0..40).map(|_| next()).collect();
    for row in pixels.chunks_exact_mut(w * 4) {
        for pixel in row[..(w / 8) * 4].chunks_exact_mut(4) {
            pixel.copy_from_slice(&[80, 60, 40, 255]);
        }
    }
    let (mut y, mut line) = (20, 0);
    while y + 14 < h {
        let mut x = w / 8 + 20;
        let end = if line == 20 {
            x + (index as usize * 9).min(w / 2)
        } else {
            (x + 300 + (next() % 800) as usize).min(w - 20)
        };
        while x + 8 < end {
            let glyph = alphabet[(next() % 40) as usize];
            for gy in 0..12 {
                for gx in 0..7 {
                    if (glyph >> ((gy * 7 + gx) % 31)) & 1 == 1 && (gy + gx) % 3 != 0 {
                        let at = ((y + gy) * w + x + gx) * 4;
                        pixels[at..at + 3].copy_from_slice(&[20, 20, 20]);
                    }
                }
            }
            x += if next() % 7 == 0 { 18 } else { 9 };
        }
        y += 18;
        line += 1;
    }
    RawFrame::bgra(width, height, index * 33_333, pixels).expect("a whole BGRA page")
}

/// Peak signal-to-noise ratio of `decoded` against `source` over RGB, in
/// dB: higher is closer.
pub fn psnr(source: &RawFrame, decoded: &RawFrame) -> f64 {
    let (mut error, mut count) = (0f64, 0f64);
    for y in 0..source.height().min(decoded.height()) {
        let (a, b) = (source.row(y), decoded.row(y));
        for x in 0..usize::from(source.width().min(decoded.width())) {
            for c in 0..3 {
                let d = f64::from(a[x * 4 + c]) - f64::from(b[x * 4 + c]);
                error += d * d;
                count += 1.0;
            }
        }
    }
    10.0 * (255.0f64 * 255.0 / (error / count.max(1.0)).max(1e-9)).log10()
}

/// Coloured text where chroma subsampling shows: syntax-highlighting
/// colours on white, light colours on a dark band, yellow on blue, and
/// one-pixel coloured lines. Glyphs are one or two pixels wide, so 4:2:0
/// cannot carry their colour.
pub fn colour_text_page(width: u16, height: u16, index: u64) -> RawFrame {
    let (w, h) = (usize::from(width), usize::from(height));
    let mut pixels = vec![255u8; w * h * 4];
    let put = |pixels: &mut Vec<u8>, x: usize, y: usize, bgr: [u8; 3]| {
        if x < w && y < h {
            pixels[(y * w + x) * 4..(y * w + x) * 4 + 3].copy_from_slice(&bgr);
        }
    };
    let fill = |pixels: &mut Vec<u8>, top: usize, bottom: usize, bgr: [u8; 3]| {
        for row in pixels[top * w * 4..bottom.min(h) * w * 4].chunks_exact_mut(4) {
            row[..3].copy_from_slice(&bgr);
        }
    };
    // (b, g, r): keyword blue, string red, comment green, number orange,
    // type purple, plain dark grey.
    let on_white = [
        [200, 70, 0],
        [30, 30, 200],
        [40, 140, 40],
        [0, 110, 230],
        [170, 40, 140],
        [60, 60, 60],
    ];
    let on_dark = [
        [255, 200, 120],
        [120, 200, 255],
        [140, 230, 140],
        [80, 180, 255],
        [240, 150, 220],
    ];
    let band = (h / 2, h / 2 + h / 6);
    let blue = (h * 3 / 4, h * 3 / 4 + h / 10);
    fill(&mut pixels, band.0, band.1, [40, 30, 30]);
    fill(&mut pixels, blue.0, blue.1, [140, 60, 20]);
    let mut page = 0x9e37_79b9u32;
    // The typing line draws from its own sequence: the rest of the page
    // must not change as it grows.
    let mut typing = 0x2545_f491u32;
    let alphabet: Vec<u32> = (0..40).map(|_| xorshift(&mut page)).collect();
    let mut y = 12;
    let mut line = 0usize;
    while y + 12 < h {
        let palette: &[[u8; 3]] = if (band.0..band.1).contains(&y) {
            &on_dark
        } else if (blue.0..blue.1).contains(&y) {
            &[[0, 230, 255]]
        } else {
            &on_white
        };
        let mut x = 12;
        // One line grows by a glyph a picture, like typing.
        let end = if line == 3 {
            x + (index as usize * 7).min(w / 2)
        } else {
            (x + 200 + (xorshift(&mut page) % 900) as usize).min(w - 12)
        };
        let rng = if line == 3 { &mut typing } else { &mut page };
        let mut colour = palette[line % palette.len()];
        while x + 6 < end {
            if xorshift(rng) % 5 == 0 {
                colour = palette[(xorshift(rng) as usize) % palette.len()];
            }
            let glyph = alphabet[(xorshift(rng) % 40) as usize];
            // 5x9 glyphs with one-pixel strokes.
            for gy in 0..9 {
                for gx in 0..5 {
                    if (glyph >> ((gy * 5 + gx) % 31)) & 1 == 1 && (gx + gy) % 2 == 0 {
                        put(&mut pixels, x + gx, y + gy, colour);
                    }
                }
            }
            x += if xorshift(rng) % 6 == 0 { 12 } else { 7 };
        }
        y += 14;
        line += 1;
    }
    // One-pixel lines in pure colours.
    for (offset, bgr) in [[0, 0, 255], [0, 200, 0], [255, 0, 0], [255, 0, 255]]
        .into_iter()
        .enumerate()
    {
        let row = h.saturating_sub(8 + offset * 2);
        for x in w / 2..w - 12 {
            put(&mut pixels, x, row, bgr);
        }
    }
    RawFrame::bgra(width, height, index * 33_333, pixels).expect("a whole BGRA page")
}

/// PSNR of each of red, green and blue, in that order, in dB.
pub fn psnr_channels(source: &RawFrame, decoded: &RawFrame) -> [f64; 3] {
    let mut error = [0f64; 3];
    let mut count = 0f64;
    for y in 0..source.height().min(decoded.height()) {
        let (a, b) = (source.row(y), decoded.row(y));
        for x in 0..usize::from(source.width().min(decoded.width())) {
            for (channel, error) in error.iter_mut().enumerate() {
                // BGRA: red is byte 2.
                let at = x * 4 + 2 - channel;
                let d = f64::from(a[at]) - f64::from(b[at]);
                *error += d * d;
            }
            count += 1.0;
        }
    }
    error.map(|error| decibels(error / count.max(1.0)))
}

/// PSNR over RGB at glyph and line edges only: pixels within one pixel of
/// a neighbour that differs from them in the source. Text legibility lives
/// there; flat areas would hide it.
pub fn edge_psnr(source: &RawFrame, decoded: &RawFrame) -> f64 {
    let (w, h) = (
        usize::from(source.width().min(decoded.width())),
        usize::from(source.height().min(decoded.height())),
    );
    let differs = |x: usize, y: usize, nx: usize, ny: usize| {
        let (a, b) = (
            &source.row(y as u16)[x * 4..x * 4 + 3],
            &source.row(ny as u16)[nx * 4..nx * 4 + 3],
        );
        a.iter().zip(b).any(|(a, b)| a.abs_diff(*b) > 32)
    };
    let mut edge = vec![false; w * h];
    for y in 0..h {
        for x in 0..w {
            if (x + 1 < w && differs(x, y, x + 1, y)) || (y + 1 < h && differs(x, y, x, y + 1)) {
                for (ex, ey) in [(x, y), (x + 1, y), (x, y + 1)] {
                    if ex < w && ey < h {
                        edge[ey * w + ex] = true;
                    }
                }
            }
        }
    }
    let (mut error, mut count) = (0f64, 0f64);
    for y in 0..h {
        let (a, b) = (source.row(y as u16), decoded.row(y as u16));
        for x in (0..w).filter(|x| edge[y * w + x]) {
            for c in 0..3 {
                let d = f64::from(a[x * 4 + c]) - f64::from(b[x * 4 + c]);
                error += d * d;
                count += 1.0;
            }
        }
    }
    decibels(error / count.max(1.0))
}

/// PSNR of the BT.709 chroma (Cb and Cr together), the part 4:2:0 halves.
pub fn chroma_psnr(source: &RawFrame, decoded: &RawFrame) -> f64 {
    let chroma = |pixel: &[u8]| {
        let (b, g, r) = (
            f64::from(pixel[0]),
            f64::from(pixel[1]),
            f64::from(pixel[2]),
        );
        let y = 0.2126 * r + 0.7152 * g + 0.0722 * b;
        [(b - y) / 1.8556, (r - y) / 1.5748]
    };
    let (mut error, mut count) = (0f64, 0f64);
    for y in 0..source.height().min(decoded.height()) {
        let (a, b) = (source.row(y), decoded.row(y));
        for x in 0..usize::from(source.width().min(decoded.width())) {
            let (ca, cb) = (chroma(&a[x * 4..x * 4 + 3]), chroma(&b[x * 4..x * 4 + 3]));
            for c in 0..2 {
                let d = ca[c] - cb[c];
                error += d * d;
                count += 1.0;
            }
        }
    }
    decibels(error / count.max(1.0))
}

fn xorshift(seed: &mut u32) -> u32 {
    *seed ^= *seed << 13;
    *seed ^= *seed >> 17;
    *seed ^= *seed << 5;
    *seed
}

fn decibels(mean_square: f64) -> f64 {
    10.0 * (255.0f64 * 255.0 / mean_square.max(1e-9)).log10()
}
