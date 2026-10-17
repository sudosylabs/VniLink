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

pub fn linux_plan(has_display: bool, prompt: bool) -> LinuxPlan {
    if has_display {
        LinuxPlan::X11
    } else if prompt {
        LinuxPlan::Portal
    } else {
        LinuxPlan::NeedsSourceSelection
    }
}

pub struct LinuxSnapshot;

impl HostSnapshotFactory for LinuxSnapshot {
    fn capture(
        &self,
        deadline: Instant,
        cancel: &Arc<AtomicBool>,
        prompt: bool,
    ) -> Result<ScreenImage, SnapshotFailure> {
        watch_cancel(deadline, cancel)?;
        match linux_plan(std::env::var_os("DISPLAY").is_some(), prompt) {
            LinuxPlan::NeedsSourceSelection => Err(SnapshotFailure::NeedsSourceSelection),
            LinuxPlan::X11 => capture_x11(deadline, cancel),
            LinuxPlan::Portal => capture_portal(deadline, cancel),
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
            .block_on(crate::dbus::open_monitor(&mut session))
            .map_err(|_| SnapshotFailure::NeedsSourceSelection)?;
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
        let _ = (deadline, cancel);
        Err(SnapshotFailure::NeedsSourceSelection)
    }
}

#[cfg(test)]
mod tests {
    use super::{linux_plan, LinuxPlan};

    #[test]
    fn selection_does_not_open_a_portal_and_choose_does() {
        assert_eq!(linux_plan(true, false), LinuxPlan::X11);
        assert_eq!(linux_plan(true, true), LinuxPlan::X11);
        assert_eq!(linux_plan(false, false), LinuxPlan::NeedsSourceSelection);
        assert_eq!(linux_plan(false, true), LinuxPlan::Portal);
    }
}
