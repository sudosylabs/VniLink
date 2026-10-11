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
