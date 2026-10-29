//! Create one directory level. The parent must already exist; nesting is
//! built one level per call so each commit point stays explicit. Creation
//! is anchored: the parent is opened as a directory handle without
//! following links and the new level is created relative to it (`mkdirat`,
//! or `NtCreateFile` on Windows), so a parent replaced by a link after it
//! was resolved cannot redirect the write.

use crate::error::Error;

use super::path::check_name;
use super::roots::RootSet;

pub(crate) fn create_dir(roots: &RootSet, id: u64, rel: &str, name: &str) -> Result<(), Error> {
    check_name(name)?;
    #[cfg(any(unix, windows))]
    {
        use vnilink_rename::anchored::AsDir;
        let parent = roots.open_dir_held(id, rel)?;
        match vnilink_rename::anchored::mkdir_at(parent.as_dir(), name) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                Err(Error::FileExists)
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                Err(Error::InvalidSetting("the parent folder is missing"))
            }
            Err(_) => Err(Error::InvalidSetting("the folder could not be created")),
        }
    }
    #[cfg(not(any(unix, windows)))]
    {
        let (_parent, dest) = roots.resolve_parent(id, rel, name)?;
        match std::fs::create_dir(&dest) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                Err(Error::FileExists)
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                Err(Error::InvalidSetting("the parent folder is missing"))
            }
            Err(_) => Err(Error::InvalidSetting("the folder could not be created")),
        }
    }
}
