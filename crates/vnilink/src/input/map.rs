// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! Controller pointer coordinates mapped onto the host desktop.
//!
//! The controller draws the remote desktop at its own window size, which is
//! rarely the host's pixel size. [`map_point`] maps one absolute position
//! between two rectangles with round-to-nearest and clamps to the host edges.
//! The topology helpers below extend that to multi-display desktops: stable
//! display ids, desktop origins that may be negative, per-display dimensions
//! and a topology revision that fences stale geometry. Width and height are
//! post-rotation physical pixels; rotation and scale travel as presentation
//! metadata for the viewer/cursor and do not change the hit-test.

use crate::error::Error;

/// One display in a desktop topology. Ids are stable within a revision;
/// a display that disappears never reuses its id in the same revision.
/// Origins are desktop pixels and may be negative. All entries in one
/// snapshot share the same revision.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DisplayInfo {
    pub id: u64,
    pub origin_x: i32,
    pub origin_y: i32,
    pub width: u16,
    pub height: u16,
    /// Physical orientation in degrees: one of 0, 90, 180, 270.
    pub rotation_deg: u16,
    /// Rendering scale in percent (100 is 1:1); validated and carried for
    /// the viewer, not applied to input coordinates here.
    pub scale_percent: u16,
    pub revision: u64,
}

/// Maximum displays in one topology snapshot.
pub const MAX_DISPLAYS: usize = 16;
/// Largest single display dimension in pixels.
pub const MAX_DISPLAY_EDGE: u16 = 16384;
/// Bounds for a desktop origin in pixels.
pub const MAX_DESKTOP_ORIGIN: i32 = 32768;

/// Maps `(x, y)` from a `from` rectangle onto a `to` rectangle.
///
/// A zero source side has no scale to read, so the point maps to the origin.
/// A zero target side clamps everything to the origin.
pub fn map_point(x: u16, y: u16, from: (u16, u16), to: (u16, u16)) -> (u16, u16) {
    (map_axis(x, from.0, to.0), map_axis(y, from.1, to.1))
}

fn map_axis(value: u16, from: u16, to: u16) -> u16 {
    if from == 0 || to == 0 {
        return 0;
    }
    let scaled = (u32::from(value) * u32::from(to) + u32::from(from) / 2) / u32::from(from);
    scaled.min(u32::from(to).saturating_sub(1)) as u16
}

/// Checks one topology snapshot and returns its revision.
///
/// Rejects empty snapshots, duplicate or zero ids, zero or oversized
/// dimensions, out-of-range origins, unknown rotations, out-of-range scales
/// and mixed revisions. A monitor index alone is not accepted as an id:
/// callers must supply the stable source id.
pub fn validate_topology(displays: &[DisplayInfo]) -> Result<u64, Error> {
    if displays.is_empty() || displays.len() > MAX_DISPLAYS {
        return Err(Error::InvalidSetting(
            "the display topology is out of range",
        ));
    }
    let revision = displays[0].revision;
    let mut seen = [0u64; MAX_DISPLAYS];
    for (index, display) in displays.iter().enumerate() {
        if display.id == 0 || seen[..index].contains(&display.id) {
            return Err(Error::InvalidSetting("a display id is invalid"));
        }
        seen[index] = display.id;
        if display.width == 0
            || display.height == 0
            || display.width > MAX_DISPLAY_EDGE
            || display.height > MAX_DISPLAY_EDGE
        {
            return Err(Error::InvalidSetting("a display size is out of range"));
        }
        // unsigned_abs: i32::MIN has no positive i32 form, so signed abs
        // panics in debug builds and stays negative (passing the check) in
        // release builds. Compare magnitudes as unsigned instead.
        if display.origin_x.unsigned_abs() > MAX_DESKTOP_ORIGIN as u32
            || display.origin_y.unsigned_abs() > MAX_DESKTOP_ORIGIN as u32
        {
            return Err(Error::InvalidSetting("a display origin is out of range"));
        }
        if !matches!(display.rotation_deg, 0 | 90 | 180 | 270) {
            return Err(Error::InvalidSetting("a display rotation is not supported"));
        }
        if display.scale_percent < 25 || display.scale_percent > 800 {
            return Err(Error::InvalidSetting("a display scale is out of range"));
        }
        if display.revision != revision {
            return Err(Error::InvalidSetting("a display revision does not match"));
        }
    }
    Ok(revision)
}

/// Bounding box of a validated topology as `(min_x, min_y, width, height)`.
/// Width and height are at least 1. Callers must validate first; invalid
/// input returns `None` instead of panicking on overflow.
pub fn desktop_bounds(displays: &[DisplayInfo]) -> Option<(i32, i32, u32, u32)> {
    if validate_topology(displays).is_err() {
        return None;
    }
    let mut min_x = i32::MAX;
    let mut min_y = i32::MAX;
    let mut max_x = i32::MIN;
    let mut max_y = i32::MIN;
    for display in displays {
        min_x = min_x.min(display.origin_x);
        min_y = min_y.min(display.origin_y);
        max_x = max_x.max(display.origin_x.saturating_add(i32::from(display.width)));
        max_y = max_y.max(display.origin_y.saturating_add(i32::from(display.height)));
    }
    let width = u32::try_from(max_x - min_x).ok()?;
    let height = u32::try_from(max_y - min_y).ok()?;
    if width == 0 || height == 0 {
        return None;
    }
    Some((min_x, min_y, width, height))
}

/// Display containing a desktop point, or `None` when the point falls in a
/// gap or outside the topology. Bounds are exclusive on the far edges.
pub fn display_for_point(x: i32, y: i32, displays: &[DisplayInfo]) -> Option<DisplayInfo> {
    if validate_topology(displays).is_err() {
        return None;
    }
    displays.iter().find_map(|display| {
        let local_x = x.checked_sub(display.origin_x)?;
        let local_y = y.checked_sub(display.origin_y)?;
        if local_x >= 0
            && local_y >= 0
            && u32::try_from(local_x).ok()? < u32::from(display.width)
            && u32::try_from(local_y).ok()? < u32::from(display.height)
        {
            Some(*display)
        } else {
            None
        }
    })
}

/// Maps a controller window point onto the desktop and returns the desktop
/// point plus the owning display id.
///
/// `expected_revision` fences outdated geometry: a mismatch is `StaleSession`.
/// Invalid topologies, zero window sides and points outside every display
/// are `InvalidSetting`.
pub fn map_window_to_desktop(
    x: u16,
    y: u16,
    window: (u16, u16),
    displays: &[DisplayInfo],
    expected_revision: u64,
) -> Result<(i32, i32, u64), Error> {
    let revision = validate_topology(displays)
        .map_err(|_| Error::InvalidSetting("the display topology is out of range"))?;
    if revision != expected_revision {
        return Err(Error::StaleSession);
    }
    let (min_x, min_y, width, height) = desktop_bounds(displays).ok_or(Error::InvalidSetting(
        "the display topology is out of range",
    ))?;
    if window.0 == 0 || window.1 == 0 {
        return Err(Error::InvalidSetting(
            "the controller window size is out of range",
        ));
    }
    let desktop_x = min_x.saturating_add(map_range_to_len(x, window.0, width) as i32);
    let desktop_y = min_y.saturating_add(map_range_to_len(y, window.1, height) as i32);
    let display = display_for_point(desktop_x, desktop_y, displays).ok_or(
        Error::InvalidSetting("the mapped point is outside every display"),
    )?;
    Ok((desktop_x, desktop_y, display.id))
}

fn map_range_to_len(value: u16, from: u16, len: u32) -> u32 {
    debug_assert!(from != 0 && len != 0);
    let scaled = (u64::from(value) * u64::from(len) + u64::from(from) / 2) / u64::from(from);
    scaled.min(u64::from(len).saturating_sub(1)) as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pointer_positions_scale_between_window_and_monitor() {
        // Identity and origin.
        assert_eq!(map_point(0, 0, (1920, 1080), (1920, 1080)), (0, 0));
        assert_eq!(
            map_point(1919, 1079, (1920, 1080), (1920, 1080)),
            (1919, 1079)
        );
        // Downscale by exactly two rounds to the nearest host pixel.
        assert_eq!(map_point(0, 0, (1920, 1080), (960, 540)), (0, 0));
        assert_eq!(map_point(960, 540, (1920, 1080), (960, 540)), (480, 270));
        assert_eq!(map_point(1, 1, (1920, 1080), (960, 540)), (1, 1));
        // Upscale and the far edge clamps onto the host instead of past it.
        assert_eq!(
            map_point(960, 540, (960, 540), (1920, 1080)),
            (1920 - 1, 1080 - 1)
        );
        assert_eq!(
            map_point(u16::MAX, u16::MAX, (960, 540), (1920, 1080)),
            (1919, 1079)
        );
        // A zero side has no scale to read or no pixels to name; axes are
        // independent, so only the zero side collapses.
        assert_eq!(map_point(50, 60, (0, 1080), (1920, 1080)), (0, 60));
        assert_eq!(map_point(50, 60, (1920, 1080), (0, 0)), (0, 0));
    }

    #[test]
    fn topology_ids_origins_and_revisions_are_validated() {
        use crate::input::{DisplayInfo, MAX_DESKTOP_ORIGIN, MAX_DISPLAY_EDGE};
        let valid = |display: DisplayInfo| display;
        let base = DisplayInfo {
            id: 7,
            origin_x: 0,
            origin_y: 0,
            width: 1920,
            height: 1080,
            rotation_deg: 0,
            scale_percent: 100,
            revision: 3,
        };
        assert_eq!(crate::input::validate_topology(&[base]).unwrap(), 3);
        // Empty and oversized snapshots are refused.
        assert!(crate::input::validate_topology(&[]).is_err());
        assert!(crate::input::validate_topology(&vec![base; 17]).is_err());
        // Zero and duplicate ids are refused; a monitor index alone is not an id.
        assert!(crate::input::validate_topology(&[valid(DisplayInfo { id: 0, ..base })]).is_err());
        assert!(crate::input::validate_topology(&[base, base]).is_err());
        // Zero and oversized dimensions are refused.
        assert!(
            crate::input::validate_topology(&[valid(DisplayInfo { width: 0, ..base })]).is_err()
        );
        assert!(crate::input::validate_topology(&[valid(DisplayInfo {
            width: MAX_DISPLAY_EDGE + 1,
            ..base
        })])
        .is_err());
        // Origins are bounded; unknown rotations and scales are refused.
        assert!(crate::input::validate_topology(&[valid(DisplayInfo {
            origin_x: MAX_DESKTOP_ORIGIN + 1,
            ..base
        })])
        .is_err());
        assert!(crate::input::validate_topology(&[valid(DisplayInfo {
            rotation_deg: 45,
            ..base
        })])
        .is_err());
        assert!(crate::input::validate_topology(&[valid(DisplayInfo {
            scale_percent: 24,
            ..base
        })])
        .is_err());
        assert!(crate::input::validate_topology(&[valid(DisplayInfo {
            scale_percent: 801,
            ..base
        })])
        .is_err());
        // Mixed revisions are refused.
        assert!(crate::input::validate_topology(&[
            base,
            DisplayInfo {
                id: 8,
                revision: 4,
                ..base
            }
        ])
        .is_err());
    }

    #[test]
    fn topology_lookup_and_window_mapping_cover_negative_origins() {
        use crate::input::{desktop_bounds, display_for_point, map_window_to_desktop, DisplayInfo};
        let left = DisplayInfo {
            id: 11,
            origin_x: -1280,
            origin_y: 0,
            width: 1280,
            height: 1024,
            rotation_deg: 0,
            scale_percent: 100,
            revision: 9,
        };
        let right = DisplayInfo {
            id: 12,
            origin_x: 0,
            origin_y: -200,
            width: 1920,
            height: 1080,
            rotation_deg: 90,
            scale_percent: 200,
            revision: 9,
        };
        let topology = [left, right];
        assert_eq!(desktop_bounds(&topology), Some((-1280, -200, 3200, 1224)));
        // Points land on the intended display; the gap between them is empty.
        assert_eq!(display_for_point(-1280, 0, &topology), Some(left));
        assert_eq!(display_for_point(-1, 1023, &topology), Some(left));
        assert_eq!(display_for_point(0, -200, &topology), Some(right));
        assert_eq!(display_for_point(1919, 879, &topology), Some(right));
        assert_eq!(display_for_point(-1, 1024, &topology), None);
        assert_eq!(display_for_point(0, -201, &topology), None);
        assert_eq!(display_for_point(1920, 0, &topology), None);
        // Far edges are exclusive.
        assert_eq!(display_for_point(0, 880, &topology), None);

        // Window corners map onto the matching display through the negative origin.
        // The window matches the desktop size here, so mapping is a direct offset.
        let window = (3200, 1224);
        assert_eq!(
            map_window_to_desktop(0, 200, window, &topology, 9).unwrap(),
            (-1280, 0, 11)
        );
        let (x, y, id) = map_window_to_desktop(639, 640, window, &topology, 9).unwrap();
        assert_eq!((x, y, id), (-641, 440, 11));
        let (x, y, id) = map_window_to_desktop(1280, 0, window, &topology, 9).unwrap();
        assert_eq!((x, y, id), (0, -200, 12));
        let (x, y, id) = map_window_to_desktop(3199, 1079, window, &topology, 9).unwrap();
        assert_eq!((x, y, id), (1919, 879, 12));
        // The far edge is exclusive: the bottom row of the window maps past the
        // right display.
        assert!(map_window_to_desktop(1280, 1080, window, &topology, 9).is_err());
        // Outdated geometry is stale; zero window sides and bad topologies fail.
        assert!(matches!(
            map_window_to_desktop(0, 0, window, &topology, 8),
            Err(Error::StaleSession)
        ));
        assert!(map_window_to_desktop(0, 0, (0, 1280), &topology, 9).is_err());
        assert!(map_window_to_desktop(0, 0, window, &[], 9).is_err());
    }

    #[test]
    fn signed_origin_extremes_are_refused_on_both_axes() {
        for origin in [i32::MIN, i32::MAX, -32769, 32769] {
            assert!(
                crate::input::validate_topology(&[screen(1, origin, 0, 1920, 1080, 0, 100)])
                    .is_err(),
                "origin_x {origin} accepted"
            );
            assert!(
                crate::input::validate_topology(&[screen(1, 0, origin, 1920, 1080, 0, 100)])
                    .is_err(),
                "origin_y {origin} accepted"
            );
        }
        for origin in [-32768, 32768, 0] {
            assert!(
                crate::input::validate_topology(&[screen(1, origin, 0, 1920, 1080, 0, 100)])
                    .is_ok(),
                "origin_x {origin} refused"
            );
            assert!(
                crate::input::validate_topology(&[screen(1, 0, origin, 1920, 1080, 0, 100)])
                    .is_ok(),
                "origin_y {origin} refused"
            );
        }
    }
    fn screen(
        id: u64,
        x: i32,
        y: i32,
        width: u16,
        height: u16,
        rotation: u16,
        scale: u16,
    ) -> DisplayInfo {
        DisplayInfo {
            id,
            origin_x: x,
            origin_y: y,
            width,
            height,
            rotation_deg: rotation,
            scale_percent: scale,
            revision: 1,
        }
    }
}
