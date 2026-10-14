//! Identity-fenced delete for one listed file or folder.
//!
//! The listing token is rechecked against the object at that name, the same
//! way rename does: a replacement is refused, a listed symlink is refused,
//! and a strong token that can no longer be confirmed is refused. A file is
//! then unlinked in place. A folder is renamed, without replacement, to a
//! sibling `.vnldelete.N` while the caller still holds its locks. The tree
//! walk runs only after those locks drop, and it does not follow links: a
//! child symlink is removed as a link and its target stays. If the walk
//! stops early, the original name is already gone and the temporary folder
//! remains.

use crate::error::Error;

#[cfg(not(unix))]
use super::entry::stat;
use super::entry::{EntryKind, EntryToken};
use super::path::check_name;
use super::rename::STALE_ENTRY;
use super::roots::RootSet;

pub(crate) const DELETE_FAILED: &str = "the entry could not be deleted";
pub(crate) const DELETE_INCOMPLETE: &str =
    "the folder was removed, but some of its contents remain";

const ASIDE_ATTEMPTS: u32 = 8;
#[cfg(not(unix))]
const MAX_DEPTH: usize = 64;
#[cfg(not(unix))]
const MAX_ENTRIES: usize = 10_000;

/// Work left after the fenced mutation. `Done` means the name is already
/// gone. `Tree` is the temporary folder to remove once the caller has
/// dropped its locks.
pub(crate) enum DeleteWork {
    Done,
    #[cfg(unix)]
    Tree {
        parent: std::os::unix::io::OwnedFd,
        name: String,
    },
    #[cfg(windows)]
    Tree {
        path: std::path::PathBuf,
        dev: u64,
        ino: u64,
    },
    #[cfg(not(any(unix, windows)))]
    Tree {
        path: std::path::PathBuf,
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
    #[cfg(not(unix))]
    {
        stage_path(roots, id, rel, name, expected)
    }
}

pub(crate) fn finish_delete(work: DeleteWork) -> Result<(), Error> {
    match work {
        DeleteWork::Done => Ok(()),
        #[cfg(unix)]
        DeleteWork::Tree { parent, name } => finish_anchored(&parent, &name),
        #[cfg(windows)]
        DeleteWork::Tree { path, dev, ino } => finish_windows(&path, dev, ino),
        #[cfg(not(any(unix, windows)))]
        DeleteWork::Tree { path } => finish_path(&path),
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
    if kind != EntryKind::Dir {
        vnilink_rename::anchored::remove_tree_at(parent.as_fd(), name)
            .map_err(|_| Error::InvalidSetting(DELETE_FAILED))?;
        return Ok(DeleteWork::Done);
    }
    let aside = aside_anchored(&parent, name)?;
    Ok(DeleteWork::Tree {
        parent,
        name: aside,
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
        },
    ))
}

#[cfg(unix)]
fn aside_anchored(parent: &std::os::unix::io::OwnedFd, name: &str) -> Result<String, Error> {
    use std::os::unix::io::AsFd;

    for attempt in 0..ASIDE_ATTEMPTS {
        let temp = format!(".vnldelete.{attempt}");
        if vnilink_rename::anchored::stat_at(parent.as_fd(), &temp).is_ok() {
            continue;
        }
        match vnilink_rename::anchored::rename_at_noreplace(parent.as_fd(), name, &temp) {
            Ok(()) => return Ok(temp),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(_) => return Err(Error::InvalidSetting(DELETE_FAILED)),
        }
    }
    Err(Error::InvalidSetting(DELETE_FAILED))
}

#[cfg(unix)]
fn finish_anchored(parent: &std::os::unix::io::OwnedFd, name: &str) -> Result<(), Error> {
    use std::os::unix::io::AsFd;

    if vnilink_rename::anchored::remove_tree_at(parent.as_fd(), name).is_ok() {
        return Ok(());
    }
    // The listed name is already gone. Success only when the temporary
    // folder is gone too; a busy child leaves it for another attempt.
    match vnilink_rename::anchored::stat_at(parent.as_fd(), name) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        _ => Err(Error::InvalidSetting(DELETE_INCOMPLETE)),
    }
}

#[cfg(not(unix))]
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
    if before.kind != EntryKind::Dir {
        std::fs::remove_file(&source).map_err(|_| Error::InvalidSetting(DELETE_FAILED))?;
        return Ok(DeleteWork::Done);
    }
    // Pin the directory object before the rename. The walk rechecks this
    // identity so a replacement at the temporary name is left alone.
    #[cfg(windows)]
    let identity = vnilink_rename::windows::entry_identity(&source)
        .map_err(|_| Error::InvalidSetting(DELETE_FAILED))?;
    let path = aside_path(&parent, &source)?;
    #[cfg(windows)]
    {
        Ok(DeleteWork::Tree {
            path,
            dev: identity.0,
            ino: identity.1,
        })
    }
    #[cfg(not(windows))]
    {
        Ok(DeleteWork::Tree { path })
    }
}

#[cfg(not(unix))]
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
    if remove_tree(path, Some((dev, ino))).is_ok() {
        return Ok(());
    }
    leftover(path)
}

#[cfg(not(any(unix, windows)))]
fn finish_path(path: &std::path::Path) -> Result<(), Error> {
    if remove_tree(path, None).is_ok() {
        return Ok(());
    }
    leftover(path)
}

#[cfg(not(unix))]
fn leftover(path: &std::path::Path) -> Result<(), Error> {
    match std::fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        _ => Err(Error::InvalidSetting(DELETE_INCOMPLETE)),
    }
}

/// Removes `path` without following a reparse point or symlink. `expect`
/// is the directory object pinned at the aside; a different object at that
/// path is left in place. Bounds match `anchored::remove_tree_at`.
#[cfg(not(unix))]
fn remove_tree(path: &std::path::Path, expect: Option<(u64, u64)>) -> std::io::Result<()> {
    let volume = expect.map(|(dev, _)| dev);
    let mut remaining = MAX_ENTRIES;
    remove_node(path, 0, &mut remaining, volume, expect)
}

#[cfg(not(unix))]
fn remove_node(
    path: &std::path::Path,
    depth: usize,
    remaining: &mut usize,
    volume: Option<u64>,
    expect: Option<(u64, u64)>,
) -> std::io::Result<()> {
    if depth > MAX_DEPTH || *remaining == 0 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "managed subtree exceeds cleanup bounds",
        ));
    }
    *remaining -= 1;
    let meta = std::fs::symlink_metadata(path)?;
    if is_reparse(&meta) || !meta.is_dir() {
        if expect.is_some() {
            confirm_object(path, expect)?;
        }
        return unlink_leaf(path, &meta);
    }
    let child_volume = confirm_directory(path, volume, expect)?;
    for entry in std::fs::read_dir(path)? {
        let entry = entry?;
        remove_node(&entry.path(), depth + 1, remaining, child_volume, None)?;
    }
    confirm_object(path, expect)?;
    std::fs::remove_dir(path)
}

#[cfg(not(unix))]
fn confirm_object(path: &std::path::Path, expect: Option<(u64, u64)>) -> std::io::Result<()> {
    let Some(expect) = expect else {
        return Ok(());
    };
    #[cfg(windows)]
    {
        if vnilink_rename::windows::entry_identity(path)? != expect {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "managed subtree changed",
            ));
        }
        return Ok(());
    }
    #[cfg(not(windows))]
    {
        let _ = (path, expect);
        Ok(())
    }
}

#[cfg(not(unix))]
fn confirm_directory(
    path: &std::path::Path,
    volume: Option<u64>,
    expect: Option<(u64, u64)>,
) -> std::io::Result<Option<u64>> {
    #[cfg(windows)]
    {
        let (dev, ino) = vnilink_rename::windows::entry_identity(path)?;
        if let Some(volume) = volume {
            if dev != volume {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "managed subtree crosses a volume",
                ));
            }
        }
        if let Some(expect) = expect {
            if (dev, ino) != expect {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "managed subtree changed",
                ));
            }
        }
        return Ok(Some(dev));
    }
    #[cfg(not(windows))]
    {
        let _ = (path, volume, expect);
        Ok(volume)
    }
}

#[cfg(not(unix))]
fn is_reparse(meta: &std::fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if meta.file_attributes() & 0x400 != 0 {
            return true;
        }
    }
    meta.file_type().is_symlink()
}

#[cfg(not(unix))]
fn unlink_leaf(path: &std::path::Path, meta: &std::fs::Metadata) -> std::io::Result<()> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        // A directory junction can reject DeleteFile. RemoveDirectory drops
        // the link itself and does not list the target.
        Err(error) if is_reparse(meta) => match std::fs::remove_dir(path) {
            Ok(()) => Ok(()),
            Err(_) => Err(error),
        },
        Err(error) => Err(error),
    }
}
