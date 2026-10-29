//! Which display server this session's screen belongs to.

/// The screen belongs to a Wayland compositor. `DISPLAY` is often set too,
/// naming XWayland, whose X11 root window shows none of the desktop: only
/// the compositor's portals read the screen.
pub(crate) fn is_wayland() -> bool {
    std::env::var_os("WAYLAND_DISPLAY").is_some()
        || std::env::var("XDG_SESSION_TYPE").is_ok_and(|kind| kind == "wayland")
}

/// The screen belongs to an X11 server, read and driven through `DISPLAY`.
pub(crate) fn is_x11() -> bool {
    !is_wayland() && std::env::var_os("DISPLAY").is_some()
}
