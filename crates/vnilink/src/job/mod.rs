// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

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
    MAX_JOB_FRAME, STATUS_BUSY, STATUS_DENIED, STATUS_ERROR, STATUS_OK, STATUS_STALE,
    TAG_JOB_CANCEL, TAG_JOB_DOWNLOAD, TAG_JOB_MANIFEST, TAG_JOB_RESOLVE, TAG_JOB_RESUME,
    TAG_JOB_STATUS, TAG_JOB_UPLOAD,
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
    /// The file moving now, while the transfer moves files.
    pub current_file: Option<String>,
    /// Why the transfer stopped, failed, or is trying again while it
    /// still moves files.
    pub error: Option<String>,
    /// Where the transfer lands, on the side that submitted it.
    pub destination: Option<JobDestination>,
}

/// Where a transfer lands: the folder on the receiving computer and the
/// files and folders copied into it, so the Files tab can show each one
/// arriving there.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobDestination {
    pub root: u64,
    pub folder: String,
    pub items: Vec<JobItem>,
}

/// One file or folder a transfer copies, by the name it arrives under.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobItem {
    pub name: String,
    /// Known once the sender has listed what it sends.
    pub folder: bool,
}

/// Host-local cache retention. Active jobs/imports remain independently pinned.
/// Zero bytes or age disables the corresponding inactive payload cache.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileRetentionPolicy {
    pub resume_cache_bytes: u64,
    pub resume_cache_age_secs: u64,
    pub recent_fetch_bytes: u64,
    pub recent_fetch_age_secs: u64,
}
impl Default for FileRetentionPolicy {
    fn default() -> Self {
        Self {
            resume_cache_bytes: 4 * 1024 * 1024 * 1024,
            resume_cache_age_secs: 7 * 24 * 60 * 60,
            recent_fetch_bytes: 1024 * 1024 * 1024,
            recent_fetch_age_secs: 300,
        }
    }
}
impl FileRetentionPolicy {
    pub fn validate(self) -> Result<Self, crate::Error> {
        if self.resume_cache_bytes > 64 * 1024 * 1024 * 1024
            || self.recent_fetch_bytes > 64 * 1024 * 1024 * 1024
            || self.resume_cache_age_secs > 90 * 24 * 60 * 60
            || self.recent_fetch_age_secs > 90 * 24 * 60 * 60
        {
            return Err(crate::Error::InvalidSetting(
                "file retention policy is out of range",
            ));
        }
        Ok(self)
    }
}

/// Receiver worker and peak temporary-storage admission. Disk checks are
/// estimates; actual write failures remain authoritative and preserve sources.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileResourcePolicy {
    pub max_concurrent_receivers: u16,
    pub max_reserved_bytes: u64,
    pub minimum_free_bytes: u64,
}
impl Default for FileResourcePolicy {
    fn default() -> Self {
        Self {
            max_concurrent_receivers: 4,
            max_reserved_bytes: 64 * 1024 * 1024 * 1024,
            minimum_free_bytes: 256 * 1024 * 1024,
        }
    }
}
impl FileResourcePolicy {
    pub fn validate(self) -> Result<Self, crate::Error> {
        if !(1..=16).contains(&self.max_concurrent_receivers)
            || self.max_reserved_bytes == 0
            || self.max_reserved_bytes > 1024 * 1024 * 1024 * 1024
            || self.minimum_free_bytes > 1024 * 1024 * 1024 * 1024
        {
            return Err(crate::Error::InvalidSetting(
                "file resource policy is out of range",
            ));
        }
        Ok(self)
    }
}
