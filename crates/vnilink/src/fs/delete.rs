//! Deletion isolates a listed object before destructive cleanup. It moves
//! into an exclusively created private directory once its identity is
//! verified (there on Unix; on the handle that moves it on Windows); the
//! cleanup retains directory handles and never follows child links. A
//! failed cleanup leaves a recovery namespace instead of sweeping public
//! names.

use crate::error::Error;

use super::entry::{EntryKind, EntryToken};
use super::path::check_name;
use super::rename::{still_listed, STALE_ENTRY};
use super::roots::RootSet;

pub(crate) const DELETE_FAILED: &str = "the entry could not be deleted";
pub(crate) const DELETE_INCOMPLETE: &str =
    "the folder was removed, but some of its contents remain";

/// Work left after the fenced mutation: the isolated object, to remove
/// once the caller has dropped its locks.
pub(crate) enum DeleteWork {
    #[cfg(any(unix, windows))]
    Isolated(vnilink_rename::anchored::DeleteCandidate),
    /// A file already removed, where ids are positions.
    #[cfg(any(unix, windows))]
    Removed,
}

/// Promotes only the static delete outcomes. Any other peer text stays
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
    #[cfg(any(unix, windows))]
    {
        stage_anchored(roots, id, rel, name, expected)
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (roots, id, rel, name, expected);
        Err(Error::Unsupported("native deletion is unavailable"))
    }
}

pub(crate) fn finish_delete(work: DeleteWork) -> Result<(), Error> {
    match work {
        #[cfg(any(unix, windows))]
        DeleteWork::Isolated(candidate) => candidate
            .finish()
            .map_err(|_| Error::InvalidSetting(DELETE_INCOMPLETE)),
        #[cfg(any(unix, windows))]
        DeleteWork::Removed => Ok(()),
    }
}

/// Whether a delete may act on `name`, of `kind` and now tokened
/// `current`, listed as `expected`. A listed link is the object the user
/// pointed at; removing it would be a different operation from cleaning
/// children inside a real folder that was already moved aside. Where ids
/// are positions a folder is refused: no fingerprint covers what it holds.
fn accept_listed(
    kind: EntryKind,
    current: EntryToken,
    expected: Option<EntryToken>,
    positional: bool,
) -> Result<(), Error> {
    if kind == EntryKind::Symlink || (kind == EntryKind::Other && current == EntryToken::NONE) {
        return Err(Error::InvalidSetting(DELETE_FAILED));
    }
    if positional && kind == EntryKind::Dir {
        return Err(Error::UnprovenEntry);
    }
    still_listed(expected, current, positional)
}

#[cfg(any(unix, windows))]
fn stage_anchored(
    roots: &RootSet,
    id: u64,
    rel: &str,
    name: &str,
    expected: Option<EntryToken>,
) -> Result<DeleteWork, Error> {
    use vnilink_rename::anchored::AsDir;

    let parent = roots.open_dir_held(id, rel)?;
    let ids = super::rename::volume_ids(&parent);
    let st = vnilink_rename::anchored::stat_at(parent.as_dir(), name)
        .map_err(|_| Error::InvalidSetting(DELETE_FAILED))?;
    let positional = ids != vnilink_rename::VolumeIds::Stable;
    accept_listed(
        entry_kind(&st),
        super::entry::token_of(name, &st, ids),
        expected,
        positional,
    )?;
    // Where ids are positions a file moved aside takes another id, so it is
    // not staged and checked by id: its fingerprint is checked again on the
    // very object removed (the deleting handle on Windows, the entry moved
    // aside under a private name on Unix) and it is removed at once.
    if positional {
        return vnilink_rename::anchored::delete_file_checked(parent.as_dir(), name, |stated| {
            still_listed(expected, super::entry::token_of(name, stated, ids), true).is_ok()
        })
        .map(|()| DeleteWork::Removed)
        .map_err(|error| {
            Error::InvalidSetting(if error.kind() == std::io::ErrorKind::InvalidData {
                STALE_ENTRY
            } else {
                DELETE_FAILED
            })
        });
    }
    // Moved aside only while it is still the object just stated; where ids
    // are positions, the entry at the position just fingerprinted.
    vnilink_rename::anchored::DeleteCandidate::stage(parent.as_dir(), name, st.identity())
        .map(DeleteWork::Isolated)
        .map_err(|error| {
            Error::InvalidSetting(if error.kind() == std::io::ErrorKind::InvalidData {
                STALE_ENTRY
            } else {
                DELETE_FAILED
            })
        })
}

#[cfg(any(unix, windows))]
pub(crate) fn entry_kind(st: &vnilink_rename::anchored::EntryStat) -> EntryKind {
    if st.is_symlink {
        EntryKind::Symlink
    } else if st.is_dir {
        EntryKind::Dir
    } else if st.is_file {
        EntryKind::File
    } else {
        EntryKind::Other
    }
}
