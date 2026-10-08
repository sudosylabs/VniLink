//! Transfer job records (Stage C).
//!
//! Jobs move whole manifests in either direction with staging,
//! conflict choices, cancellation and restartable progress. Each side
//! polls its own records; senders learn completion by asking the
//! receiver, so both directions complete honestly without either side
//! dialing blindly. Records persist as JSON beside the blob store.

mod wire;

pub(crate) use wire::{
    check_version, decode, encode, error_for_status, DownloadRequest, JobCancel, JobReply,
    JobResolve, JobResume, JobStateReply, JobStatus, ManifestReply, UploadOffer, JOB_VERSION,
    MAX_JOB_FRAME, STATUS_DENIED, STATUS_ERROR, STATUS_OK, STATUS_STALE, TAG_FS, TAG_JOB_CANCEL,
    TAG_JOB_DOWNLOAD, TAG_JOB_RESOLVE, TAG_JOB_RESUME, TAG_JOB_STATUS, TAG_JOB_UPLOAD,
};

use serde::{Deserialize, Serialize};

/// Data flow relative to the local installation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum JobDirection {
    Upload,
    Download,
}

/// Lifecycle of one job record. Terminal states are `Complete`,
/// `Cancelled` and `Failed`; `Interrupted` resumes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum JobState {
    Preparing,
    Serving,
    Transferring,
    WaitingDecision,
    Interrupted,
    Cancelled,
    Failed,
    Complete,
}

impl JobState {
    pub fn name(self) -> &'static str {
        match self {
            Self::Preparing => "preparing",
            Self::Serving => "serving",
            Self::Transferring => "transferring",
            Self::WaitingDecision => "waiting-decision",
            Self::Interrupted => "interrupted",
            Self::Cancelled => "cancelled",
            Self::Failed => "failed",
            Self::Complete => "complete",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        Some(match name {
            "preparing" => Self::Preparing,
            "serving" => Self::Serving,
            "transferring" => Self::Transferring,
            "waiting-decision" => Self::WaitingDecision,
            "interrupted" => Self::Interrupted,
            "cancelled" => Self::Cancelled,
            "failed" => Self::Failed,
            "complete" => Self::Complete,
            _ => return None,
        })
    }

    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Cancelled | Self::Failed | Self::Complete)
    }
}

/// What to do with one colliding destination.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ConflictChoice {
    Skip,
    Replace,
    KeepBoth,
}

/// The observable picture of one job. Counts are exact; names of waiting
/// files are exposed so the UI can ask a scoped question.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobSnapshot {
    pub id: u64,
    pub peer: String,
    /// Display label: first file path plus a count, e.g. `"a.txt +2 more"`.
    /// Paths never enter logs; this field is for the queue row.
    pub label: String,
    pub direction: JobDirection,
    pub sender: bool,
    pub state: JobState,
    pub total_bytes: u64,
    pub done_bytes: u64,
    pub files_total: u64,
    pub files_done: u64,
    pub resumed_files: u64,
    pub skipped_files: u64,
    pub skipped_symlinks: u64,
    pub waiting_file: Option<String>,
    pub error: Option<String>,
}
