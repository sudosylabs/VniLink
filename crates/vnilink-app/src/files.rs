//! Application file-manager surface: roots, listings, rename, delete and jobs.
//!
//! Every method follows the crate threading rule: call from a background
//! thread, never the UI thread. `job_poll` advances at most one file but
//! that file can take a while over the network; `fs_*` and
//! `job_submit_*` need a live control session. File bytes never cross
//! this boundary as whole vectors on receive: jobs stream to disk inside
//! the core, and listings carry names, sizes and progress only.

use crate::{error::map_error, FfiError, VnilinkCore};

/// One shared root, `/` or a drive: the id from `file_roots` plus its path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FfiFileRoot {
    pub id: u64,
    pub path: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FfiFolderKind {
    Home,
    Desktop,
    Documents,
    Downloads,
    Pictures,
    Volume,
}

/// One shared root: its id and name (`/` or a drive such as `C:`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FfiPlaceRoot {
    pub id: u64,
    pub name: String,
}

/// A well-known folder, named by root and relative path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FfiPlace {
    pub kind: FfiFolderKind,
    pub root: u64,
    pub path: String,
}

/// Where a file manager starts on one computer: its roots, then the
/// folders that exist there, home first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FfiPlaces {
    pub roots: Vec<FfiPlaceRoot>,
    pub folders: Vec<FfiPlace>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FfiEntryKind {
    File,
    Dir,
    Symlink,
    Other,
}

/// What a listed entry is proven by when it is changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FfiTokenKind {
    /// Device, inode and creation time.
    Identity,
    /// On a volume whose ids are entry positions (FAT, exFAT): the volume
    /// in `dev` and a digest of what the listing saw in `fingerprint`.
    Fingerprint,
}

/// The token a listing gives an entry, handed back with a rename or
/// delete so the core refuses a replacement. An identity with no device or
/// inode means the platform could not report one; the core treats the
/// token as weak. `born` is the creation time in nanoseconds, where kept.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FfiEntryToken {
    pub kind: FfiTokenKind,
    pub dev: Option<u64>,
    pub ino: Option<u64>,
    pub born: Option<u64>,
    pub fingerprint: Option<u64>,
}

#[derive(Clone, PartialEq, Eq)]
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

#[derive(Clone, PartialEq, Eq)]
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FfiJobDirection {
    Upload,
    Download,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FfiConflictChoice {
    Skip,
    Replace,
    KeepBoth,
}

/// One item named as the panes name it: a root, its folder under the root, and its name.
#[derive(Clone, PartialEq, Eq)]
pub struct FfiFileLocation {
    pub root: u64,
    pub path: String,
    pub name: String,
}

#[derive(Clone, PartialEq, Eq)]
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
    pub current_file: Option<String>,
    pub error: Option<String>,
    /// Where the transfer lands, when this side submitted it.
    pub destination: Option<FfiJobDestination>,
}

/// The folder a transfer lands in and what lands in it.
#[derive(Clone, PartialEq, Eq)]
pub struct FfiJobDestination {
    pub root: u64,
    pub folder: String,
    pub items: Vec<FfiJobItem>,
}

/// One file or folder a transfer copies, by the name it lands under.
#[derive(Clone, PartialEq, Eq)]
pub struct FfiJobItem {
    pub name: String,
    pub folder: bool,
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
        kind: match token.kind {
            vnilink::TokenKind::Identity => FfiTokenKind::Identity,
            vnilink::TokenKind::Fingerprint => FfiTokenKind::Fingerprint,
        },
        dev: token.dev,
        ino: token.ino,
        born: token.born,
        fingerprint: token.fingerprint,
    }
}

/// A token handed back from the application, refused unless its fields
/// are those of its kind: an identity carries no fingerprint, and a
/// fingerprint no inode or creation time.
pub(crate) fn to_entry_token(token: FfiEntryToken) -> Result<vnilink::EntryToken, FfiError> {
    let (kind, valid) = match token.kind {
        FfiTokenKind::Identity => (vnilink::TokenKind::Identity, token.fingerprint.is_none()),
        FfiTokenKind::Fingerprint => (
            vnilink::TokenKind::Fingerprint,
            token.ino.is_none() && token.born.is_none(),
        ),
    };
    if !valid {
        return Err(FfiError::InvalidSetting {
            message: "the entry token is not valid".to_owned(),
        });
    }
    Ok(vnilink::EntryToken {
        kind,
        dev: token.dev,
        ino: token.ino,
        born: token.born,
        fingerprint: token.fingerprint,
    })
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

pub(crate) fn places(places: vnilink::Places) -> FfiPlaces {
    FfiPlaces {
        roots: places
            .roots
            .into_iter()
            .map(|root| FfiPlaceRoot {
                id: root.id,
                name: root.name,
            })
            .collect(),
        folders: places
            .folders
            .into_iter()
            .map(|folder| FfiPlace {
                kind: match folder.kind {
                    vnilink::FolderKind::Home => FfiFolderKind::Home,
                    vnilink::FolderKind::Desktop => FfiFolderKind::Desktop,
                    vnilink::FolderKind::Documents => FfiFolderKind::Documents,
                    vnilink::FolderKind::Downloads => FfiFolderKind::Downloads,
                    vnilink::FolderKind::Pictures => FfiFolderKind::Pictures,
                    vnilink::FolderKind::Volume => FfiFolderKind::Volume,
                },
                root: folder.root,
                path: folder.path,
            })
            .collect(),
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
        current_file: snapshot.current_file,
        error: snapshot.error,
        destination: snapshot.destination.map(|destination| FfiJobDestination {
            root: destination.root,
            folder: destination.folder,
            items: destination
                .items
                .into_iter()
                .map(|item| FfiJobItem {
                    name: item.name,
                    folder: item.folder,
                })
                .collect(),
        }),
    }
}

impl VnilinkCore {
    pub fn enable_filesystem_access(&self) -> Result<(), FfiError> {
        self.inner.enable_filesystem_access().map_err(map_error)
    }
    /// Start once per application core; progress reads do not drive network I/O.
    pub fn start_file_job_driver(&self) {
        self.inner.start_job_driver();
    }

    /// Chooses the managed transfer directory. Refused while a
    /// controlling session is active.
    pub fn set_transfer_directory(&self, path: String) -> Result<(), FfiError> {
        self.inner.set_transfer_directory(path).map_err(map_error)
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

    /// Where a path on this computer lies under the shared roots: its
    /// folder's root and path, and its own name.
    pub fn locate(&self, path: String) -> Result<Option<FfiFileLocation>, FfiError> {
        self.inner
            .locate(std::path::Path::new(&path))
            .map(|found| found.map(|(root, path, name)| FfiFileLocation { root, path, name }))
            .map_err(map_error)
    }

    /// Watches one folder on this computer for the window, which hears of
    /// each change as a `FolderChanged` event with the returned id.
    pub fn watch_folder(&self, root: u64, path: String) -> Result<u64, FfiError> {
        self.inner.watch_folder(root, &path).map_err(map_error)
    }

    pub fn unwatch_folder(&self, watch: u64) {
        self.inner.unwatch_folder(watch);
    }

    /// This computer's roots, home folder and usual folders.
    pub fn places(&self) -> Result<FfiPlaces, FfiError> {
        self.inner.places().map(places).map_err(map_error)
    }

    /// Lists one page of a local directory under a shared root.
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

    /// Creates one directory level under a shared root.
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
            .rename_entry(
                root,
                &path,
                &name,
                expected.map(to_entry_token).transpose()?,
                &new_name,
            )
            .map_err(map_error)
    }

    /// Deletes one local entry inside its parent folder: to the Trash when
    /// `trash`, else permanently. `expected` carries the listing token; a
    /// replaced source is refused instead of deleting a different object.
    pub fn delete_entry(
        &self,
        root: u64,
        path: String,
        name: String,
        expected: Option<FfiEntryToken>,
        trash: bool,
    ) -> Result<(), FfiError> {
        let expected = expected.map(to_entry_token).transpose()?;
        if trash {
            self.inner.trash_entry(root, &path, &name, expected)
        } else {
            self.inner.delete_entry(root, &path, &name, expected)
        }
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
                expected.map(to_entry_token).transpose()?,
                &new_name,
            )
            .map_err(map_error)
    }

    /// Remote delete mirrors `delete_entry` on the controlled computer.
    pub fn fs_delete(
        &self,
        session_id: u64,
        generation: u64,
        root: u64,
        path: String,
        name: String,
        expected: Option<FfiEntryToken>,
    ) -> Result<(), FfiError> {
        self.inner
            .fs_delete(
                session_id,
                generation,
                root,
                &path,
                &name,
                expected.map(to_entry_token).transpose()?,
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
            kind: FfiTokenKind::Identity,
            dev: Some(3),
            ino: Some(9),
            born: Some(11),
            fingerprint: None,
        })
        .unwrap();
        assert!(token.is_strong());
        assert_eq!(entry_token(token).dev, Some(3));
        assert_eq!(entry_token(token).born, Some(11));
        let fingerprint = vnilink::EntryToken::fingerprinted(3, 42);
        assert_eq!(
            to_entry_token(entry_token(fingerprint)).unwrap(),
            fingerprint
        );
        for malformed in [
            FfiEntryToken {
                kind: FfiTokenKind::Identity,
                dev: Some(3),
                ino: Some(9),
                born: None,
                fingerprint: Some(1),
            },
            FfiEntryToken {
                kind: FfiTokenKind::Fingerprint,
                dev: Some(3),
                ino: Some(9),
                born: None,
                fingerprint: Some(1),
            },
            FfiEntryToken {
                kind: FfiTokenKind::Fingerprint,
                dev: Some(3),
                ino: None,
                born: Some(5),
                fingerprint: Some(1),
            },
        ] {
            assert!(to_entry_token(malformed).is_err(), "{malformed:?}");
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
