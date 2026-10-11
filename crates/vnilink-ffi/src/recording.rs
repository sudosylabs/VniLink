//! Nonvisual recording controls. Capture and the live viewer retain their owners.
use crate::{error::map_error, DesktopService, FfiError, VnilinkCore};
use std::{path::Path, sync::Arc};

#[derive(Clone, Copy, Debug, uniffi::Record)]
pub struct FfiRecordingLimits {
    pub max_bytes: u64,
    pub max_duration_ms: u64,
}
#[derive(Clone, Copy, Debug, uniffi::Record)]
pub struct FfiRecordingRetention {
    pub max_segments: u32,
    pub max_total_bytes: u64,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum FfiRecordingPhase {
    Starting,
    Recording,
    Finished,
    Failed,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum FfiRecordingFailure {
    Startup,
    Media,
    Storage,
    Limit,
    SlowStorage,
    FormatChanged,
}
#[derive(Clone, Debug, uniffi::Record)]
pub struct FfiRecordingStatus {
    pub phase: FfiRecordingPhase,
    pub failure: Option<FfiRecordingFailure>,
    pub frames: u64,
    pub bytes: u64,
    pub duration_ms: u64,
    pub segments: u32,
}
#[derive(Clone, Debug, uniffi::Record)]
pub struct FfiRecordingRecovery {
    pub complete_bytes: u64,
    pub frames: u64,
    pub duration_ms: u64,
}
/// Explicit locally selected recovery; call on a background thread. Interrupted
/// output has no cue index and is reported separately from normal finalization.
#[uniffi::export]
pub fn recover_desktop_recording(destination: String) -> Result<FfiRecordingRecovery, FfiError> {
    let recovered = vnilink_media::recover_desktop_recording(Path::new(&destination))
        .map_err(recording_error)?;
    Ok(FfiRecordingRecovery {
        complete_bytes: recovered.complete_bytes,
        frames: recovered.clusters,
        duration_ms: recovered.last_timestamp_ms,
    })
}
#[derive(uniffi::Object)]
pub struct RecordingHandle {
    inner: vnilink_media::DesktopRecorder,
}
#[uniffi::export]
impl RecordingHandle {
    /// Requests finalization without waiting on storage. Poll for Finished or Failed.
    pub fn stop(&self) {
        self.inner.stop();
    }
    pub fn status(&self) -> FfiRecordingStatus {
        use vnilink_media::{RecordingFailure as F, RecordingPhase as P};
        let status = self.inner.status();
        FfiRecordingStatus {
            phase: match status.phase {
                P::Starting => FfiRecordingPhase::Starting,
                P::Recording => FfiRecordingPhase::Recording,
                P::Finished => FfiRecordingPhase::Finished,
                P::Failed => FfiRecordingPhase::Failed,
            },
            failure: status.failure.map(|f| match f {
                F::Startup => FfiRecordingFailure::Startup,
                F::Media => FfiRecordingFailure::Media,
                F::Storage => FfiRecordingFailure::Storage,
                F::Limit => FfiRecordingFailure::Limit,
                F::SlowStorage => FfiRecordingFailure::SlowStorage,
                F::FormatChanged => FfiRecordingFailure::FormatChanged,
            }),
            frames: status.frames,
            bytes: status.bytes,
            duration_ms: status.duration_ms,
            segments: status.segments,
        }
    }
}
#[uniffi::export]
impl VnilinkCore {
    /// Call on a background thread: admission reserves a locally selected
    /// stem; segments land as `stem.000001.mkv`, ... once frames arrive.
    /// The host must separately authorize recording for this paired installation.
    pub fn start_desktop_recording(
        &self,
        desktop: Arc<DesktopService>,
        destination: String,
        limits: FfiRecordingLimits,
        retention: Option<FfiRecordingRetention>,
    ) -> Result<Arc<RecordingHandle>, FfiError> {
        let core = desktop.core.upgrade().ok_or(FfiError::NoSession)?;
        if !Arc::ptr_eq(&core, &self.inner) {
            return Err(FfiError::Unauthorized);
        }
        let inner = vnilink_media::DesktopRecorder::start_with_retention(
            &desktop.inner,
            Path::new(&destination),
            vnilink_media::RecordingLimits {
                max_bytes: limits.max_bytes,
                max_duration_ms: limits.max_duration_ms,
            },
            retention.map(|r| vnilink_media::RecordingRetention {
                max_segments: r.max_segments,
                max_total_bytes: r.max_total_bytes,
            }),
        )
        .map_err(recording_error)?;
        Ok(Arc::new(RecordingHandle { inner }))
    }
}
fn recording_error(error: vnilink_media::RecordingError) -> FfiError {
    use vnilink_media::RecordingError as E;
    match error {
        E::Source(error) => map_error(error),
        E::Storage(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            FfiError::FileExists
        }
        E::Storage(_) => FfiError::Unsupported {
            message: "recording storage is unavailable".into(),
        },
        E::Limit => FfiError::InvalidSetting {
            message: "recording limits or retention are outside supported bounds".into(),
        },
        E::InvalidMedia => FfiError::InvalidSetting {
            message: "recording requires a new absolute .mkv destination and valid H.264 media"
                .into(),
        },
        E::FormatChanged => FfiError::Unsupported {
            message: "recording media format changed".into(),
        },
    }
}
