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
    // The system's Recycle Bin takes a path and resolves it again, following
    // a junction swapped into a parent meanwhile; an object outside the
    // approved folder could be recycled. In a shared folder Windows offers
    // permanent deletion instead, which works through handles. The whole
    // disk, where nothing is outside, keeps its Recycle Bin.
    #[cfg(windows)]
    if !roots.whole_filesystem() {
        return Err(Error::Unsupported(NO_TRASH));
    }
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

#[cfg(all(test, windows))]
mod windows_tests {
    use super::*;

    /// A shared folder offers no Recycle Bin on Windows: the shell would
    /// resolve the path again, through any junction swapped in meanwhile.
    #[test]
    fn a_shared_folder_refuses_the_recycle_bin_before_touching_anything() {
        let scratch = crate::test_support::Scratch::tagged("trash");
        let root = scratch.join("root");
        std::fs::create_dir(&root).unwrap();
        std::fs::write(root.join("kept.txt"), b"kept").unwrap();
        let mut roots = RootSet::default();
        roots.set(std::slice::from_ref(&root)).unwrap();
        assert!(matches!(
            stage_trash(&roots, 0, "", "kept.txt", None),
            Err(Error::Unsupported(message)) if message == NO_TRASH
        ));
        assert_eq!(std::fs::read(root.join("kept.txt")).unwrap(), b"kept");
    }
}
