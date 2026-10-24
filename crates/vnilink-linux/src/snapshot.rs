//! One frame for a remote screenshot.
//!
//! An X11 `DISPLAY` is read directly. Without one, selection does not open a
//! portal. The explicit Choose action does. A live portal or X11 desktop that
//! has attested a picture in the last two seconds is used instead of a second capture.

use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Instant;

use vnilink::snapshot::{
    classify_stop, encode_bgra, unix_ms, watch_cancel, HostSnapshotFactory, ScreenImage,
    SnapshotFailure,
};

use crate::x11::{X11Capture, X11Plan};

/// How this computer can produce a screenshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinuxPlan {
    X11,
    Portal,
    NeedsSourceSelection,
}

/// An X11 session reads its root window. Elsewhere, Wayland included, only
/// the ScreenCast portal reads the screen: with a kept grant it shares the
/// same screen without asking, and without one it asks someone there, so a
/// plain refresh asks for a source to be chosen first.
pub fn linux_plan(x11: bool, granted: bool, prompt: bool) -> LinuxPlan {
    if x11 {
        LinuxPlan::X11
    } else if granted || prompt {
        LinuxPlan::Portal
    } else {
        LinuxPlan::NeedsSourceSelection
    }
}

/// Screenshots of this computer's screen. `grant` is where the portal's
/// consent is kept ([`crate::ScreenGrant`]), as RustDesk keeps its own:
/// once someone here has chosen the screen, later screenshots need no one.
/// Revoked in the system's privacy settings, the next one asks again.
pub struct LinuxSnapshot {
    grant: Option<std::path::PathBuf>,
}

impl LinuxSnapshot {
    pub fn new(grant: Option<std::path::PathBuf>) -> Self {
        Self { grant }
    }
}

impl HostSnapshotFactory for LinuxSnapshot {
    fn capture(
        &self,
        deadline: Instant,
        cancel: &Arc<AtomicBool>,
        prompt: bool,
    ) -> Result<ScreenImage, SnapshotFailure> {
        watch_cancel(deadline, cancel)?;
        let grant = self.grant.as_ref().map(crate::ScreenGrant::at);
        let granted = grant.as_ref().and_then(crate::ScreenGrant::token).is_some();
        match linux_plan(crate::session::is_x11(), granted, prompt) {
            LinuxPlan::NeedsSourceSelection => Err(SnapshotFailure::NeedsSourceSelection),
            LinuxPlan::X11 => capture_x11(deadline, cancel),
            LinuxPlan::Portal => capture_portal(deadline, cancel, grant.as_ref()),
        }
    }
}

fn capture_x11(deadline: Instant, cancel: &AtomicBool) -> Result<ScreenImage, SnapshotFailure> {
    watch_cancel(deadline, cancel)?;
    let capture = X11Capture::open(&X11Plan::current_session())
        .map_err(|_| classify_stop(cancel, deadline))?;
    watch_cancel(deadline, cancel)?;
    let frame = capture
        .next_frame(0)
        .map_err(|_| classify_stop(cancel, deadline))?;
    watch_cancel(deadline, cancel)?;
    encode_bgra(
        frame.width(),
        frame.height(),
        frame.stride(),
        frame.pixels(),
        unix_ms(),
        "root",
    )
}

fn capture_portal(
    deadline: Instant,
    cancel: &Arc<AtomicBool>,
    grant: Option<&crate::ScreenGrant>,
) -> Result<ScreenImage, SnapshotFailure> {
    #[cfg(target_os = "linux")]
    {
        watch_cancel(deadline, cancel)?;
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|_| SnapshotFailure::Unavailable)?;
        let mut session = crate::portal::PortalSession::new();
        let remote = runtime
            // Answered after the viewer has given up, the dialog's grant is
            // still kept, so the viewer's next refresh shows the screen.
            .block_on(crate::dbus::open_monitor(&mut session, grant))
            .map_err(|error| match error {
                // Someone there declined the dialog: permission, not a source.
                vnilink_media::Error::CaptureDenied(_) => SnapshotFailure::NeedsPermission,
                _ => SnapshotFailure::NeedsSourceSelection,
            })?;
        let node = session
            .pipewire_node()
            .ok_or(SnapshotFailure::NeedsSourceSelection)?;
        watch_cancel(deadline, cancel)?;
        let startup = vnilink::BackendStartup::with_stop(
            Arc::clone(cancel),
            deadline.saturating_duration_since(Instant::now()),
        );
        let capture = crate::pipewire::PortalCapture::open(remote, node, startup)
            .map_err(|_| classify_stop(cancel, deadline))?;
        let mut stamp = 1u64;
        loop {
            watch_cancel(deadline, cancel)?;
            match capture.poll_frame(stamp) {
                Ok(Some(frame)) => {
                    return encode_bgra(
                        frame.width(),
                        frame.height(),
                        frame.stride(),
                        frame.pixels(),
                        unix_ms(),
                        "portal",
                    );
                }
                Ok(None) => {
                    stamp = stamp.saturating_add(1);
                    std::thread::sleep(std::time::Duration::from_millis(20));
                }
                Err(_) => return Err(classify_stop(cancel, deadline)),
            }
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (deadline, cancel, grant);
        Err(SnapshotFailure::NeedsSourceSelection)
    }
}

#[cfg(test)]
mod tests {
    use super::{linux_plan, LinuxPlan};

    #[test]
    fn selection_does_not_open_a_portal_and_choose_does() {
        assert_eq!(linux_plan(true, false, false), LinuxPlan::X11);
        assert_eq!(linux_plan(true, true, true), LinuxPlan::X11);
        assert_eq!(
            linux_plan(false, false, false),
            LinuxPlan::NeedsSourceSelection
        );
        assert_eq!(linux_plan(false, false, true), LinuxPlan::Portal);
    }

    /// Wayland, XWayland's `DISPLAY` set or not: a kept grant reads the
    /// screen through the portal on a plain refresh.
    #[test]
    fn a_kept_grant_reads_the_screen_without_asking() {
        assert_eq!(linux_plan(false, true, false), LinuxPlan::Portal);
        assert_eq!(linux_plan(false, true, true), LinuxPlan::Portal);
    }
}
