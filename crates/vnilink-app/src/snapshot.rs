//! Facade for one remote screenshot. Capture policy stays in the core.

use crate::{error::map_error, FfiError, VnilinkCore};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use vnilink::snapshot::{ScreenAccess, ScreenSnapshot};

pub enum FfiScreenSnapshot {
    Ready {
        jpeg: Vec<u8>,
        width: u16,
        height: u16,
        captured_unix_ms: u64,
        display_id: String,
    },
    NeedsPermission,
    NeedsSourceSelection,
    Unavailable,
    TimedOut,
    Cancelled,
}

/// Probe result. This is not a picture.
pub enum FfiScreenAccess {
    Allowed,
    Denied,
    Unavailable,
}

impl VnilinkCore {
    pub fn request_screen_snapshot(
        &self,
        connection_id: u64,
        generation: u64,
        cancel: &Arc<AtomicBool>,
        prompt: bool,
    ) -> Result<FfiScreenSnapshot, FfiError> {
        Ok(
            match self
                .inner
                .request_screen_snapshot(connection_id, generation, cancel, prompt)
                .map_err(map_error)?
            {
                ScreenSnapshot::Ready(image) => FfiScreenSnapshot::Ready {
                    jpeg: image.jpeg,
                    width: image.width,
                    height: image.height,
                    captured_unix_ms: image.captured_unix_ms,
                    display_id: image.display_id,
                },
                ScreenSnapshot::NeedsPermission => FfiScreenSnapshot::NeedsPermission,
                ScreenSnapshot::NeedsSourceSelection => FfiScreenSnapshot::NeedsSourceSelection,
                ScreenSnapshot::Unavailable => FfiScreenSnapshot::Unavailable,
                ScreenSnapshot::TimedOut => FfiScreenSnapshot::TimedOut,
                ScreenSnapshot::Cancelled => FfiScreenSnapshot::Cancelled,
            },
        )
    }

    /// Whether the host would admit a screenshot. Does not capture or rate-limit.
    pub fn screen_capture_access(
        &self,
        connection_id: u64,
        generation: u64,
    ) -> Result<FfiScreenAccess, FfiError> {
        Ok(
            match self
                .inner
                .screen_capture_access(connection_id, generation)
                .map_err(map_error)?
            {
                ScreenAccess::Allowed => FfiScreenAccess::Allowed,
                ScreenAccess::Denied => FfiScreenAccess::Denied,
                ScreenAccess::Unavailable => FfiScreenAccess::Unavailable,
            },
        )
    }
}

/// Bridge outcome name. Each core failure keeps its own name.
pub fn snapshot_error_outcome(error: &FfiError) -> &'static str {
    match error {
        FfiError::NoSession => "disconnected",
        FfiError::StaleSession => "replaced",
        FfiError::NotPaired => "unpaired",
        FfiError::Unauthorized => "sharingDisabled",
        FfiError::Busy => "busy",
        FfiError::Timeout => "timedOut",
        FfiError::Unsupported { .. } => "unsupported",
        _ => "failed",
    }
}

#[cfg(test)]
mod tests {
    use super::snapshot_error_outcome;
    use crate::FfiError;

    #[test]
    fn snapshot_errors_keep_distinct_bridge_names() {
        assert_eq!(snapshot_error_outcome(&FfiError::NoSession), "disconnected");
        assert_eq!(snapshot_error_outcome(&FfiError::StaleSession), "replaced");
        assert_eq!(snapshot_error_outcome(&FfiError::NotPaired), "unpaired");
        assert_eq!(
            snapshot_error_outcome(&FfiError::Unauthorized),
            "sharingDisabled"
        );
        assert_eq!(snapshot_error_outcome(&FfiError::Busy), "busy");
        assert_eq!(snapshot_error_outcome(&FfiError::Timeout), "timedOut");
        assert_eq!(
            snapshot_error_outcome(&FfiError::Unsupported {
                message: "snapshot".into(),
            }),
            "unsupported"
        );
        assert_eq!(snapshot_error_outcome(&FfiError::ShutDown), "failed");
        assert_eq!(
            snapshot_error_outcome(&FfiError::Network {
                message: "reset".into(),
            }),
            "failed"
        );
    }
}
