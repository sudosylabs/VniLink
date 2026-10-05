//! Controller pointer coordinates mapped onto the host monitor.
//!
//! The controller draws the remote desktop at its own window size, which is
//! rarely the host's pixel size. This maps one absolute position between the
//! two rectangles with round-to-nearest and clamps to the host edges. DPI,
//! rotation, and extra monitors are still open; both rectangles are the one
//! negotiated monitor.

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
