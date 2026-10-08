//! True same-directory rename with no-replace semantics.
//!
//! The source is re-resolved and re-stated at execution time: the expected
//! token from listing must still name the same object, otherwise the entry
//! was replaced and the mutation is refused instead of renaming a different
//! object. The destination must not exist; the atomic primitive in
//! `vnilink-rename` enforces that without a check-then-rename race on Linux
//! and macOS. Renaming a folder never transfers or rewrites its children:
//! the directory entry itself moves.
//!
//! Case-only renames on case-insensitive filesystems go through a temporary
//! sibling so the object keeps its identity when the platform cannot rename
//! in place. Anything that was actively transferred stays the transfer
//! manager's business: this layer refuses to rename a source or destination
//! it cannot state, and reports interruption instead of guessing.

use crate::error::Error;

#[cfg(not(unix))]
use super::entry::stat;
use super::entry::{EntryKind, EntryToken};
#[cfg(unix)]
use super::path::check_name;
#[cfg(not(unix))]
use super::path::{check_name, UNSAFE_NAME};
use super::roots::RootSet;

pub(crate) const STALE_ENTRY: &str = "the entry changed since it was listed";
pub(crate) const RENAME_FAILED: &str = "the entry could not be renamed";

pub(crate) fn rename_entry(
    roots: &RootSet,
    id: u64,
    rel: &str,
    name: &str,
    expected: Option<EntryToken>,
    new_name: &str,
) -> Result<(), Error> {
    check_name(name)?;
    check_name(new_name)?;
    if name == new_name {
        return Ok(());
    }
    #[cfg(unix)]
    {
        use std::os::unix::io::AsFd;
        let dir = roots.open_dir_fd(id, rel)?;
        let before = stat_anchored(&dir, name)?;
        if before.kind == EntryKind::Other && before.token == EntryToken::NONE {
            return Err(Error::InvalidSetting(RENAME_FAILED));
        }
        if let Some(expected) = expected {
            if expected.is_strong()
                && before.token.is_strong()
                && !expected.same_object(before.token)
            {
                return Err(Error::InvalidSetting(STALE_ENTRY));
            }
            if expected.is_strong() && !before.token.is_strong() {
                return Err(Error::InvalidSetting(STALE_ENTRY));
            }
        }
        if same_spelling_different_case(name, new_name)
            && dest_exists_pointing_at_anchored(&dir, new_name, before.token)
        {
            return rename_case_only_anchored(&dir, name, new_name);
        }
        match vnilink_rename::anchored::rename_at_noreplace(dir.as_fd(), name, new_name) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                Err(Error::FileExists)
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                Err(Error::InvalidSetting(RENAME_FAILED))
            }
            Err(_) => Err(Error::InvalidSetting(RENAME_FAILED)),
        }
    }
    #[cfg(not(unix))]
    {
        let (_parent, source) = roots.resolve_parent(id, rel, name)?;
        let (_parent, dest) = roots.resolve_parent(id, rel, new_name)?;

        let before = stat(&source, String::new());
        if before.kind == EntryKind::Other && before.token == EntryToken::NONE {
            return Err(Error::InvalidSetting(RENAME_FAILED));
        }
        if let Some(expected) = expected {
            if expected.is_strong()
                && before.token.is_strong()
                && !expected.same_object(before.token)
            {
                return Err(Error::InvalidSetting(STALE_ENTRY));
            }
            if expected.is_strong() && !before.token.is_strong() {
                return Err(Error::InvalidSetting(STALE_ENTRY));
            }
        }

        if same_spelling_different_case(name, new_name)
            && dest_exists_pointing_at(&dest, before.token)
        {
            return rename_case_only(&source, &dest);
        }

        match vnilink_rename::rename_noreplace(&source, &dest) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                Err(Error::FileExists)
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                Err(Error::InvalidSetting(RENAME_FAILED))
            }
            Err(_) => Err(Error::InvalidSetting(RENAME_FAILED)),
        }
    }
}

#[cfg(unix)]
fn stat_anchored(
    dir: &std::os::unix::io::OwnedFd,
    name: &str,
) -> Result<super::entry::FsEntry, Error> {
    use std::os::unix::io::AsFd;
    match vnilink_rename::anchored::stat_at(dir.as_fd(), name) {
        Ok(st) => {
            let kind = if st.is_symlink {
                EntryKind::Symlink
            } else if st.is_dir {
                EntryKind::Dir
            } else if st.is_file {
                EntryKind::File
            } else {
                EntryKind::Other
            };
            // A symlink source is never renameable through this layer.
            if kind == EntryKind::Symlink {
                return Err(Error::InvalidSetting(RENAME_FAILED));
            }
            Ok(super::entry::FsEntry {
                name: String::new(),
                kind,
                size: if kind == EntryKind::File { st.size } else { 0 },
                mtime_ms: st.mtime_ms,
                token: EntryToken {
                    dev: Some(st.dev),
                    ino: Some(st.ino),
                },
            })
        }
        Err(_) => Err(Error::InvalidSetting(RENAME_FAILED)),
    }
}

#[cfg(unix)]
fn dest_exists_pointing_at_anchored(
    dir: &std::os::unix::io::OwnedFd,
    dest: &str,
    source: EntryToken,
) -> bool {
    let Ok(after) = stat_anchored(dir, dest) else {
        return false;
    };
    source.is_strong() && after.token.is_strong() && source.same_object(after.token)
}

#[cfg(unix)]
fn rename_case_only_anchored(
    dir: &std::os::unix::io::OwnedFd,
    source: &str,
    dest: &str,
) -> Result<(), Error> {
    use std::os::unix::io::AsFd;
    for attempt in 0..8u32 {
        let temp = format!(".vnlcase.{attempt}.tmp");
        // Skip occupied temp names without touching them.
        if stat_anchored(dir, &temp).is_ok() {
            continue;
        }
        vnilink_rename::anchored::rename_at_noreplace(dir.as_fd(), source, &temp)
            .map_err(|_| Error::InvalidSetting(RENAME_FAILED))?;
        match vnilink_rename::anchored::rename_at_noreplace(dir.as_fd(), &temp, dest) {
            Ok(()) => return Ok(()),
            Err(_) => {
                let _ = vnilink_rename::anchored::rename_at_noreplace(dir.as_fd(), &temp, source);
                return Err(Error::InvalidSetting(RENAME_FAILED));
            }
        }
    }
    Err(Error::InvalidSetting(RENAME_FAILED))
}

fn same_spelling_different_case(a: &str, b: &str) -> bool {
    a.len() == b.len() && a.to_lowercase() == b.to_lowercase()
}

/// True when the destination already resolves to the source object itself,
/// i.e. a case-only rename on a case-insensitive filesystem where both
/// spellings address one entry.
#[cfg(not(unix))]
fn dest_exists_pointing_at(dest: &std::path::Path, source: EntryToken) -> bool {
    let after = stat(dest, String::new());
    source.is_strong() && after.token.is_strong() && source.same_object(after.token)
}

/// Moves aside through a unique temporary sibling, then onto the new
/// spelling. Either step failing leaves the temporary name for a later
/// sweep; the original bytes are never copied.
#[cfg(not(unix))]
fn rename_case_only(source: &std::path::Path, dest: &std::path::Path) -> Result<(), Error> {
    let parent = source.parent().ok_or(Error::InvalidSetting(UNSAFE_NAME))?;
    for attempt in 0..8u32 {
        let temp = parent.join(format!(".vnlcase.{attempt}.tmp"));
        if std::fs::symlink_metadata(&temp).is_ok() {
            continue;
        }
        std::fs::rename(source, &temp).map_err(|_| Error::InvalidSetting(RENAME_FAILED))?;
        match std::fs::rename(&temp, dest) {
            Ok(()) => return Ok(()),
            Err(_) => {
                // Best effort: move back so the entry keeps a stable name.
                let _ = std::fs::rename(&temp, source);
                return Err(Error::InvalidSetting(RENAME_FAILED));
            }
        }
    }
    Err(Error::InvalidSetting(RENAME_FAILED))
}
