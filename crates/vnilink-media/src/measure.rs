//! Pictures and comparisons for encoder measurements on each platform, so
//! VideoToolbox and Media Foundation are measured on the same screen.

use crate::RawFrame;

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
