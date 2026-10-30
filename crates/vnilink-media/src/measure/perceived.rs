// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! Perceived quality read from the viewing computer's own screen: the page
//! in `test/perceived/stimulus.ts`, shown full screen on the computer being
//! controlled, comes back through whichever remote desktop application is
//! under test. Two anchor strips at known corners give the mapping between
//! viewer and host pixels; a column of strips gives the host's frame
//! counter, click count, pointer position, test card generation and size,
//! and the keys that typed text with a hash of that text.
//! The application is a black box: nothing here asks it anything.
//!
//! The host's cell size and pixel density are not needed: the anchors'
//! distance, the decoded cell width and the host's own width solve both.

use super::timecode::{find, Timecode};
use crate::{Error, RawFrame};

/// Strip values that mark the two anchors.
pub const ANCHOR_TOP_LEFT: u32 = 0xa5a5a;
pub const ANCHOR_BOTTOM_RIGHT: u32 = 0x5a5a5;

/// The column below the top-left anchor, in order. The anchor is index 0.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Frame = 1,
    Clicks = 2,
    PointerX = 3,
    PointerY = 4,
    Card = 5,
    Width = 6,
    Height = 7,
    Keys = 8,
    Typed = 9,
}

const ROLES: [Role; 9] = [
    Role::Frame,
    Role::Clicks,
    Role::PointerX,
    Role::PointerY,
    Role::Card,
    Role::Width,
    Role::Height,
    Role::Keys,
    Role::Typed,
];

/// A rectangle in either computer's pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

/// The test card's place on a host page of `width` by `height` with strip
/// cells of `cell` pixels, as `perceivedLayout` in the stimulus lays it out.
pub fn card(width: u32, height: u32, cell: u32) -> Rect {
    let (width, height, cell) = (f64::from(width), f64::from(height), f64::from(cell));
    let margin = 2.0 * cell;
    let strip_width = 50.0 * cell;
    let bottom = height - margin - 3.0 * cell;
    let left = margin + strip_width + 4.0 * cell;
    let card_width = (((1024.0f64).min(width - left - margin) / 8.0).floor() * 8.0).max(64.0);
    let card_height =
        (((768.0f64).min(bottom - 2.0 * cell - margin) / 8.0).floor() * 8.0).max(64.0);
    Rect {
        x: left,
        y: margin,
        width: card_width,
        height: card_height,
    }
}

/// Viewer pixels for host pixels, and back: the picture as the
/// application shows it, scaled and placed.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Mapping {
    pub scale_x: f64,
    pub scale_y: f64,
    pub offset_x: f64,
    pub offset_y: f64,
    /// The host page's strip cell and size, in host pixels.
    pub cell: u32,
    pub width: u32,
    pub height: u32,
}

impl Mapping {
    pub fn to_viewer(&self, x: f64, y: f64) -> (f64, f64) {
        (
            self.offset_x + x * self.scale_x,
            self.offset_y + y * self.scale_y,
        )
    }

    pub fn to_host(&self, x: f64, y: f64) -> (f64, f64) {
        (
            (x - self.offset_x) / self.scale_x,
            (y - self.offset_y) / self.scale_y,
        )
    }

    pub fn viewer_rect(&self, rect: Rect) -> Rect {
        let (x, y) = self.to_viewer(rect.x, rect.y);
        Rect {
            x,
            y,
            width: rect.width * self.scale_x,
            height: rect.height * self.scale_y,
        }
    }

    /// The column of strips in viewer pixels, with a cell of slack around
    /// it: reading only this region keeps up with the display.
    pub fn column(&self) -> Rect {
        let cell = f64::from(self.cell);
        self.viewer_rect(Rect {
            x: cell,
            y: cell,
            width: 53.0 * cell,
            height: 52.0 * cell,
        })
    }

    pub fn card(&self) -> Rect {
        self.viewer_rect(card(self.width, self.height, self.cell))
    }
}

/// What one picture of the viewer's screen shows of the host page.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Reading {
    pub anchor: Option<Timecode>,
    pub bottom_right: Option<Timecode>,
    pub values: [Option<u32>; 10],
}

impl Reading {
    pub fn value(&self, role: Role) -> Option<u32> {
        self.values[role as usize]
    }

    /// The mapping, once both anchors and the host's size were read.
    pub fn mapping(&self) -> Option<Mapping> {
        let (a, b) = (self.anchor?, self.bottom_right?);
        let (width, height) = (self.value(Role::Width)?, self.value(Role::Height)?);
        if width < 640 || height < 360 || b.x <= a.x || b.y <= a.y {
            return None;
        }
        // Host cell 0 of the anchors is at 4c and width - 50c; middles at
        // 3.5c and height - 3.5c. With the viewer cell v = s c:
        // xb - xa = s (width - 54 c) = s width - 54 v.
        let viewer_cell = f64::from((a.cell + b.cell) / 2.0);
        let scale_x = (f64::from(b.x - a.x) + 54.0 * viewer_cell) / f64::from(width);
        let cell = (viewer_cell / scale_x).round().max(1.0);
        let scale_y = f64::from(b.y - a.y) / (f64::from(height) - 7.0 * cell);
        if !(scale_x > 0.0 && scale_y > 0.0) {
            return None;
        }
        Some(Mapping {
            scale_x,
            scale_y,
            offset_x: f64::from(a.x) - scale_x * 4.0 * cell,
            offset_y: f64::from(a.y) - scale_y * 3.5 * cell,
            cell: cell as u32,
            width,
            height,
        })
    }
}

/// Reads the anchors and the column anywhere in `frame`.
pub fn read(frame: &RawFrame) -> Reading {
    reading(&find(frame, 3))
}

/// The mapping from `frame`, its vertical placement taken from the anchors'
/// top edges rather than the rows the strips were read on: those place a
/// strip's middle only to within a couple of pixels.
pub fn locate(frame: &RawFrame) -> Option<Mapping> {
    let reading = read(frame);
    let mapping = reading.mapping()?;
    let (a, b) = (reading.anchor?, reading.bottom_right?);
    let cell = f64::from(mapping.cell);
    let (top_a, top_b) = (top_edge(frame, a)?, top_edge(frame, b)?);
    // Host tops: the column's first strip at 2c, the bottom anchor at
    // height - 5c.
    let scale_y = (top_b - top_a) / (f64::from(mapping.height) - 7.0 * cell);
    if scale_y.is_nan() || scale_y <= 0.0 {
        return Some(mapping);
    }
    Some(Mapping {
        scale_y,
        offset_y: top_a - scale_y * 2.0 * cell,
        ..mapping
    })
}

/// The first dark row of a strip's first cell, which is dark, searching
/// down from above the row it was read on; in picture pixels.
fn top_edge(frame: &RawFrame, code: Timecode) -> Option<f64> {
    let x = usize::try_from(code.x).ok()? + (code.cell / 2.0) as usize;
    if x >= usize::from(frame.width()) {
        return None;
    }
    let dark = |y: u32| {
        let pixel = &frame.row(y as u16)[x * 4..x * 4 + 3];
        114 * u32::from(pixel[0]) + 587 * u32::from(pixel[1]) + 299 * u32::from(pixel[2])
            < 128 * 1000
    };
    let from = code.y.saturating_sub((code.cell * 3.0) as u32);
    (from..=code.y.min(u32::from(frame.height()) - 1))
        .find(|&y| dark(y))
        .map(f64::from)
}

/// Reads only `region` of `frame`, in its pixels, for speed once the
/// column's place is known.
pub fn read_region(frame: &RawFrame, region: Rect) -> Result<Reading, Error> {
    let crop = crop(frame, region)?;
    let (left, top) = (region.x.max(0.0) as u32, region.y.max(0.0) as u32);
    let codes: Vec<Timecode> = find(&crop, 3)
        .into_iter()
        .map(|code| Timecode {
            x: code.x + left,
            y: code.y + top,
            ..code
        })
        .collect();
    Ok(reading(&codes))
}

/// `region` of `frame`, clamped to it, as its own picture.
pub fn crop(frame: &RawFrame, region: Rect) -> Result<RawFrame, Error> {
    let left = (region.x.max(0.0) as u16).min(frame.width().saturating_sub(1));
    let top = (region.y.max(0.0) as u16).min(frame.height().saturating_sub(1));
    let right = ((region.x + region.width).ceil().max(0.0) as u16).min(frame.width());
    let bottom = ((region.y + region.height).ceil().max(0.0) as u16).min(frame.height());
    let (width, height) = (
        right.saturating_sub(left).max(1),
        bottom.saturating_sub(top).max(1),
    );
    let mut pixels = Vec::with_capacity(usize::from(width) * usize::from(height) * 4);
    for y in top..top + height {
        let row = frame.row(y);
        let from = usize::from(left) * 4;
        pixels.extend_from_slice(&row[from..from + usize::from(width) * 4]);
    }
    RawFrame::bgra(width, height, frame.timestamp_us(), pixels)
        .map(|picture| picture.with_captured_at(frame.captured_at()))
}

fn reading(codes: &[Timecode]) -> Reading {
    let mut reading = Reading {
        anchor: codes
            .iter()
            .copied()
            .find(|code| code.value == ANCHOR_TOP_LEFT),
        bottom_right: codes
            .iter()
            .copied()
            .find(|code| code.value == ANCHOR_BOTTOM_RIGHT),
        values: [None; 10],
    };
    let Some(anchor) = reading.anchor else {
        return reading;
    };
    reading.values[0] = Some(anchor.value);
    // Strips are five cells apart; the column shares the anchor's left edge.
    let pitch = 5.0 * anchor.cell;
    for code in codes {
        if (code.x as f32 - anchor.x as f32).abs() > 2.0 * anchor.cell || code.y <= anchor.y {
            continue;
        }
        let index = ((code.y - anchor.y) as f32 / pitch).round() as usize;
        if let Some(role) = ROLES.iter().find(|role| **role as usize == index) {
            reading.values[*role as usize] = Some(code.value);
        }
    }
    reading
}
