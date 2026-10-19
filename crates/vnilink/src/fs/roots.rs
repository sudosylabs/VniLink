//! Owner-approved filesystem roots.
//!
//! The set starts empty and is replaced wholesale by the owner. A root id
//! is its position at set time, except that a shared Windows drive keeps its
//! letter's id (A: is 0, C: is 2), so a remembered folder still names the
//! same drive after another one comes or goes. Unix access is anchored to directory handles:
//! the root is opened `O_NOFOLLOW|O_DIRECTORY` and pinned to its
//! device+inode, then every component is opened via `openat` with
//! `O_NOFOLLOW`. A symlink anywhere on the resolved path, including a
//! parent swapped after listing or the root itself replaced by a symlink,
//! refuses the operation instead of escaping the approved folder. Nonempty
//! Windows root admission is disabled until handle-relative child operations
//! are available; native root identity alone cannot enforce confinement.

use std::path::{Path, PathBuf};

use crate::error::Error;

use super::path::{check_rel, UNSAFE_PATH};

/// Largest number of approved roots.
pub const MAX_ROOTS: usize = 32;

pub(crate) const NO_ROOTS: &str = "no file roots are set";
pub(crate) const UNKNOWN_ROOT: &str = "the file root is unknown";
pub(crate) const NOT_A_DIRECTORY: &str = "the path is not a directory";
pub(crate) const ROOTS_REBOUND: &str = "the file roots were rebound during the transfer";

#[derive(Debug, Clone)]
struct Root {
    id: u64,
    path: PathBuf,
    #[allow(dead_code)]
    dev: u64,
    #[allow(dead_code)]
    ino: u64,
}

/// One job leg's pinned root: the identity plus the epoch that admitted
/// it. The leg serves only this object; a rebind that moves the entry
/// fails the leg instead of switching sources or destinations mid-job.
/// Unix and Windows pins carry native object identity across rebind/restart.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct PinnedRoot {
    pub(crate) epoch: u64,
    pub(crate) id: u64,
    pub(crate) dev: u64,
    pub(crate) ino: u64,
    pub(crate) path: PathBuf,
}

#[derive(Debug, Clone)]
pub(crate) struct RootSet {
    roots: Vec<Root>,
    epoch: u64,
    whole_filesystem: bool,
}

impl Default for RootSet {
    fn default() -> Self {
        // Restart-unique base: epoch equality short-circuits pin
        // verification ("no re-set since the pin"), which a zero base
        // cannot promise across a restart — a persisted pin's epoch
        // could collide with the fresh counter and skip the identity
        // check that would fail a rebound leg.
        Self {
            roots: Vec::new(),
            epoch: rand::random(),
            whole_filesystem: false,
        }
    }
}

impl RootSet {
    pub(crate) fn is_empty(&self) -> bool {
        self.roots.is_empty()
    }
    pub(crate) fn epoch(&self) -> u64 {
        self.epoch
    }

    /// Pin root `id` for a job leg. Fails when the root is unknown, like
    /// any other resolution.
    pub(crate) fn pin(&self, id: u64) -> Result<PinnedRoot, Error> {
        let root = self.root(id)?;
        Ok(PinnedRoot {
            epoch: self.epoch,
            id,
            dev: root.dev,
            ino: root.ino,
            path: root.path.clone(),
        })
    }

    /// Verify a leg's pin: the current entry must be the pinned object.
    /// A matching epoch short-circuits (no rebind ran since the pin); a
    /// changed epoch re-verifies the full identity, so a same-object
    /// re-set or a restart with unchanged roots continues while any
    /// rebound entry fails the leg. Callers must serve nothing on error.
    pub(crate) fn verify_pin(&self, pin: &PinnedRoot) -> Result<(), Error> {
        // Epoch equality means no rebind ran since the pin: the entry is
        // unchanged. (Resolution still validates the id separately.)
        if self.epoch == pin.epoch {
            return Ok(());
        }
        match self.roots.iter().find(|root| root.id == pin.id) {
            Some(root) if root.dev == pin.dev && root.ino == pin.ino && root.path == pin.path => {
                Ok(())
            }
            _ => Err(Error::InvalidSetting(ROOTS_REBOUND)),
        }
    }

    pub(crate) fn set(&mut self, paths: &[PathBuf]) -> Result<(), Error> {
        self.set_mode(paths, false)
    }

    fn set_mode(&mut self, paths: &[PathBuf], whole_filesystem: bool) -> Result<(), Error> {
        if paths.len() > MAX_ROOTS {
            return Err(Error::InvalidSetting("too many file roots"));
        }
        // Path-based child operations cannot enforce confinement on Windows.
        // Refuse admission rather than expose roots with a check/use race.
        #[cfg(windows)]
        if !paths.is_empty() && !whole_filesystem {
            return Err(Error::Unsupported(
                "rooted Files operations require handle-relative Windows support",
            ));
        }
        let mut roots = Vec::with_capacity(paths.len());
        for (index, path) in paths.iter().enumerate() {
            let id = if whole_filesystem {
                drive_id(path).unwrap_or(index as u64)
            } else {
                index as u64
            };
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
                    id,
                    path: canonical,
                    dev,
                    ino,
                });
            }
            #[cfg(windows)]
            {
                let file = vnilink_rename::windows::open_directory(&canonical)
                    .map_err(|_| Error::InvalidSetting("a file root is not usable"))?;
                let (dev, ino) = vnilink_rename::windows::directory_identity(&file)
                    .map_err(|_| Error::InvalidSetting("a file root is not usable"))?;
                roots.push(Root {
                    id,
                    path: canonical,
                    dev,
                    ino,
                });
            }
            #[cfg(not(any(unix, windows)))]
            {
                roots.push(Root {
                    id,
                    path: canonical,
                    dev: 0,
                    ino: 0,
                });
            }
        }
        self.roots = roots;
        self.whole_filesystem = whole_filesystem;
        self.epoch += 1;
        Ok(())
    }

    /// Whole-filesystem browsing is a distinct owner choice. It never grants
    /// a peer Files permission or elevates the process's OS access.
    pub(crate) fn set_filesystem(&mut self) -> Result<(), Error> {
        #[cfg(unix)]
        {
            self.set(&[PathBuf::from("/")])?;
            self.whole_filesystem = true;
            Ok(())
        }
        #[cfg(windows)]
        {
            let paths = vnilink_rename::windows::drive_roots()
                .map_err(|_| Error::Unsupported("Windows mounted drives"))?;
            let available: Vec<_> = paths
                .into_iter()
                .filter(|p| vnilink_rename::windows::open_directory(p).is_ok())
                .collect();
            if available.is_empty() {
                return Err(Error::Unsupported("no accessible drives"));
            }
            self.set_mode(&available, true)
        }
        #[cfg(not(any(unix, windows)))]
        {
            Err(Error::Unsupported("whole-filesystem native operations"))
        }
    }

    #[cfg(unix)]
    fn directory_path(&self, id: u64, rel: &str) -> Result<PathBuf, Error> {
        check_rel(rel)?;
        let path = self.root(id)?.path.join(rel);
        if self.whole_filesystem {
            // Following a user-visible directory link is allowed in this mode.
            // The resulting components are still opened NOFOLLOW; a concurrent
            // replacement fails instead of redirecting the actual operation.
            std::fs::canonicalize(path).map_err(|_| Error::InvalidSetting(NOT_A_DIRECTORY))
        } else {
            Ok(path)
        }
    }

    pub(crate) fn snapshot(&self) -> Vec<(u64, PathBuf)> {
        self.roots
            .iter()
            .map(|root| (root.id, root.path.clone()))
            .collect()
    }

    /// Names an absolute directory as a root and a relative path, or
    /// `None` when it lies outside every root or the path rules.
    pub(crate) fn locate(&self, path: &Path) -> Option<(u64, String)> {
        let canonical = std::fs::canonicalize(path).ok()?;
        self.roots.iter().find_map(|root| {
            let relative = canonical.strip_prefix(&root.path).ok()?;
            let parts = relative
                .components()
                .map(|component| component.as_os_str().to_str())
                .collect::<Option<Vec<_>>>()?;
            let rel = parts.join("/");
            check_rel(&rel).ok()?;
            Some((root.id, rel))
        })
    }

    fn root(&self, id: u64) -> Result<&Root, Error> {
        if self.roots.is_empty() {
            return Err(Error::Unsupported(NO_ROOTS));
        }
        self.roots
            .iter()
            .find(|root| root.id == id)
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
        let path = self.directory_path(id, rel)?;
        let relative = path
            .strip_prefix(&self.root(id)?.path)
            .map_err(|_| Error::InvalidSetting(UNSAFE_PATH))?;
        for component in relative.components() {
            let component = component
                .as_os_str()
                .to_str()
                .ok_or(Error::InvalidSetting(UNSAFE_PATH))?;
            let child = vnilink_rename::anchored::open_child(current.as_fd(), component)
                .map_err(|_| Error::InvalidSetting(UNSAFE_PATH))?;
            current = child;
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
            self.directory_path(id, rel)
        }
        #[cfg(not(unix))]
        {
            let root = self.root(id)?;
            let mut current = root.path.clone();
            #[cfg(windows)]
            if self.whole_filesystem {
                let file = vnilink_rename::windows::open_directory(&root.path)
                    .map_err(|_| Error::InvalidSetting(UNSAFE_PATH))?;
                if vnilink_rename::windows::directory_identity(&file)
                    .map_err(|_| Error::InvalidSetting(UNSAFE_PATH))?
                    != (root.dev, root.ino)
                {
                    return Err(Error::InvalidSetting(UNSAFE_PATH));
                }
                for component in rel.split('/').filter(|p| !p.is_empty()) {
                    current.push(component);
                }
                let resolved = std::fs::canonicalize(current)
                    .map_err(|_| Error::InvalidSetting(NOT_A_DIRECTORY))?;
                if !resolved.is_dir() {
                    return Err(Error::InvalidSetting(NOT_A_DIRECTORY));
                }
                return Ok(resolved);
            }
            if !rel.is_empty() {
                for component in rel.split('/') {
                    current.push(component);
                    if !under_root(&root.path, &current) {
                        return Err(Error::InvalidSetting(UNSAFE_PATH));
                    }
                    ensure_real_dir(&current)?;
                }
            }
            ensure_real_dir(&root.path)?;
            #[cfg(windows)]
            {
                let file = vnilink_rename::windows::open_directory(&root.path)
                    .map_err(|_| Error::InvalidSetting(UNSAFE_PATH))?;
                let identity = vnilink_rename::windows::directory_identity(&file)
                    .map_err(|_| Error::InvalidSetting(UNSAFE_PATH))?;
                if identity != (root.dev, root.ino) {
                    return Err(Error::InvalidSetting(UNSAFE_PATH));
                }
            }
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
        if !self.whole_filesystem && !under_root(&root.path, &dest) {
            return Err(Error::InvalidSetting(UNSAFE_PATH));
        }
        Ok((parent, dest))
    }
}

/// A drive root's id from its letter: `A:\` is 0, `C:\` is 2.
fn drive_id(path: &Path) -> Option<u64> {
    drive_letter(path).map(|letter| u64::from(letter - b'A'))
}

/// The drive letter a Windows path starts with, plain (`C:\`) or verbatim
/// (`\\?\C:\`), in upper case. `None` elsewhere, including on Unix.
pub(crate) fn drive_letter(path: &Path) -> Option<u8> {
    match path.components().next()? {
        std::path::Component::Prefix(prefix) => match prefix.kind() {
            std::path::Prefix::Disk(letter) | std::path::Prefix::VerbatimDisk(letter) => {
                Some(letter.to_ascii_uppercase())
            }
            _ => None,
        },
        _ => None,
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

#[cfg(all(test, windows))]
mod windows_admission_tests {
    use super::*;
    #[test]
    fn drives_keep_their_letter_as_id() {
        assert_eq!(drive_id(Path::new("C:\\")), Some(2));
        assert_eq!(drive_id(Path::new("\\\\?\\D:\\")), Some(3));
        assert_eq!(drive_letter(Path::new("\\\\?\\e:\\")), Some(b'E'));
        assert_eq!(drive_id(Path::new("\\\\server\\share")), None);
        let mut roots = RootSet::default();
        roots.set_filesystem().unwrap();
        for (id, path) in roots.snapshot() {
            assert_eq!(Some(id), drive_id(&path), "{path:?}");
        }
    }

    #[test]
    fn nonempty_roots_fail_closed_without_mutating_admission() {
        let mut roots = RootSet::default();
        let epoch = roots.epoch();
        assert!(matches!(
            roots.set(&[PathBuf::from("C:\\")]),
            Err(Error::Unsupported(_))
        ));
        assert!(roots.snapshot().is_empty());
        assert_eq!(roots.epoch(), epoch);
        roots.set(&[]).unwrap();
    }
}

#[cfg(all(test, unix))]
mod filesystem_mode_tests {
    use super::*;
    #[test]
    fn whole_filesystem_follows_directory_links_but_folder_mode_stays_confined() {
        let fixture = std::env::temp_dir().join(format!(
            "vnilink-fs-mode-{}-{}",
            std::process::id(),
            rand::random::<u64>()
        ));
        let approved = fixture.join("approved");
        let other = fixture.join("other");
        std::fs::create_dir_all(&approved).unwrap();
        std::fs::create_dir_all(&other).unwrap();
        std::os::unix::fs::symlink(&other, approved.join("link")).unwrap();
        let mut roots = RootSet::default();
        roots.set(&[approved.clone()]).unwrap();
        assert!(roots.open_dir_fd(0, "link").is_err());
        roots.set_filesystem().unwrap();
        let relative = approved
            .join("link")
            .strip_prefix("/")
            .unwrap()
            .to_string_lossy()
            .into_owned();
        assert!(roots.open_dir_fd(0, &relative).is_ok());
        assert_eq!(
            roots.resolve_dir(0, &relative).unwrap(),
            std::fs::canonicalize(&other).unwrap()
        );
        assert!(roots.open_dir_fd(0, "../etc").is_err());
        roots.set(&[approved]).unwrap();
        assert!(roots.open_dir_fd(0, "link").is_err());
        std::fs::remove_dir_all(fixture).unwrap();
    }
}
