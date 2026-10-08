//! Create one directory level. The parent must already exist; nesting is
//! built one level per call so each commit point stays explicit. Creation
//! is anchored: the parent is opened as a directory handle (`O_NOFOLLOW`)
//! and the new level is committed with `mkdirat`, so a parent replaced by
//! a symlink cannot redirect the write outside the approved root.

use crate::error::Error;

use super::path::check_name;
use super::roots::RootSet;

pub(crate) fn create_dir(roots: &RootSet, id: u64, rel: &str, name: &str) -> Result<(), Error> {
    check_name(name)?;
    #[cfg(unix)]
    {
        use std::os::unix::io::AsFd;
        let parent = roots.open_dir_fd(id, rel)?;
        match vnilink_rename::anchored::mkdir_at(parent.as_fd(), name) {
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
    #[cfg(not(unix))]
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
