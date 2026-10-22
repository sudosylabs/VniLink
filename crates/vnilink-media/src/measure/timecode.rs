//! A strip of black and white cells carrying a 20-bit time, drawn by a
//! stimulus on screen and read back from a screen capture, for glass-to-glass
//! latency. It must survive H.264 at low bitrate and any scale, so cells are
//! large, the strip measures its own cell width, and every bit is sent again
//! inverted.
//!
//! Layout, one cell per entry, dark = 1, on a light background with at least
//! two light cells either side:
//!
//! | Cells | Content |
//! |---|---|
//! | 0–3 | 1 0 1 0, the cell width |
//! | 4–23 | the value, most significant bit first |
//! | 24–43 | the value inverted |
//! | 44–45 | 1 1, the strip's end |
//!
//! The stimulus in `test/latency/stimulus.ts` draws the same layout.

use crate::RawFrame;

/// Bits in a value. Values wrap at `1 << VALUE_BITS`.
pub const VALUE_BITS: u32 = 20;
/// Cells in one strip, quiet zones excluded.
pub const CELLS: usize = 46;

const HEADER: [bool; 4] = [true, false, true, false];

/// One strip found in a picture.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Timecode {
    pub value: u32,
    /// Left edge and the middle row of the strip, in picture pixels.
    pub x: u32,
    pub y: u32,
    /// Cell width in picture pixels.
    pub cell: f32,
    /// Rows that read the same value.
    pub rows: u32,
}

/// The cells of a strip for `value`, dark = `true`.
pub fn cells(value: u32) -> [bool; CELLS] {
    let value = value & ((1 << VALUE_BITS) - 1);
    let mut cells = [false; CELLS];
    cells[..4].copy_from_slice(&HEADER);
    for bit in 0..VALUE_BITS as usize {
        let set = value >> (VALUE_BITS as usize - 1 - bit) & 1 == 1;
        cells[4 + bit] = set;
        cells[24 + bit] = !set;
    }
    cells[44] = true;
    cells[45] = true;
    cells
}

/// Every strip in `frame` that at least `min_rows` rows read alike.
pub fn find(frame: &RawFrame, min_rows: u32) -> Vec<Timecode> {
    let mut found: Vec<Timecode> = Vec::new();
    let mut dark = Vec::with_capacity(usize::from(frame.width()));
    for y in (0..frame.height()).step_by(2) {
        dark.clear();
        dark.extend(frame.row(y).chunks_exact(4).map(|pixel| {
            // BGRA; Rec. 601 luma is enough to split black from white.
            let luma =
                114 * u32::from(pixel[0]) + 587 * u32::from(pixel[1]) + 299 * u32::from(pixel[2]);
            luma < 128 * 1000
        }));
        for (x, cell, value) in read_row(&dark) {
            match found.iter_mut().find(|code| {
                code.value == value
                    && (code.x as f32 - x as f32).abs() <= code.cell * 2.0
                    // The same strip a row or two further down.
                    && u32::from(y) <= code.y + 4
            }) {
                Some(code) => {
                    code.rows += 1;
                    code.y = code.y.max(u32::from(y));
                }
                None => found.push(Timecode {
                    value,
                    x: x as u32,
                    y: u32::from(y),
                    cell,
                    rows: 1,
                }),
            }
        }
    }
    // `y` tracked the last row; report the middle one.
    for code in &mut found {
        code.y = code.y.saturating_sub(code.rows);
    }
    found.retain(|code| code.rows >= min_rows);
    found
}

/// How much older `older` is than `newer`, in value units, across the wrap.
pub fn age(newer: u32, older: u32) -> u32 {
    newer.wrapping_sub(older) & ((1 << VALUE_BITS) - 1)
}

/// The strips in one row of dark/light samples, as (left edge, cell width, value).
fn read_row(dark: &[bool]) -> Vec<(usize, f32, u32)> {
    let runs = runs(dark);
    let mut out = Vec::new();
    for index in 1..runs.len().saturating_sub(4) {
        let (start, first, is_dark) = runs[index];
        // A header is a light quiet zone, then dark, light, dark of one cell each.
        if !is_dark || runs[index - 1].2 {
            continue;
        }
        let (second, third) = (runs[index + 1].1, runs[index + 2].1);
        let cell = (first + second + third) as f32 / 3.0;
        if cell < 2.0
            || [first, second, third]
                .iter()
                .any(|&run| (run as f32 - cell).abs() > cell * 0.4)
        {
            continue;
        }
        if (runs[index - 1].1 as f32) < cell {
            continue;
        }
        let Some(cell) = refine(dark, start, cell) else {
            continue;
        };
        let sample = |at: usize| -> Option<bool> {
            let x = start as f32 + (at as f32 + 0.5) * cell;
            dark.get(x as usize).copied()
        };
        let mut read = [false; CELLS];
        let mut complete = true;
        for (at, slot) in read.iter_mut().enumerate() {
            match sample(at) {
                Some(value) => *slot = value,
                None => complete = false,
            }
        }
        if !complete || read[..4] != HEADER || !read[44] || !read[45] {
            continue;
        }
        let mut value = 0u32;
        let mut valid = true;
        for bit in 0..VALUE_BITS as usize {
            valid &= read[4 + bit] != read[24 + bit];
            value = value << 1 | u32::from(read[4 + bit]);
        }
        if valid {
            out.push((start, cell, value));
        }
    }
    out
}

/// The cell width measured over the whole strip: from its left edge to where
/// the last dark cells give way to the quiet zone, about `CELLS` cells on.
fn refine(dark: &[bool], start: usize, estimate: f32) -> Option<f32> {
    let expected = start as f32 + CELLS as f32 * estimate;
    let window = estimate * 0.6 * CELLS as f32 / 10.0 + 2.0;
    let from = (expected - window).max(start as f32) as usize;
    let to = ((expected + window) as usize).min(dark.len());
    (from.max(1)..to)
        .filter(|&x| dark[x - 1] && !dark[x])
        .min_by(|&a, &b| {
            (a as f32 - expected)
                .abs()
                .total_cmp(&(b as f32 - expected).abs())
        })
        .map(|end| (end - start) as f32 / CELLS as f32)
}

/// Runs of equal samples as (start, length, dark).
fn runs(dark: &[bool]) -> Vec<(usize, usize, bool)> {
    let mut runs: Vec<(usize, usize, bool)> = Vec::new();
    for (x, &value) in dark.iter().enumerate() {
        match runs.last_mut() {
            Some(run) if run.2 == value => run.1 += 1,
            _ => runs.push((x, 1, value)),
        }
    }
    runs
}
