//! Job-control wire codec (Stage C).
//!
//! Short streams on the auxiliary connection carry one framed message:
//! a tag byte plus a versioned JSON body. Manifests ride inside these
//! messages; file bytes never do (they move over blob connections).
//! Every request carries the session generation and is fenced before any
//! grant check or disk touch, exactly like filesystem requests.

use serde::{Deserialize, Serialize};

use crate::error::Error;
use crate::manifest::Manifest;

/// Job wire version. Bump on any incompatible change.
pub const JOB_VERSION: u8 = 1;

/// Largest job-control frame. It holds a whole manifest: up to
/// `MAX_MANIFEST_FILES` entries of a path, a hash and sizes in JSON.
pub const MAX_JOB_FRAME: usize = 64 * 1024 * 1024;

// Tag 0x01 carried a files-only session's filesystem requests; a link's
// gate drops it, and it stays unused.
/// Stream tag for an upload offer.
pub const TAG_JOB_UPLOAD: u8 = 0x02;
/// Stream tag for a download request.
pub const TAG_JOB_DOWNLOAD: u8 = 0x03;
/// Stream tag for a job-status query.
pub const TAG_JOB_STATUS: u8 = 0x04;
/// Stream tag for symmetric cancellation: either side tells the peer that
/// one job is cancelled. The peer marks its record cancelled and stops its
/// in-flight fetch, so sender cancellation still prevents delivery.
pub const TAG_JOB_CANCEL: u8 = 0x05;
/// Stream tag for conflict decisions: the sender forwards the owner's
/// Skip/Replace/KeepBoth choice to the receiver that owns the conflict.
pub const TAG_JOB_RESOLVE: u8 = 0x06;
/// Stream tag for coordinated resume: either side tells the peer to resume
/// after symmetric cancellation, reauthorizing offers as needed.
pub const TAG_JOB_RESUME: u8 = 0x07;
/// Stream tag for a download's manifest: the controller asks, with a
/// [`JobStatus`], until the host has prepared it ([`ManifestReply`]).
/// Auxiliary streams share their tags with the service, desktop and
/// clipboard streams (8 to 11).
pub const TAG_JOB_MANIFEST: u8 = 0x0d;

pub(crate) const STATUS_OK: u8 = 0;
pub(crate) const STATUS_DENIED: u8 = 2;
pub(crate) const STATUS_STALE: u8 = 3;
pub(crate) const STATUS_ERROR: u8 = 4;
/// The peer's job is held by a step that has not ended; asking again succeeds.
pub(crate) const STATUS_BUSY: u8 = 5;

/// Controller offers an upload: the manifest is fully hashed and offered.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UploadOffer {
    pub v: u8,
    pub id: u32,
    pub generation: u64,
    pub sender_job: u64,
    pub manifest: Manifest,
    pub dst_root: u64,
    pub dst_rel: String,
}

/// Controller asks the host to prepare a download from its roots: the
/// entries `names` of `src_folder`, or with no names the whole root.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DownloadRequest {
    pub v: u8,
    pub id: u32,
    pub generation: u64,
    pub src_root: u64,
    pub src_folder: String,
    pub names: Vec<String>,
}

/// Host answers an upload offer, adopting the sender's job id.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobReply {
    pub v: u8,
    pub id: u32,
    pub status: u8,
    pub job_id: u64,
}

/// Host answers a download request, and each later ask for its manifest:
/// with the prepared manifest, or with none while it is still preparing.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManifestReply {
    pub v: u8,
    pub id: u32,
    pub status: u8,
    pub job_id: u64,
    pub manifest: Option<Manifest>,
}

/// Either side asks the other for one job's receiver-side state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobStatus {
    pub v: u8,
    pub id: u32,
    pub generation: u64,
    pub job_id: u64,
}

/// Either side tells the peer that one job is cancelled. Symmetric with
/// status queries: the generation is fenced before any state touch.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobCancel {
    pub v: u8,
    pub id: u32,
    pub generation: u64,
    pub job_id: u64,
}

/// Receiver-side state answer. `state` is the `JobState` name; `error`
/// carries the terminal reason without paths or bytes. `waiting_file` names
/// the conflicting destination path when `state` is `waiting-decision`, so
/// the sender can show the same conflict without its own Files window on
/// the receiver.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobStateReply {
    pub v: u8,
    pub id: u32,
    pub status: u8,
    pub state: String,
    pub error: Option<String>,
    pub waiting_file: Option<String>,
    /// Bytes the receiver holds, verified, including the file arriving now.
    #[serde(default)]
    pub done_bytes: u64,
    /// Files landed, and files skipped, so far.
    #[serde(default)]
    pub files_done: u64,
    #[serde(default)]
    pub skipped_files: u64,
}

/// Sender forwards the owner's conflict decision to the receiver that owns
/// the waiting file. `choice` is `skip`/`replace`/`keep-both`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobResolve {
    pub v: u8,
    pub id: u32,
    pub generation: u64,
    pub job_id: u64,
    pub choice: String,
    pub apply_to_remaining: bool,
}

/// Either side tells the peer to resume after symmetric cancellation.
/// Generation fenced; the peer reauthorizes offers and returns to active.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobResume {
    pub v: u8,
    pub id: u32,
    pub generation: u64,
    pub job_id: u64,
}

pub(crate) fn encode<T: Serialize>(value: &T) -> Result<Vec<u8>, Error> {
    serde_json::to_vec(value)
        .map_err(|_| Error::InvalidSetting("the job message cannot be encoded"))
}

pub(crate) fn decode<T: serde::de::DeserializeOwned>(bytes: &[u8]) -> Result<T, Error> {
    if bytes.len() > MAX_JOB_FRAME {
        return Err(Error::Malformed);
    }
    serde_json::from_slice(bytes).map_err(|_| Error::Malformed)
}

pub(crate) fn check_version(v: u8) -> Result<(), Error> {
    if v != JOB_VERSION {
        return Err(Error::IncompatibleVersion);
    }
    Ok(())
}

pub(crate) fn error_for_status(status: u8) -> Error {
    match status {
        STATUS_DENIED => Error::TransferDenied,
        STATUS_STALE => Error::StaleSession,
        STATUS_BUSY => Error::Busy,
        _ => Error::InvalidSetting("the peer refused the job request"),
    }
}
