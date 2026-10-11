//! File-manager surface for Swift/C#: roots, listings, rename and jobs.
//!
//! Every method follows the crate threading rule: call from a background
//! thread, never the UI thread. `job_poll` advances at most one file but
//! that file can take a while over the network; `fs_*` and
//! `job_submit_*` need a live control session. File bytes never cross
//! this boundary as whole vectors on receive: jobs stream to disk inside
//! the core, and listings carry names, sizes and progress only.

use crate::{error::map_error, FfiError, VnilinkCore};

/// One owner-approved root: the id from `file_roots` plus its path.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct FfiFileRoot {
    pub id: u64,
    pub path: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum FfiEntryKind {
    File,
    Dir,
    Symlink,
    Other,
}

/// Listing identity for rename fencing. Absent fields mean the platform
/// could not report an identity; the core treats the token as weak.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Record)]
pub struct FfiEntryToken {
    pub dev: Option<u64>,
    pub ino: Option<u64>,
}

#[derive(Clone, PartialEq, Eq, uniffi::Record)]
pub struct FfiFsEntry {
    pub name: String,
    pub kind: FfiEntryKind,
    pub size: u64,
    pub mtime_ms: u64,
    pub token: FfiEntryToken,
}

impl std::fmt::Debug for FfiFsEntry {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("FfiFsEntry")
            .field("kind", &self.kind)
            .field("size", &self.size)
            .finish()
    }
}

#[derive(Clone, PartialEq, Eq, uniffi::Record)]
pub struct FfiDirPage {
    pub entries: Vec<FfiFsEntry>,
    pub total: u64,
    pub next_offset: Option<u64>,
    pub truncated: bool,
    pub non_utf8_skipped: u64,
}

impl std::fmt::Debug for FfiDirPage {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("FfiDirPage")
            .field("len", &self.entries.len())
            .field("total", &self.total)
            .field("truncated", &self.truncated)
            .finish()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum FfiJobState {
    Preparing,
    Serving,
    Transferring,
    WaitingDecision,
    Interrupted,
    Cancelled,
    Failed,
    Complete,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum FfiJobDirection {
    Upload,
    Download,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum FfiConflictChoice {
    Skip,
    Replace,
    KeepBoth,
}

#[derive(Clone, PartialEq, Eq, uniffi::Record)]
pub struct FfiJobSnapshot {
    pub id: u64,
    pub peer: String,
    pub label: String,
    pub direction: FfiJobDirection,
    pub sender: bool,
    pub state: FfiJobState,
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

impl std::fmt::Debug for FfiJobSnapshot {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("FfiJobSnapshot")
            .field("id", &self.id)
            .field("state", &self.state)
            .field("files_done", &self.files_done)
            .field("files_total", &self.files_total)
            .finish()
    }
}

pub(crate) fn entry_kind(kind: vnilink::EntryKind) -> FfiEntryKind {
    match kind {
        vnilink::EntryKind::File => FfiEntryKind::File,
        vnilink::EntryKind::Dir => FfiEntryKind::Dir,
        vnilink::EntryKind::Symlink => FfiEntryKind::Symlink,
        vnilink::EntryKind::Other => FfiEntryKind::Other,
    }
}

pub(crate) fn entry_token(token: vnilink::EntryToken) -> FfiEntryToken {
    FfiEntryToken {
        dev: token.dev,
        ino: token.ino,
    }
}

pub(crate) fn to_entry_token(token: FfiEntryToken) -> vnilink::EntryToken {
    vnilink::EntryToken {
        dev: token.dev,
        ino: token.ino,
    }
}

pub(crate) fn fs_entry(entry: vnilink::FsEntry) -> FfiFsEntry {
    FfiFsEntry {
        name: entry.name,
        kind: entry_kind(entry.kind),
        size: entry.size,
        mtime_ms: entry.mtime_ms,
        token: entry_token(entry.token),
    }
}

pub(crate) fn dir_page(page: vnilink::DirPage) -> FfiDirPage {
    FfiDirPage {
        entries: page.entries.into_iter().map(fs_entry).collect(),
        total: page.total,
        next_offset: page.next_offset,
        truncated: page.truncated,
        non_utf8_skipped: page.non_utf8_skipped,
    }
}

pub(crate) fn job_state(state: vnilink::JobState) -> FfiJobState {
    match state {
        vnilink::JobState::Preparing => FfiJobState::Preparing,
        vnilink::JobState::Serving => FfiJobState::Serving,
        vnilink::JobState::Transferring => FfiJobState::Transferring,
        vnilink::JobState::WaitingDecision => FfiJobState::WaitingDecision,
        vnilink::JobState::Interrupted => FfiJobState::Interrupted,
        vnilink::JobState::Cancelled => FfiJobState::Cancelled,
        vnilink::JobState::Failed => FfiJobState::Failed,
        vnilink::JobState::Complete => FfiJobState::Complete,
    }
}

pub(crate) fn job_direction(direction: vnilink::JobDirection) -> FfiJobDirection {
    match direction {
        vnilink::JobDirection::Upload => FfiJobDirection::Upload,
        vnilink::JobDirection::Download => FfiJobDirection::Download,
    }
}

pub(crate) fn to_conflict_choice(choice: FfiConflictChoice) -> vnilink::ConflictChoice {
    match choice {
        FfiConflictChoice::Skip => vnilink::ConflictChoice::Skip,
        FfiConflictChoice::Replace => vnilink::ConflictChoice::Replace,
        FfiConflictChoice::KeepBoth => vnilink::ConflictChoice::KeepBoth,
    }
}

pub(crate) fn job_snapshot(snapshot: vnilink::JobSnapshot) -> FfiJobSnapshot {
    FfiJobSnapshot {
        id: snapshot.id,
        peer: snapshot.peer,
        label: snapshot.label,
        direction: job_direction(snapshot.direction),
        sender: snapshot.sender,
        state: job_state(snapshot.state),
        total_bytes: snapshot.total_bytes,
        done_bytes: snapshot.done_bytes,
        files_total: snapshot.files_total,
        files_done: snapshot.files_done,
        resumed_files: snapshot.resumed_files,
        skipped_files: snapshot.skipped_files,
        skipped_symlinks: snapshot.skipped_symlinks,
        waiting_file: snapshot.waiting_file,
        error: snapshot.error,
    }
}

#[uniffi::export]
impl VnilinkCore {
    /// Chooses the managed directory for blob storage. Refused while a
    /// controlling session is active.
    pub fn set_blob_directory(&self, path: String) -> Result<(), FfiError> {
        self.inner.set_blob_directory(path).map_err(map_error)
    }

    /// Replaces the owner-approved file roots. Nothing is browsable until
    /// this runs. Refused while a controlling session is active.
    pub fn set_file_roots(&self, paths: Vec<String>) -> Result<(), FfiError> {
        self.inner
            .set_file_roots(paths.into_iter().map(std::path::PathBuf::from).collect())
            .map_err(map_error)
    }

    pub fn file_roots(&self) -> Result<Vec<FfiFileRoot>, FfiError> {
        self.inner
            .file_roots()
            .map(|roots| {
                roots
                    .into_iter()
                    .map(|(id, path)| FfiFileRoot {
                        id,
                        path: path.to_string_lossy().into_owned(),
                    })
                    .collect()
            })
            .map_err(map_error)
    }

    /// Lists one page of a local directory under an approved root.
    pub fn list_dir(
        &self,
        root: u64,
        path: String,
        offset: u64,
        limit: u16,
    ) -> Result<FfiDirPage, FfiError> {
        self.inner
            .list_dir(root, &path, offset, limit)
            .map(dir_page)
            .map_err(map_error)
    }

    /// Creates one directory level under an approved root.
    pub fn create_dir(&self, root: u64, path: String, name: String) -> Result<(), FfiError> {
        self.inner.create_dir(root, &path, &name).map_err(map_error)
    }

    /// Renames one local entry inside its parent folder. `expected`
    /// carries the listing token; a replaced source is refused instead of
    /// renaming a different object. Never overwrites.
    pub fn rename_entry(
        &self,
        root: u64,
        path: String,
        name: String,
        expected: Option<FfiEntryToken>,
        new_name: String,
    ) -> Result<(), FfiError> {
        self.inner
            .rename_entry(root, &path, &name, expected.map(to_entry_token), &new_name)
            .map_err(map_error)
    }

    /// Lists a directory on the controlled computer. Needs the live
    /// session id and generation.
    pub fn fs_list(
        &self,
        session_id: u64,
        generation: u64,
        root: u64,
        path: String,
        offset: u64,
        limit: u16,
    ) -> Result<FfiDirPage, FfiError> {
        self.inner
            .fs_list(session_id, generation, root, &path, offset, limit)
            .map(dir_page)
            .map_err(map_error)
    }

    pub fn fs_mkdir(
        &self,
        session_id: u64,
        generation: u64,
        root: u64,
        path: String,
        name: String,
    ) -> Result<(), FfiError> {
        self.inner
            .fs_mkdir(session_id, generation, root, &path, &name)
            .map_err(map_error)
    }

    /// Remote rename mirrors `rename_entry` on the controlled computer.
    #[allow(clippy::too_many_arguments)]
    pub fn fs_rename(
        &self,
        session_id: u64,
        generation: u64,
        root: u64,
        path: String,
        name: String,
        expected: Option<FfiEntryToken>,
        new_name: String,
    ) -> Result<(), FfiError> {
        self.inner
            .fs_rename(
                session_id,
                generation,
                root,
                &path,
                &name,
                expected.map(to_entry_token),
                &new_name,
            )
            .map_err(map_error)
    }

    /// Uploads a local tree to the paired peer. Returns the sender job id;
    /// the peer adopts it. Drive with `job_poll` on both sides.
    pub fn job_submit_upload(
        &self,
        peer_endpoint_id: String,
        src_root: u64,
        src_rel: String,
        dst_root: u64,
        dst_rel: String,
    ) -> Result<u64, FfiError> {
        self.inner
            .job_submit_upload(&peer_endpoint_id, src_root, &src_rel, dst_root, &dst_rel)
            .map_err(map_error)
    }

    /// Asks the paired peer to prepare a download. Returns the receiver
    /// job id, adopted from the host. Drive with `job_poll` on both sides.
    pub fn job_submit_download(
        &self,
        peer_endpoint_id: String,
        src_root: u64,
        src_rel: String,
        dst_root: u64,
        dst_rel: String,
    ) -> Result<u64, FfiError> {
        self.inner
            .job_submit_download(&peer_endpoint_id, src_root, &src_rel, dst_root, &dst_rel)
            .map_err(map_error)
    }

    pub fn job_snapshot(&self, id: u64) -> Result<FfiJobSnapshot, FfiError> {
        self.inner
            .job_snapshot(id)
            .map(job_snapshot)
            .map_err(map_error)
    }

    pub fn job_list(&self) -> Result<Vec<FfiJobSnapshot>, FfiError> {
        self.inner
            .job_list()
            .map(|jobs| jobs.into_iter().map(job_snapshot).collect())
            .map_err(map_error)
    }

    /// Advances one job by at most one file. Blocks; call off the UI
    /// thread. Receivers fetch, stage and publish; senders ask the
    /// receiver for its state.
    pub fn job_poll(&self, id: u64) -> Result<FfiJobSnapshot, FfiError> {
        self.inner.job_poll(id).map(job_snapshot).map_err(map_error)
    }

    pub fn job_cancel(&self, id: u64) -> Result<FfiJobSnapshot, FfiError> {
        self.inner
            .job_cancel(id)
            .map(job_snapshot)
            .map_err(map_error)
    }

    pub fn job_resume(&self, id: u64) -> Result<FfiJobSnapshot, FfiError> {
        self.inner
            .job_resume(id)
            .map(job_snapshot)
            .map_err(map_error)
    }

    /// Answers the waiting conflict for one job.
    pub fn job_resolve(
        &self,
        id: u64,
        choice: FfiConflictChoice,
        apply_to_remaining: bool,
    ) -> Result<FfiJobSnapshot, FfiError> {
        self.inner
            .job_resolve(id, to_conflict_choice(choice), apply_to_remaining)
            .map(job_snapshot)
            .map_err(map_error)
    }

    /// Forgets one terminal job (Complete, Cancelled, Failed), releasing its
    /// staging and orphaned offers. Active/Waiting/Interrupted refused.
    pub fn job_forget(&self, id: u64) -> Result<(), FfiError> {
        self.inner.job_forget(id).map_err(map_error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mappings_cover_every_variant() {
        assert_eq!(entry_kind(vnilink::EntryKind::File), FfiEntryKind::File);
        assert_eq!(entry_kind(vnilink::EntryKind::Dir), FfiEntryKind::Dir);
        assert_eq!(
            entry_kind(vnilink::EntryKind::Symlink),
            FfiEntryKind::Symlink
        );
        assert_eq!(entry_kind(vnilink::EntryKind::Other), FfiEntryKind::Other);
        for (state, expected) in [
            (vnilink::JobState::Preparing, FfiJobState::Preparing),
            (vnilink::JobState::Serving, FfiJobState::Serving),
            (vnilink::JobState::Transferring, FfiJobState::Transferring),
            (
                vnilink::JobState::WaitingDecision,
                FfiJobState::WaitingDecision,
            ),
            (vnilink::JobState::Interrupted, FfiJobState::Interrupted),
            (vnilink::JobState::Cancelled, FfiJobState::Cancelled),
            (vnilink::JobState::Failed, FfiJobState::Failed),
            (vnilink::JobState::Complete, FfiJobState::Complete),
        ] {
            assert_eq!(job_state(state), expected);
        }
        assert_eq!(
            job_direction(vnilink::JobDirection::Upload),
            FfiJobDirection::Upload
        );
        assert_eq!(
            job_direction(vnilink::JobDirection::Download),
            FfiJobDirection::Download
        );
        assert_eq!(
            to_conflict_choice(FfiConflictChoice::Skip),
            vnilink::ConflictChoice::Skip
        );
        assert_eq!(
            to_conflict_choice(FfiConflictChoice::Replace),
            vnilink::ConflictChoice::Replace
        );
        assert_eq!(
            to_conflict_choice(FfiConflictChoice::KeepBoth),
            vnilink::ConflictChoice::KeepBoth
        );
        let token = to_entry_token(FfiEntryToken {
            dev: Some(3),
            ino: Some(9),
        });
        assert!(token.is_strong());
        assert_eq!(entry_token(token).dev, Some(3));
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Record)]
pub struct FfiFileRetentionPolicy {
    pub resume_cache_bytes: u64,
    pub resume_cache_age_secs: u64,
    pub recent_fetch_bytes: u64,
    pub recent_fetch_age_secs: u64,
}
impl From<vnilink::job::FileRetentionPolicy> for FfiFileRetentionPolicy {
    fn from(p: vnilink::job::FileRetentionPolicy) -> Self {
        Self {
            resume_cache_bytes: p.resume_cache_bytes,
            resume_cache_age_secs: p.resume_cache_age_secs,
            recent_fetch_bytes: p.recent_fetch_bytes,
            recent_fetch_age_secs: p.recent_fetch_age_secs,
        }
    }
}
impl From<FfiFileRetentionPolicy> for vnilink::job::FileRetentionPolicy {
    fn from(p: FfiFileRetentionPolicy) -> Self {
        Self {
            resume_cache_bytes: p.resume_cache_bytes,
            resume_cache_age_secs: p.resume_cache_age_secs,
            recent_fetch_bytes: p.recent_fetch_bytes,
            recent_fetch_age_secs: p.recent_fetch_age_secs,
        }
    }
}
#[uniffi::export]
impl VnilinkCore {
    pub fn file_retention_policy(&self) -> Result<FfiFileRetentionPolicy, FfiError> {
        self.inner
            .file_retention_policy()
            .map(Into::into)
            .map_err(map_error)
    }
    pub fn set_file_retention_policy(
        &self,
        policy: FfiFileRetentionPolicy,
    ) -> Result<(), FfiError> {
        self.inner
            .set_file_retention_policy(policy.into())
            .map_err(map_error)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Record)]
pub struct FfiFileResourcePolicy {
    pub max_concurrent_receivers: u16,
    pub max_reserved_bytes: u64,
    pub minimum_free_bytes: u64,
}
impl From<vnilink::job::FileResourcePolicy> for FfiFileResourcePolicy {
    fn from(p: vnilink::job::FileResourcePolicy) -> Self {
        Self {
            max_concurrent_receivers: p.max_concurrent_receivers,
            max_reserved_bytes: p.max_reserved_bytes,
            minimum_free_bytes: p.minimum_free_bytes,
        }
    }
}
impl From<FfiFileResourcePolicy> for vnilink::job::FileResourcePolicy {
    fn from(p: FfiFileResourcePolicy) -> Self {
        Self {
            max_concurrent_receivers: p.max_concurrent_receivers,
            max_reserved_bytes: p.max_reserved_bytes,
            minimum_free_bytes: p.minimum_free_bytes,
        }
    }
}
#[uniffi::export]
impl VnilinkCore {
    pub fn file_resource_policy(&self) -> Result<FfiFileResourcePolicy, FfiError> {
        self.inner
            .file_resource_policy()
            .map(Into::into)
            .map_err(map_error)
    }
    pub fn set_file_resource_policy(&self, policy: FfiFileResourcePolicy) -> Result<(), FfiError> {
        self.inner
            .set_file_resource_policy(policy.into())
            .map_err(map_error)
    }
}
