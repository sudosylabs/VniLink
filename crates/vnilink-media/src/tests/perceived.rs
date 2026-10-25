//! The perceived-quality page read back from a viewer's screen, scaled and
//! placed as a remote desktop application shows it.

use crate::measure::perceived::{
    card, crop, locate, read, read_region, Rect, Role, ANCHOR_BOTTOM_RIGHT, ANCHOR_TOP_LEFT,
};
use crate::measure::timecode::{cells, CELLS};
use crate::RawFrame;

const HOST: (u32, u32) = (1920, 1080);
const CELL: f64 = 12.0;

/// A viewer screen showing the host page at `scale` from (`left`, `top`):
/// the anchors and the column with `values` (frame, clicks, x, y, card).
fn viewer(width: u16, height: u16, scale: f64, left: f64, top: f64, values: [u32; 5]) -> RawFrame {
    let (w, h) = (usize::from(width), usize::from(height));
    let mut pixels = vec![200u8; w * h * 4];
    let page = (
        left,
        top,
        left + f64::from(HOST.0) * scale,
        top + f64::from(HOST.1) * scale,
    );
    // The page is white where the application shows it.
    for y in page.1 as usize..(page.3 as usize).min(h) {
        for x in page.0 as usize..(page.2 as usize).min(w) {
            pixels[(y * w + x) * 4..(y * w + x) * 4 + 3].fill(255);
        }
    }
    let mut strip = |value: u32, host_x: f64, host_y: f64| {
        let pattern = cells(value);
        let (x0, y0) = (left + host_x * scale, top + host_y * scale);
        let rows = (3.0 * CELL * scale) as usize;
        for y in y0 as usize..(y0 as usize + rows).min(h) {
            for x in 0..w {
                let at = (x as f64 + 0.5 - x0) / (CELL * scale) - 2.0;
                if at >= 0.0 && (at as usize) < CELLS && pattern[at as usize] {
                    pixels[(y * w + x) * 4..(y * w + x) * 4 + 3].fill(0);
                }
            }
        }
    };
    let column = [
        ANCHOR_TOP_LEFT,
        values[0],
        values[1],
        values[2],
        values[3],
        values[4],
        HOST.0,
        HOST.1,
        12,
        0x1234,
    ];
    for (index, value) in column.into_iter().enumerate() {
        strip(value, 2.0 * CELL, 2.0 * CELL + index as f64 * 5.0 * CELL);
    }
    strip(
        ANCHOR_BOTTOM_RIGHT,
        f64::from(HOST.0) - 52.0 * CELL,
        f64::from(HOST.1) - 5.0 * CELL,
    );
    RawFrame::bgra(width, height, 0, pixels).unwrap()
}

#[test]
fn the_mapping_and_the_column_come_back_from_an_enlarged_picture() {
    let frame = viewer(3400, 1960, 1.6, 120.0, 70.0, [1234, 7, 960, 540, 3]);
    let reading = read(&frame);
    assert_eq!(reading.value(Role::Frame), Some(1234));
    assert_eq!(reading.value(Role::Clicks), Some(7));
    assert_eq!(reading.value(Role::PointerX), Some(960));
    assert_eq!(reading.value(Role::PointerY), Some(540));
    assert_eq!(reading.value(Role::Card), Some(3));
    assert_eq!(reading.value(Role::Keys), Some(12));
    assert_eq!(reading.value(Role::Typed), Some(0x1234));
    let mapping = reading.mapping().expect("both anchors and the size");
    assert_eq!(
        (mapping.cell, mapping.width, mapping.height),
        (12, 1920, 1080)
    );
    assert!((mapping.scale_x - 1.6).abs() < 0.01, "{mapping:?}");
    assert!((mapping.scale_y - 1.6).abs() < 0.02, "{mapping:?}");
    // A host point lands within two viewer pixels of where it is drawn.
    let (x, y) = mapping.to_viewer(960.0, 540.0);
    assert!((x - (120.0 + 960.0 * 1.6)).abs() < 2.0, "{x}");
    assert!((y - (70.0 + 540.0 * 1.6)).abs() < 3.0, "{y}");
    let (hx, hy) = mapping.to_host(x, y);
    assert!((hx - 960.0).abs() < 0.01 && (hy - 540.0).abs() < 0.01);
    let shown = mapping.card();
    let expected = card(1920, 1080, 12);
    assert_eq!(
        expected,
        Rect {
            x: 672.0,
            y: 24.0,
            width: 1024.0,
            height: 768.0
        }
    );
    assert!((shown.x - (120.0 + 672.0 * 1.6)).abs() < 2.0, "{shown:?}");
}

#[test]
fn a_shrunk_picture_reads_and_the_column_region_alone_is_enough() {
    let frame = viewer(1600, 1000, 0.75, 40.0, 30.0, [99, 0, 10, 20, 1]);
    let mapping = read(&frame).mapping().expect("both anchors and the size");
    assert!((mapping.scale_x - 0.75).abs() < 0.01, "{mapping:?}");
    let region = read_region(&frame, mapping.column()).unwrap();
    assert_eq!(region.value(Role::Frame), Some(99));
    assert_eq!(region.value(Role::PointerY), Some(20));
    // The region holds the column, not the bottom-right anchor.
    assert!(region.bottom_right.is_none());
}

#[test]
fn no_mapping_without_both_anchors_and_a_crop_stays_inside_the_picture() {
    let frame = viewer(800, 600, 0.3, 0.0, 0.0, [1, 2, 3, 4, 5]);
    let mut reading = read(&frame);
    reading.bottom_right = None;
    assert!(reading.mapping().is_none());
    let piece = crop(
        &frame,
        Rect {
            x: 790.0,
            y: 590.0,
            width: 100.0,
            height: 100.0,
        },
    )
    .unwrap();
    assert_eq!((piece.width(), piece.height()), (10, 10));
}

#[test]
fn the_anchors_top_edges_place_the_picture_within_a_pixel() {
    for (scale, left, top) in [(1.0, 0.0, 0.0), (1.6, 120.0, 70.0), (0.75, 40.0, 31.0)] {
        let frame = viewer(3400, 1960, scale, left, top, [1, 2, 3, 4, 5]);
        let mapping = locate(&frame).expect("both anchors and the size");
        let (_, y) = mapping.to_viewer(960.0, 24.0);
        assert!(
            (y - (top + 24.0 * scale)).abs() <= 1.0,
            "{scale}: {y} {mapping:?}"
        );
        let (_, y) = mapping.to_viewer(960.0, 1000.0);
        assert!(
            (y - (top + 1000.0 * scale)).abs() <= 1.5,
            "{scale}: {y} {mapping:?}"
        );
    }
}
