//! One ScreenCaptureKit frame for a remote screenshot.
//!
//! Screen Recording is preflighted. Selecting a device does not prompt.
//! `CGRequestScreenCaptureAccess` runs only after the remote user chooses Allow.
//! FileVault preboot is not a logged-in display and is not captured.
//! The stream is stopped when the capture value is dropped.

use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use vnilink::snapshot::{
    classify_stop, encode_bgra, unix_ms, watch_cancel, HostSnapshotFactory, ScreenImage,
    SnapshotFailure,
};

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGPreflightScreenCaptureAccess() -> bool;
    fn CGRequestScreenCaptureAccess() -> bool;
}

/// What preflight and the explicit Allow action may do. Tests decide this
/// without calling CoreGraphics.
pub(crate) enum MacConsent {
    Open,
    /// The caller may show the system dialog, then open if it is granted.
    Request,
    NeedsPermission,
}

pub(crate) fn mac_consent(preflight_allowed: bool, prompt: bool) -> MacConsent {
    if preflight_allowed {
        MacConsent::Open
    } else if prompt {
        MacConsent::Request
    } else {
        MacConsent::NeedsPermission
    }
}

pub struct MacSnapshot;

impl HostSnapshotFactory for MacSnapshot {
    fn capture(
        &self,
        deadline: Instant,
        cancel: &Arc<AtomicBool>,
        prompt: bool,
    ) -> Result<ScreenImage, SnapshotFailure> {
        watch_cancel(deadline, cancel)?;
        // SAFETY: CGPreflightScreenCaptureAccess takes no pointer and does not
        // show a dialog. It only reports the process TCC state.
        let preflight_allowed = unsafe { CGPreflightScreenCaptureAccess() };
        match mac_consent(preflight_allowed, prompt) {
            MacConsent::NeedsPermission => return Err(SnapshotFailure::NeedsPermission),
            MacConsent::Open => {}
            MacConsent::Request => {
                // SAFETY: CGRequestScreenCaptureAccess takes no pointer. It may
                // show the system dialog, and only the explicit Allow action sets prompt.
                if unsafe { !CGRequestScreenCaptureAccess() } {
                    return Err(SnapshotFailure::NeedsPermission);
                }
            }
        }
        let startup = vnilink::BackendStartup::with_stop(
            Arc::clone(cancel),
            deadline.saturating_duration_since(Instant::now()),
        );
        let (capture, display_id) = crate::capture::ScreenCapture::open_snapshot(&startup)
            .map_err(|_| classify_stop(cancel, deadline))?;
        let mut stamp = 1u64;
        loop {
            watch_cancel(deadline, cancel)?;
            match capture.poll_frame(stamp) {
                Ok(Some(frame)) => {
                    let captured_unix_ms = unix_ms();
                    return encode_bgra(
                        frame.width(),
                        frame.height(),
                        frame.stride(),
                        frame.pixels(),
                        captured_unix_ms,
                        &display_id,
                    );
                }
                Ok(None) => {
                    stamp = stamp.saturating_add(1);
                    thread::sleep(Duration::from_millis(20));
                }
                Err(_) => return Err(classify_stop(cancel, deadline)),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{mac_consent, MacConsent};

    #[test]
    fn selection_preflights_and_only_allow_requests_consent() {
        assert!(matches!(mac_consent(true, false), MacConsent::Open));
        assert!(matches!(mac_consent(true, true), MacConsent::Open));
        assert!(matches!(
            mac_consent(false, false),
            MacConsent::NeedsPermission
        ));
        assert!(matches!(mac_consent(false, true), MacConsent::Request));
    }
}
