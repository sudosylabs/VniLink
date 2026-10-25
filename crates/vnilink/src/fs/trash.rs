//! Moving an entry to this computer's Trash: the delete that can be undone
//! from the Trash or Recycle Bin. The entry is checked as delete checks it,
//! still the object that was listed and inside an approved root, and then
//! handed to the system's own Trash.

use std::path::PathBuf;

use super::entry::stat;
use super::rename::STALE_ENTRY;
use super::{check_name, EntryKind, EntryToken, RootSet};
use crate::error::Error;

/// The place has no Trash (a network share, some removable drives) or the
/// system refused to move the entry there.
pub(crate) const NO_TRASH: &str = "the entry can't be moved to the Trash";

/// Resolves `name` in its folder and checks it is still the listed object.
/// Returns the path to move. A link is moved itself, as the file manager
/// shows it, never what it points to.
pub(crate) fn stage_trash(
    roots: &RootSet,
    id: u64,
    rel: &str,
    name: &str,
    expected: Option<EntryToken>,
) -> Result<PathBuf, Error> {
    check_name(name)?;
    let (_parent, full) = roots.resolve_parent(id, rel, name)?;
    let entry = stat(&full, name.to_owned());
    if entry.kind == EntryKind::Other && entry.token == EntryToken::NONE {
        return Err(Error::InvalidSetting(STALE_ENTRY));
    }
    if let Some(expected) = expected {
        let token = entry.token;
        if expected.is_strong() && (!token.is_strong() || !expected.same_object(token)) {
            return Err(Error::InvalidSetting(STALE_ENTRY));
        }
    }
    Ok(full)
}

/// Moves `path` to the Trash. Runs on a thread of its own: on Windows the
/// Recycle Bin is reached through COM, which this thread initializes for
/// itself rather than inheriting whatever its caller's thread chose.
pub(crate) fn finish_trash(path: PathBuf) -> Result<(), Error> {
    std::thread::Builder::new()
        .name("vnilink-trash".into())
        .spawn(crate::trace::within(tracing::Span::current(), move || {
            move_to_trash(&path)
        }))
        .map_err(|_| Error::Unsupported(NO_TRASH))?
        .join()
        .map_err(|_| Error::Unsupported(NO_TRASH))?
}

fn move_to_trash(path: &std::path::Path) -> Result<(), Error> {
    #[allow(unused_mut)]
    let mut context = trash::TrashContext::default();
    // NSFileManager, not Finder: Finder is driven by AppleScript, which
    // asks the user to let VniLink control it.
    #[cfg(target_os = "macos")]
    {
        use trash::macos::{DeleteMethod, TrashContextExtMacos};
        context.set_delete_method(DeleteMethod::NsFileManager);
    }
    context
        .delete(path)
        .map_err(|_| Error::Unsupported(NO_TRASH))
}
