//! Deletion isolates a listed object before destructive cleanup. It moves
//! into an exclusively created private directory once its identity is
//! verified (there on Unix; on the handle that moves it on Windows); the
//! cleanup retains directory handles and never follows child links. A
//! failed cleanup leaves a recovery namespace instead of sweeping public
//! names.

use crate::error::Error;

use super::entry::{EntryKind, EntryToken};
use super::path::check_name;
use super::rename::STALE_ENTRY;
use super::roots::RootSet;

pub(crate) const DELETE_FAILED: &str = "the entry could not be deleted";
pub(crate) const DELETE_INCOMPLETE: &str =
    "the folder was removed, but some of its contents remain";

/// Work left after the fenced mutation: the isolated object, to remove
/// once the caller has dropped its locks.
pub(crate) enum DeleteWork {
    #[cfg(any(unix, windows))]
    Isolated(vnilink_rename::anchored::DeleteCandidate),
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
    vnilink_rename::anchored::DeleteCandidate::stage(parent.as_dir(), name, expected)
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
fn listed_at(
    parent: &vnilink_rename::anchored::Dir,
    name: &str,
) -> Result<(EntryKind, EntryToken), Error> {
    use vnilink_rename::anchored::AsDir;

    let st = vnilink_rename::anchored::stat_at(parent.as_dir(), name)
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
