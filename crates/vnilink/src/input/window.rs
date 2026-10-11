//! One focused host window. Following it is geometry, not a visual control.

use crate::error::Error;

use super::{MAX_DESKTOP_ORIGIN, MAX_DISPLAY_EDGE};

/// Bounds of one host window in desktop pixels. The id is a platform handle
/// and is not a display id. No title or process name is stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WindowGeometry {
    pub id: u64,
    pub x: i32,
    pub y: i32,
    pub width: u16,
    pub height: u16,
    pub focused: bool,
}

pub(crate) fn validate_window(window: WindowGeometry) -> Result<(), Error> {
    if window.id == 0 {
        return Err(Error::InvalidSetting("a window id is invalid"));
    }
    if window.width == 0
        || window.height == 0
        || window.width > MAX_DISPLAY_EDGE
        || window.height > MAX_DISPLAY_EDGE
    {
        return Err(Error::InvalidSetting("a window size is out of range"));
    }
    let bound = u32::try_from(MAX_DESKTOP_ORIGIN).unwrap_or(u32::MAX);
    if window.x.unsigned_abs() > bound || window.y.unsigned_abs() > bound {
        return Err(Error::InvalidSetting("a window origin is out of range"));
    }
    Ok(())
}
