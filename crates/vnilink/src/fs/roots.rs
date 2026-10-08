//! Owner-approved filesystem roots.
//!
//! The set starts empty and is replaced wholesale by the owner. A root id
//! is its position at set time. All access is anchored to directory handles:
//! the root is opened `O_NOFOLLOW|O_DIRECTORY` and pinned to its
//! device+inode, then every component is opened via `openat` with
//! `O_NOFOLLOW`. A symlink anywhere on the resolved path, including a
//! parent swapped after listing or the root itself replaced by a symlink,
//! refuses the operation instead of escaping the approved folder.

use std::path::{Path, PathBuf};

use crate::error::Error;

use super::path::{check_rel, UNSAFE_PATH};

/// Largest number of approved roots.
pub const MAX_ROOTS: usize = 16;

pub(crate) const NO_ROOTS: &str = "no file roots are set";
pub(crate) const UNKNOWN_ROOT: &str = "the file root is unknown";
pub(crate) const NOT_A_DIRECTORY: &str = "the path is not a directory";

#[derive(Debug, Clone)]
struct Root {
    path: PathBuf,
    #[allow(dead_code)]
    dev: u64,
    #[allow(dead_code)]
    ino: u64,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct RootSet {
    roots: Vec<Root>,
}

impl RootSet {
    pub(crate) fn set(&mut self, paths: &[PathBuf]) -> Result<(), Error> {
        if paths.len() > MAX_ROOTS {
            return Err(Error::InvalidSetting("too many file roots"));
        }
        let mut roots = Vec::with_capacity(paths.len());
        for path in paths {
            let meta = std::fs::symlink_metadata(path)
                .map_err(|_| Error::InvalidSetting("a file root is not usable"))?;
            if meta.file_type().is_symlink() || !meta.is_dir() {
                return Err(Error::InvalidSetting("a file root is not usable"));
            }
            let canonical = std::fs::canonicalize(path)
                .map_err(|_| Error::InvalidSetting("a file root is not usable"))?;
            let meta = std::fs::symlink_metadata(&canonical)
                .map_err(|_| Error::InvalidSetting("a file root is not usable"))?;
            if !meta.is_dir() {
                return Err(Error::InvalidSetting("a file root is not usable"));
            }
            if roots.iter().any(|root: &Root| root.path == canonical) {
                return Err(Error::InvalidSetting("a file root is listed twice"));
            }
            #[cfg(unix)]
            {
                // Pin the directory object itself: open O_NOFOLLOW|O_DIRECTORY
                // and record device+inode so a later replacement (symlink or
                // different directory at the same path) is refused on every use.
                use std::os::unix::io::AsFd;
                let fd = vnilink_rename::anchored::open_root(&canonical)
                    .map_err(|_| Error::InvalidSetting("a file root is not usable"))?;
                let (dev, ino) = vnilink_rename::anchored::fstat_dev_ino(fd.as_fd())
                    .map_err(|_| Error::InvalidSetting("a file root is not usable"))?;
                roots.push(Root {
                    path: canonical,
                    dev,
                    ino,
                });
            }
            #[cfg(not(unix))]
            {
                roots.push(Root {
                    path: canonical,
                    dev: 0,
                    ino: 0,
                });
            }
        }
        self.roots = roots;
        Ok(())
    }

    pub(crate) fn snapshot(&self) -> Vec<(u64, PathBuf)> {
        self.roots
            .iter()
            .enumerate()
            .map(|(index, root)| (index as u64, root.path.clone()))
            .collect()
    }

    fn root(&self, id: u64) -> Result<&Root, Error> {
        if self.roots.is_empty() {
            return Err(Error::Unsupported(NO_ROOTS));
        }
        self.roots
            .get(id as usize)
            .ok_or(Error::InvalidSetting(UNKNOWN_ROOT))
    }

    /// Opens the root itself anchored and pinned: `O_NOFOLLOW|O_DIRECTORY`
    /// plus device+inode equality against the stored object. A root replaced
    /// by a symlink, or by a different directory at the same path (including
    /// via a swapped parent), fails here.
    #[cfg(unix)]
    pub(crate) fn open_root_fd(&self, id: u64) -> Result<std::os::unix::io::OwnedFd, Error> {
        use std::os::unix::io::AsFd;
        let root = self.root(id)?;
        let fd = vnilink_rename::anchored::open_root(&root.path)
            .map_err(|_| Error::InvalidSetting(UNSAFE_PATH))?;
        let (dev, ino) = vnilink_rename::anchored::fstat_dev_ino(fd.as_fd())
            .map_err(|_| Error::InvalidSetting(UNSAFE_PATH))?;
        if dev != root.dev || ino != root.ino {
            return Err(Error::InvalidSetting(UNSAFE_PATH));
        }
        Ok(fd)
    }

    /// Opens `rel` under root `id` as a directory handle, walking every
    /// component via `openat(O_NOFOLLOW|O_DIRECTORY)`. Any symlink, file or
    /// missing component refuses.
    #[cfg(unix)]
    pub(crate) fn open_dir_fd(
        &self,
        id: u64,
        rel: &str,
    ) -> Result<std::os::unix::io::OwnedFd, Error> {
        use std::os::unix::io::AsFd;
        check_rel(rel)?;
        let mut current = self.open_root_fd(id)?;
        if !rel.is_empty() {
            for component in rel.split('/') {
                let child = vnilink_rename::anchored::open_child(current.as_fd(), component)
                    .map_err(|_| Error::InvalidSetting(UNSAFE_PATH))?;
                current = child;
            }
        }
        Ok(current)
    }

    /// Resolves `rel` under root `id` to an absolute path that is still a
    /// real directory chain. Every component, including the root itself, is
    /// verified through directory handles at resolve time; callers that
    /// mutate must use the `*_fd` handles above so the check and the use
    /// share one anchored object instead of a pathname race.
    pub(crate) fn resolve_dir(&self, id: u64, rel: &str) -> Result<PathBuf, Error> {
        check_rel(rel)?;
        #[cfg(unix)]
        {
            // Verify through handles first: root pinning plus per-component
            // NOFOLLOW opens. The returned path is for compatible readers;
            // mutating callers use `open_dir_fd`/`open_root_fd` directly.
            let _verified = self.open_dir_fd(id, rel)?;
            let root = self.root(id)?;
            let mut current = root.path.clone();
            if !rel.is_empty() {
                for component in rel.split('/') {
                    current.push(component);
                }
            }
            Ok(current)
        }
        #[cfg(not(unix))]
        {
            let root = self.root(id)?;
            let mut current = root.path.clone();
            if !rel.is_empty() {
                for component in rel.split('/') {
                    current.push(component);
                    if !under_root(&root.path, &current) {
                        return Err(Error::InvalidSetting(UNSAFE_PATH));
                    }
                    ensure_real_dir(&current)?;
                }
            }
            // Non-Unix still revalidates the root itself (symlink check).
            ensure_real_dir(&root.path)?;
            Ok(current)
        }
    }

    /// Resolves the parent chain of a to-be-created entry and appends the
    /// unchecked final name. The caller validates the name and performs the
    /// creation with no-replace semantics through directory handles.
    pub(crate) fn resolve_parent(
        &self,
        id: u64,
        rel: &str,
        name: &str,
    ) -> Result<(PathBuf, PathBuf), Error> {
        check_rel(rel)?;
        let parent = self.resolve_dir(id, rel)?;
        let dest = parent.join(name);
        let root = self.root(id)?;
        if !under_root(&root.path, &dest) {
            return Err(Error::InvalidSetting(UNSAFE_PATH));
        }
        Ok((parent, dest))
    }
}

fn under_root(root: &Path, path: &Path) -> bool {
    if path == root {
        return true;
    }
    path.starts_with(root)
}

/// A path component that exists as a real directory. Symlinks, files and
/// missing entries are all refused here; creation paths handle absence.
#[allow(dead_code)]
fn ensure_real_dir(path: &Path) -> Result<(), Error> {
    match std::fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_symlink() => Err(Error::InvalidSetting(UNSAFE_PATH)),
        Ok(meta) if meta.is_dir() => Ok(()),
        Ok(_) => Err(Error::InvalidSetting(NOT_A_DIRECTORY)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            Err(Error::InvalidSetting(NOT_A_DIRECTORY))
        }
        Err(_) => Err(Error::InvalidSetting(NOT_A_DIRECTORY)),
    }
}
