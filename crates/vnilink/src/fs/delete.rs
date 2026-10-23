//! Deletion isolates a listed object before destructive cleanup. Unix moves
//! it into an exclusively created private directory and verifies its identity
//! there; the cleanup retains directory handles and never follows child links.
//! A failed cleanup leaves a recovery namespace instead of sweeping public names.

use crate::error::Error;

#[cfg(windows)]
use super::entry::stat;
use super::entry::{EntryKind, EntryToken};
use super::path::check_name;
use super::rename::STALE_ENTRY;
use super::roots::RootSet;

pub(crate) const DELETE_FAILED: &str = "the entry could not be deleted";
pub(crate) const DELETE_INCOMPLETE: &str =
    "the folder was removed, but some of its contents remain";

#[cfg(windows)]
const ASIDE_ATTEMPTS: u32 = 8;

/// Work left after the fenced mutation. `Done` means the name is already
/// gone. `Tree` is the temporary folder to remove once the caller has
/// dropped its locks.
pub(crate) enum DeleteWork {
    #[cfg(windows)]
    Done,
    #[cfg(unix)]
    Isolated(vnilink_rename::anchored::DeleteCandidate),
    #[cfg(windows)]
    Tree {
        path: std::path::PathBuf,
        dev: u64,
        ino: u64,
    },
}

/// Promotes only the two static delete outcomes. Any other peer text stays
/// the generic refusal, so a response cannot inject a new error string.
pub(crate) fn failure_message(message: &str) -> &'static str {
    if message == DELETE_FAILED {
        DELETE_FAILED
    } else if message == DELETE_INCOMPLETE {
        DELETE_INCOMPLETE
    } else {
        "the remote filesystem refused the request"
    }
}

pub(crate) fn stage_delete(
    roots: &RootSet,
    id: u64,
    rel: &str,
    name: &str,
    expected: Option<EntryToken>,
) -> Result<DeleteWork, Error> {
    check_name(name)?;
    #[cfg(unix)]
    {
        stage_anchored(roots, id, rel, name, expected)
    }
    #[cfg(windows)]
    {
        stage_path(roots, id, rel, name, expected)
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (roots, id, rel, name, expected);
        Err(Error::Unsupported("native deletion is unavailable"))
    }
}

pub(crate) fn finish_delete(work: DeleteWork) -> Result<(), Error> {
    match work {
        #[cfg(windows)]
        DeleteWork::Done => Ok(()),
        #[cfg(unix)]
        DeleteWork::Isolated(candidate) => candidate
            .finish()
            .map_err(|_| Error::InvalidSetting(DELETE_INCOMPLETE)),
        #[cfg(windows)]
        DeleteWork::Tree { path, dev, ino } => finish_windows(&path, dev, ino),
    }
}

fn accept_listed(
    kind: EntryKind,
    token: EntryToken,
    expected: Option<EntryToken>,
) -> Result<(), Error> {
    // A listed symlink is the object the user pointed at. Removing it would
    // be a different operation from cleaning children inside a real folder
    // that was already moved aside.
    if kind == EntryKind::Symlink || (kind == EntryKind::Other && token == EntryToken::NONE) {
        return Err(Error::InvalidSetting(DELETE_FAILED));
    }
    if let Some(expected) = expected {
        let replaced = expected.is_strong()
            && ((token.is_strong() && !expected.same_object(token)) || !token.is_strong());
        if replaced {
            return Err(Error::InvalidSetting(STALE_ENTRY));
        }
    }
    Ok(())
}

#[cfg(unix)]
fn stage_anchored(
    roots: &RootSet,
    id: u64,
    rel: &str,
    name: &str,
    expected: Option<EntryToken>,
) -> Result<DeleteWork, Error> {
    use std::os::unix::io::AsFd;

    let parent = roots.open_dir_fd(id, rel)?;
    let (kind, token) = listed_at(&parent, name)?;
    accept_listed(kind, token, expected)?;
    let (dev, ino) = token
        .dev
        .zip(token.ino)
        .ok_or(Error::InvalidSetting(DELETE_FAILED))?;
    let expected = vnilink_rename::anchored::Identity {
        dev,
        ino,
        born: token.born,
    };
    vnilink_rename::anchored::DeleteCandidate::stage(parent.as_fd(), name, expected)
        .map(DeleteWork::Isolated)
        .map_err(|error| {
            Error::InvalidSetting(if error.kind() == std::io::ErrorKind::InvalidData {
                STALE_ENTRY
            } else {
                DELETE_FAILED
            })
        })
}

#[cfg(unix)]
fn listed_at(
    parent: &std::os::unix::io::OwnedFd,
    name: &str,
) -> Result<(EntryKind, EntryToken), Error> {
    use std::os::unix::io::AsFd;

    let st = vnilink_rename::anchored::stat_at(parent.as_fd(), name)
        .map_err(|_| Error::InvalidSetting(DELETE_FAILED))?;
    let kind = if st.is_symlink {
        EntryKind::Symlink
    } else if st.is_dir {
        EntryKind::Dir
    } else if st.is_file {
        EntryKind::File
    } else {
        EntryKind::Other
    };
    Ok((
        kind,
        EntryToken {
            dev: Some(st.dev),
            ino: Some(st.ino),
            born: st.born,
        },
    ))
}

#[cfg(windows)]
fn stage_path(
    roots: &RootSet,
    id: u64,
    rel: &str,
    name: &str,
    expected: Option<EntryToken>,
) -> Result<DeleteWork, Error> {
    let (parent, source) = roots.resolve_parent(id, rel, name)?;
    let before = stat(&source, String::new());
    accept_listed(before.kind, before.token, expected)?;
    #[cfg(windows)]
    let identity = before
        .token
        .dev
        .zip(before.token.ino)
        .ok_or(Error::InvalidSetting(DELETE_FAILED))?;
    if before.kind != EntryKind::Dir {
        #[cfg(windows)]
        vnilink_rename::windows::delete_tree_checked(&source, identity)
            .map_err(|_| Error::InvalidSetting(DELETE_FAILED))?;
        return Ok(DeleteWork::Done);
    }
    // Pin the directory object before the rename. The walk rechecks this
    // identity so a replacement at the temporary name is left alone.
    let path = aside_path(&parent, &source)?;
    #[cfg(windows)]
    {
        Ok(DeleteWork::Tree {
            path,
            dev: identity.0,
            ino: identity.1,
        })
    }
}

#[cfg(windows)]
fn aside_path(
    parent: &std::path::Path,
    source: &std::path::Path,
) -> Result<std::path::PathBuf, Error> {
    for attempt in 0..ASIDE_ATTEMPTS {
        let temp = parent.join(format!(".vnldelete.{attempt}"));
        if std::fs::symlink_metadata(&temp).is_ok() {
            continue;
        }
        match vnilink_rename::rename_noreplace(source, &temp) {
            Ok(()) => return Ok(temp),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(_) => return Err(Error::InvalidSetting(DELETE_FAILED)),
        }
    }
    Err(Error::InvalidSetting(DELETE_FAILED))
}

#[cfg(windows)]
fn finish_windows(path: &std::path::Path, dev: u64, ino: u64) -> Result<(), Error> {
    if vnilink_rename::windows::delete_tree_checked(path, (dev, ino)).is_ok() {
        return Ok(());
    }
    leftover(path)
}

#[cfg(windows)]
fn leftover(path: &std::path::Path) -> Result<(), Error> {
    match std::fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        _ => Err(Error::InvalidSetting(DELETE_INCOMPLETE)),
    }
}
