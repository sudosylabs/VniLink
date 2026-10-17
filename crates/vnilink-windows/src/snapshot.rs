//! One Desktop Duplication frame for a remote screenshot.
//!
//! Opens the first output of the first adapter, the same output the desktop
//! stream uses. This does not choose among monitors. The secure UAC desktop
//! is not captured. The duplication is released when this function returns.

use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use vnilink::snapshot::{
    classify_stop, encode_bgra, unix_ms, watch_cancel, HostSnapshotFactory, ScreenImage,
    SnapshotFailure,
};

pub struct WindowsSnapshot;

impl HostSnapshotFactory for WindowsSnapshot {
    fn capture(
        &self,
        deadline: Instant,
        cancel: &Arc<AtomicBool>,
        _prompt: bool,
    ) -> Result<ScreenImage, SnapshotFailure> {
        // Windows has no consent dialog. Allow retries this first output.
        watch_cancel(deadline, cancel)?;
        let startup = vnilink::BackendStartup::with_stop(
            Arc::clone(cancel),
            deadline.saturating_duration_since(Instant::now()),
        );
        let capture = crate::capture::WindowsCapture::open(&startup)
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
                        crate::plan::SNAPSHOT_DISPLAY_ID,
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
