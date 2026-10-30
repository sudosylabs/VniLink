// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! Viewer pointer mapping, shared by the native viewers.
//!
//! A stamped frame names its exact live source: the topology entry must
//! match the frame's display id AND revision, or the pointer is fenced.
//! Relabeling a stale crop with fresh geometry would submit input for
//! the wrong pixels, so revision mismatches (and unknown revisions)
//! send nothing until matching pixels arrive.

/// Map a point in an aspect-fit picture to the displayed source's
/// logical extent. `frame` is the decoded picture size (which quality
/// scaling may shrink below the source); the picture fraction maps onto
/// `source`, never onto the initial handshake monitor. Letterbox space
/// is not part of the remote desktop.
pub fn pointer(
    x: f64,
    y: f64,
    view: (f64, f64),
    frame: (u16, u16),
    source: (u16, u16),
) -> Option<(u16, u16)> {
    if frame.0 == 0 || frame.1 == 0 || view.0 <= 0.0 || view.1 <= 0.0 {
        return None;
    }
    let scale = (view.0 / f64::from(frame.0)).min(view.1 / f64::from(frame.1));
    let width = f64::from(frame.0) * scale;
    let height = f64::from(frame.1) * scale;
    let left = (view.0 - width) / 2.0;
    let top = (view.1 - height) / 2.0;
    if x < left || y < top || x >= left + width || y >= top + height {
        return None;
    }
    let source = if source.0 == 0 || source.1 == 0 {
        frame
    } else {
        source
    };
    Some((
        ((x - left) / width * f64::from(source.0)) as u16,
        ((y - top) / height * f64::from(source.1)) as u16,
    ))
}

/// Local picture pixels on one display, as desktop coordinates.
/// A point on the far edge is outside that display.
pub fn place_on_display(x: u16, y: u16, display: vnilink::DisplayInfo) -> Option<(i32, i32)> {
    if u32::from(x) >= u32::from(display.width) || u32::from(y) >= u32::from(display.height) {
        return None;
    }
    Some((
        display.origin_x.checked_add(i32::from(x))?,
        display.origin_y.checked_add(i32::from(y))?,
    ))
}

/// The display this picture belongs to.
/// One display is that display. Several displays need a preferred id that is
/// still in the snapshot. A missing snapshot or an unmatched set sends nothing.
pub fn display_for_picture(
    displays: &[vnilink::DisplayInfo],
    preferred: Option<u64>,
) -> Option<vnilink::DisplayInfo> {
    if let Some(id) = preferred {
        if let Some(display) = displays.iter().copied().find(|display| display.id == id) {
            return Some(display);
        }
    }
    if displays.len() == 1 {
        Some(displays[0])
    } else {
        None
    }
}

/// The display a stamped frame belongs to. Binds the frame's carried
/// (source id, revision) against the live topology entry, so twin
/// same-sized displays stay distinct and a stale held picture cannot be
/// relabeled with fresh geometry. A zero (unstamped) id, an unknown id,
/// or any revision mismatch selects nothing: guessing would send pointer
/// input for the wrong pixels. Mapping always uses the live entry's
/// geometry, never the frame's capture-time size.
pub fn display_for_frame_id(
    displays: &[vnilink::DisplayInfo],
    display_id: u64,
    revision: u64,
) -> Option<vnilink::DisplayInfo> {
    if display_id == 0 || revision == 0 {
        return None;
    }
    displays.iter().copied().find(|display| {
        display.id == display_id && display.revision == revision && display.revision > 0
    })
}

/// The display a captured frame belongs to. The host streams exactly the
/// display chosen at share time and reports its size in the accept
/// handshake; the source is the unique topology display matching that
/// geometry. Ambiguous (twin) or missing matches select nothing: guessing
/// would send pointer input to the wrong display.
pub fn display_for_frame(
    displays: &[vnilink::DisplayInfo],
    frame: (u16, u16),
) -> Option<vnilink::DisplayInfo> {
    let mut matches = displays.iter().copied().filter(|display| {
        display.revision > 0 && display.width == frame.0 && display.height == frame.1
    });
    let first = matches.next()?;
    if matches.next().is_some() {
        return None;
    }
    Some(first)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fitted_pointer_excludes_bars_and_scales_to_source_extent() {
        assert_eq!(
            pointer(0., 0., (1000., 1000.), (1920, 1080), (3840, 2160)),
            None
        );
        assert_eq!(
            pointer(500., 500., (1000., 1000.), (1920, 1080), (3840, 2160)),
            Some((1920, 1080))
        );
        assert_eq!(
            pointer(999., 780., (1000., 1000.), (1920, 1080), (3840, 2160)),
            Some((3836, 2155))
        );
        assert_eq!(pointer(500., 500., (1000., 1000.), (0, 0), (0, 0)), None);
    }

    /// SPEC17: the picture fraction maps onto the displayed source's
    /// logical extent, never onto the initial handshake monitor.
    /// Initial 1920x1080, secondary 1280x720 at x=1920: a center click
    /// on the secondary picture lands at desktop (2560, 360).
    #[test]
    fn secondary_source_maps_with_its_own_extent() {
        let secondary = display(2, 1920, 1280, 720);
        let map = |x: f64, y: f64| {
            pointer(x, y, (1280., 720.), (1280, 720), (1280, 720))
                .and_then(|(x, y)| place_on_display(x, y, secondary))
        };
        assert_eq!(map(640., 360.), Some((2560, 360)));
        assert_eq!(map(0., 0.), Some((1920, 0)));
        // The old handshake scale put the right third past the source
        // width and fenced it; the source extent keeps it inside.
        assert_eq!(map(1200., 360.), Some((3120, 360)));
        assert_eq!(map(1279., 719.), Some((3199, 719)));
        // The far edge is outside the picture.
        assert_eq!(map(1280., 360.), None);
        assert_eq!(map(640., 720.), None);
    }

    /// SPEC17: a quality-scaled decode still addresses source pixels:
    /// the fraction is scale-free, the extent is the source.
    #[test]
    fn quality_scaled_frame_maps_to_source_pixels() {
        let secondary = display(2, 1920, 1280, 720);
        let map = |x: f64, y: f64| {
            pointer(x, y, (640., 360.), (640, 360), (1280, 720))
                .and_then(|(x, y)| place_on_display(x, y, secondary))
        };
        assert_eq!(map(320., 180.), Some((2560, 360)));
        assert_eq!(map(0., 0.), Some((1920, 0)));
        assert_eq!(map(639., 359.), Some((3198, 718)));
    }

    /// SPEC17: negative origins and letterboxed views compose with
    /// the source extent without shifting the desktop point.
    #[test]
    fn negative_origin_and_letterbox_keep_the_desktop_point() {
        let left = display(1, -1280, 1280, 720);
        let map = |x: f64, y: f64| {
            pointer(x, y, (1280., 720.), (1280, 720), (1280, 720))
                .and_then(|(x, y)| place_on_display(x, y, left))
        };
        assert_eq!(map(640., 360.), Some((-640, 360)));
        assert_eq!(map(0., 0.), Some((-1280, 0)));
        // Square window around a wide picture: bars above and below.
        let barred = |x: f64, y: f64| {
            pointer(x, y, (1000., 1000.), (1280, 720), (1280, 720))
                .and_then(|(x, y)| place_on_display(x, y, left))
        };
        assert_eq!(barred(500., 500.), Some((-640, 360)));
        assert_eq!(barred(500., 100.), None);
        assert_eq!(barred(500., 950.), None);
    }

    fn display(id: u64, origin_x: i32, width: u16, height: u16) -> vnilink::DisplayInfo {
        vnilink::DisplayInfo {
            id,
            origin_x,
            origin_y: 0,
            width,
            height,
            rotation_deg: 0,
            scale_percent: 100,
            revision: 3,
        }
    }

    #[test]
    fn a_picture_lands_on_one_display_and_not_on_an_unmatched_set() {
        let only = display(7, -1920, 1920, 1080);
        assert_eq!(display_for_picture(&[only], None).unwrap().id, 7);
        assert_eq!(place_on_display(3, 4, only), Some((-1917, 4)));
        assert_eq!(place_on_display(1920, 0, only), None);
        let right = display(8, 0, 1280, 800);
        assert!(display_for_picture(&[only, right], None).is_none());
        assert_eq!(display_for_picture(&[only, right], Some(8)).unwrap().id, 8);
        assert!(display_for_picture(&[only, right], Some(9)).is_none());
        assert!(display_for_picture(&[], None).is_none());
    }

    #[test]
    fn a_frame_binds_to_its_unique_source_never_the_first_item() {
        // The captured display is second: geometry must win over order.
        let left = display(1, 0, 1280, 720);
        let right = display(2, 1280, 1920, 1080);
        assert_eq!(
            display_for_frame(&[left, right], (1920, 1080)).unwrap().id,
            2
        );
        assert_eq!(
            display_for_frame(&[left, right], (1280, 720)).unwrap().id,
            1
        );
        // Twins are ambiguous: guessing would misdirect the pointer.
        let twin = display(3, 3200, 1920, 1080);
        assert!(display_for_frame(&[left, right, twin], (1920, 1080)).is_none());
        // No match, no source.
        assert!(display_for_frame(&[left, right], (800, 600)).is_none());
        assert!(display_for_frame(&[], (1920, 1080)).is_none());
        // Uninitialized entries never bind.
        let mut stale = display(4, 0, 1920, 1080);
        stale.revision = 0;
        assert!(display_for_frame(&[stale], (1920, 1080)).is_none());
    }

    #[test]
    fn a_stamped_frame_binds_its_source_even_for_twins() {
        let left = display(1, 0, 1920, 1080);
        let right = display(2, 1920, 1920, 1080);
        // Same size, distinct ids: the stamp disambiguates what geometry
        // cannot.
        assert_eq!(display_for_frame_id(&[left, right], 2, 3).unwrap().id, 2);
        assert_eq!(
            display_for_frame_id(&[left, right], 2, 3).unwrap().origin_x,
            1920
        );
        assert_eq!(display_for_frame_id(&[left, right], 1, 3).unwrap().id, 1);
        // Order never wins over the stamp.
        assert_eq!(display_for_frame_id(&[right, left], 1, 3).unwrap().id, 1);
        // Zero, unknown and uninitialized sources fence the pointer.
        assert!(display_for_frame_id(&[left, right], 0, 3).is_none());
        assert!(display_for_frame_id(&[left, right], 9, 3).is_none());
        assert!(display_for_frame_id(&[], 1, 3).is_none());
        let mut stale = display(4, 0, 1920, 1080);
        stale.revision = 0;
        assert!(display_for_frame_id(&[stale], 4, 0).is_none());
    }

    /// SPEC15: a same-id frame whose revision moved on is a stale held
    /// picture: it must fence, never relabel with fresh geometry.
    #[test]
    fn revision_mismatch_fences_the_stale_held_picture() {
        let left = display(1, 0, 1920, 1080);
        let mut moved = display(1, 0, 1920, 1080);
        moved.revision = 9;
        // Same id, older pixels: fenced.
        assert!(display_for_frame_id(&[moved], 1, 3).is_none());
        // Same id, newer pixels than the known topology: fenced.
        assert!(display_for_frame_id(&[left], 1, 9).is_none());
        // Exact match maps to the live entry.
        assert_eq!(display_for_frame_id(&[moved], 1, 9).unwrap().revision, 9);
        // Unknown frame revisions never map, even for a live id.
        assert!(display_for_frame_id(&[moved], 1, 0).is_none());
    }
}
