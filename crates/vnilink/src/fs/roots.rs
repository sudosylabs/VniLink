//! Owner-approved filesystem roots.
//!
//! The set starts empty and is replaced wholesale by the owner. A root id
//! is its position at set time, except that a shared Windows drive keeps its
//! letter's id (A: is 0, C: is 2), so a remembered folder still names the
//! same drive after another one comes or goes. Access is anchored to
//! directory handles (`vnilink_rename::anchored`): the root is opened
//! without following a final link and pinned to its object (device and
//! inode on Unix, volume serial and file index on Windows), then every
//! component is opened relative to its parent's handle without following
//! it (`openat` with `O_NOFOLLOW` on Unix, `NtCreateFile` with
//! `FILE_OPEN_REPARSE_POINT` on Windows). A link anywhere on the resolved
//! path (a symlink, or on Windows a junction or mount point), including a
//! parent swapped after listing or the root itself replaced by one, refuses
//! the operation instead of escaping the approved folder.

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
    /// Creation time, where the platform reports one: a recycled id at the
    /// same path is another object.
    #[allow(dead_code)]
    born: Option<u64>,
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
    pub(crate) born: Option<u64>,
    pub(crate) path: PathBuf,
}

/// A directory opened for a mutation, with the folders above it up to the
/// root kept open so none of them can move while it is in use.
#[cfg(any(unix, windows))]
pub(crate) struct HeldDir {
    dir: vnilink_rename::anchored::Dir,
    _chain: Vec<vnilink_rename::anchored::Dir>,
}

#[cfg(any(unix, windows))]
impl std::ops::Deref for HeldDir {
    type Target = vnilink_rename::anchored::Dir;

    fn deref(&self) -> &Self::Target {
        &self.dir
    }
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

    /// Whether the whole file system is shared rather than chosen folders.
    #[cfg_attr(not(windows), allow(dead_code))]
    pub(crate) fn whole_filesystem(&self) -> bool {
        self.whole_filesystem
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
            born: root.born,
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
            Some(root)
                if root.dev == pin.dev
                    && root.ino == pin.ino
                    && root.born == pin.born
                    && root.path == pin.path =>
            {
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
            #[cfg(any(unix, windows))]
            {
                // Pin the directory object itself: open it without following
                // a final link and record its identity, so a later
                // replacement (a link or a different directory at the same
                // path) is refused on every use.
                // A volume whose ids are entry positions (FAT, exFAT) or
                // that reports none cannot prove an object is the one
                // listed, so it is no root.
                use vnilink_rename::anchored::AsDir;
                let dir = vnilink_rename::anchored::open_root(&canonical)
                    .map_err(|_| Error::InvalidSetting("a file root is not usable"))?;
                vnilink_rename::anchored::require_stable_ids(dir.as_dir())
                    .map_err(|_| Error::InvalidSetting("a file root is not usable"))?;
                let identity = vnilink_rename::anchored::fstat_identity(dir.as_dir())
                    .map_err(|_| Error::InvalidSetting("a file root is not usable"))?;
                roots.push(Root {
                    id,
                    path: canonical,
                    dev: identity.dev,
                    ino: identity.ino,
                    born: identity.born,
                });
            }
            #[cfg(not(any(unix, windows)))]
            {
                roots.push(Root {
                    id,
                    path: canonical,
                    dev: 0,
                    ino: 0,
                    born: None,
                });
            }
        }
        self.roots = roots;
        self.whole_filesystem = whole_filesystem;
        self.epoch += 1;
        Ok(())
    }

    /// Reads the drives again while the whole disk is shared, so one
    /// plugged in since appears and one taken out goes. Ids follow drive
    /// letters, so every other root keeps its id; a transfer pinned to a
    /// drive that went fails on its next step rather than reaching another.
    /// Unix shares `/`, whose drives are folders under it.
    pub(crate) fn refresh_drives(&mut self) -> Result<(), Error> {
        #[cfg(windows)]
        if self.whole_filesystem {
            let available: Vec<_> = vnilink_rename::windows::drive_roots()
                .map_err(|_| Error::Unsupported("Windows mounted drives"))?
                .into_iter()
                .filter(|p| usable_drive(p))
                .collect();
            let current: Vec<_> = self.roots.iter().map(|root| root.path.clone()).collect();
            let same = available.len() == current.len()
                && available.iter().all(|path| {
                    std::fs::canonicalize(path).is_ok_and(|path| current.contains(&path))
                });
            if !same && !available.is_empty() {
                self.set_mode(&available, true)?;
            }
        }
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
            let available: Vec<_> = paths.into_iter().filter(|p| usable_drive(p)).collect();
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

    #[cfg(any(unix, windows))]
    fn directory_path(&self, id: u64, rel: &str) -> Result<PathBuf, Error> {
        check_rel(rel)?;
        // One component at a time: a Windows root is a verbatim path, where
        // `/` is not a separator.
        let mut path = self.root(id)?.path.clone();
        path.extend(rel.split('/').filter(|component| !component.is_empty()));
        if self.whole_filesystem {
            // Following a user-visible directory link is allowed in this mode.
            // The resulting components are still opened without following;
            // a concurrent replacement fails instead of redirecting the
            // actual operation.
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

    /// Opens the root itself anchored and pinned: not following a final link,
    /// plus identity equality against the stored object, creation time
    /// included. A root replaced by a link, or by a different directory at
    /// the same path (including via a swapped parent), fails here.
    #[cfg(any(unix, windows))]
    fn open_root_with(&self, id: u64, held: bool) -> Result<vnilink_rename::anchored::Dir, Error> {
        use vnilink_rename::anchored::AsDir;
        let root = self.root(id)?;
        let dir = if held {
            vnilink_rename::anchored::open_root_held(&root.path)
        } else {
            vnilink_rename::anchored::open_root(&root.path)
        }
        .map_err(|_| Error::InvalidSetting(UNSAFE_PATH))?;
        let identity = vnilink_rename::anchored::fstat_identity(dir.as_dir())
            .map_err(|_| Error::InvalidSetting(UNSAFE_PATH))?;
        if (identity.dev, identity.ino, identity.born) != (root.dev, root.ino, root.born) {
            return Err(Error::InvalidSetting(UNSAFE_PATH));
        }
        Ok(dir)
    }

    /// Opens `rel` under root `id` as a directory handle, opening every
    /// component relative to its parent without following it. Any link,
    /// file or missing component refuses.
    #[cfg(any(unix, windows))]
    pub(crate) fn open_dir_fd(
        &self,
        id: u64,
        rel: &str,
    ) -> Result<vnilink_rename::anchored::Dir, Error> {
        Ok(self.open_dir_with(id, rel, false)?.dir)
    }

    /// [`open_dir_fd`](Self::open_dir_fd) for a mutation: every folder from
    /// the root down stays open for as long as the result lives, and on
    /// Windows none of them can be renamed or deleted meanwhile, so the
    /// folder cannot be moved out of the root while it is written.
    #[cfg(any(unix, windows))]
    pub(crate) fn open_dir_held(&self, id: u64, rel: &str) -> Result<HeldDir, Error> {
        self.open_dir_with(id, rel, true)
    }

    #[cfg(any(unix, windows))]
    fn open_dir_with(&self, id: u64, rel: &str, held: bool) -> Result<HeldDir, Error> {
        use vnilink_rename::anchored::AsDir;
        check_rel(rel)?;
        let mut current = self.open_root_with(id, held)?;
        let mut chain = Vec::new();
        let path = self.directory_path(id, rel)?;
        let relative = path
            .strip_prefix(&self.root(id)?.path)
            .map_err(|_| Error::InvalidSetting(UNSAFE_PATH))?;
        for component in relative.components() {
            let component = component
                .as_os_str()
                .to_str()
                .ok_or(Error::InvalidSetting(UNSAFE_PATH))?;
            let child = if held {
                vnilink_rename::anchored::open_child_held(current.as_dir(), component)
            } else {
                vnilink_rename::anchored::open_child(current.as_dir(), component)
            }
            .map_err(|_| Error::InvalidSetting(UNSAFE_PATH))?;
            let parent = std::mem::replace(&mut current, child);
            if held {
                chain.push(parent);
            }
        }
        Ok(HeldDir {
            dir: current,
            _chain: chain,
        })
    }

    /// Resolves `rel` under root `id` to an absolute path that is still a
    /// real directory chain. Every component, including the root itself, is
    /// verified through directory handles at resolve time; callers that
    /// mutate must use the `*_fd` handles above so the check and the use
    /// share one anchored object instead of a pathname race.
    pub(crate) fn resolve_dir(&self, id: u64, rel: &str) -> Result<PathBuf, Error> {
        check_rel(rel)?;
        #[cfg(any(unix, windows))]
        {
            // Verify through handles first: root pinning plus per-component
            // opens that never follow a link. The returned path is for
            // compatible readers; mutating callers use
            // `open_dir_held` directly.
            let _verified = self.open_dir_fd(id, rel)?;
            self.directory_path(id, rel)
        }
        #[cfg(not(any(unix, windows)))]
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
        if !self.whole_filesystem && !under_root(&root.path, &dest) {
            return Err(Error::InvalidSetting(UNSAFE_PATH));
        }
        Ok((parent, dest))
    }
}

/// A drive that opens as a directory on a volume with stable file ids. A
/// FAT or exFAT drive is left out: its ids are entry positions, so no
/// operation on it could prove it still names the listed object.
#[cfg(windows)]
fn usable_drive(path: &Path) -> bool {
    use vnilink_rename::anchored::AsDir;
    vnilink_rename::anchored::open_root(path)
        .is_ok_and(|dir| vnilink_rename::anchored::require_stable_ids(dir.as_dir()).is_ok())
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

    /// A junction needs no privilege, unlike a symbolic link.
    fn junction(link: &Path, target: &Path) {
        let status = std::process::Command::new("cmd")
            .arg("/C")
            .arg("mklink")
            .arg("/J")
            .arg(link)
            .arg(target)
            .stdout(std::process::Stdio::null())
            .status()
            .unwrap();
        assert!(status.success(), "mklink /J failed");
    }

    /// A folder root is admitted and walked through handles; a junction is
    /// refused as a root and as any component under one, and the
    /// whole-disk mode follows it as Unix follows a directory symlink.
    #[test]
    fn folder_roots_refuse_junctions_and_whole_disks_follow_them() {
        let fixture = std::env::temp_dir().join(format!(
            "vnilink-fs-mode-{}-{}",
            std::process::id(),
            rand::random::<u64>()
        ));
        let approved = fixture.join("approved");
        let other = fixture.join("other");
        std::fs::create_dir_all(approved.join("real/deeper")).unwrap();
        std::fs::create_dir_all(&other).unwrap();
        junction(&approved.join("link"), &other);
        let mut roots = RootSet::default();
        roots.set(std::slice::from_ref(&approved)).unwrap();
        assert!(roots.open_dir_fd(0, "real/deeper").is_ok());
        assert_eq!(
            roots.resolve_dir(0, "real/deeper").unwrap(),
            std::fs::canonicalize(approved.join("real\\deeper")).unwrap()
        );
        assert!(roots.open_dir_fd(0, "link").is_err());
        assert!(roots.resolve_dir(0, "link").is_err());
        assert!(matches!(
            roots.set(&[approved.join("link")]),
            Err(Error::InvalidSetting(_))
        ));
        roots.set_filesystem().unwrap();
        let canonical = std::fs::canonicalize(approved.join("link")).unwrap();
        assert_eq!(canonical, std::fs::canonicalize(&other).unwrap());
        let (id, rel) = roots
            .snapshot()
            .into_iter()
            .find_map(|(id, root)| {
                let rest = std::fs::canonicalize(&approved).ok()?;
                let rest = rest.strip_prefix(&root).ok()?;
                let parts: Vec<_> = rest.iter().map(|part| part.to_str().unwrap()).collect();
                Some((id, format!("{}/link", parts.join("/"))))
            })
            .unwrap();
        assert!(roots.open_dir_fd(id, &rel).is_ok());
        assert_eq!(roots.resolve_dir(id, &rel).unwrap(), canonical);
        roots.set(std::slice::from_ref(&approved)).unwrap();
        assert!(roots.open_dir_fd(0, "link").is_err());
        std::fs::remove_dir_all(fixture).unwrap();
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

#[cfg(all(test, unix))]
mod tests {
    use crate::test_support::Scratch;

    /// Persisted pins cannot adopt a different directory at the same path.
    #[test]
    #[cfg(unix)]
    fn persisted_root_pin_refuses_same_path_replacement() {
        let scratch = Scratch::tagged("fs");
        let root = scratch.join("root");
        std::fs::create_dir(&root).unwrap();
        let mut roots = crate::fs::RootSet::default();
        roots.set(std::slice::from_ref(&root)).unwrap();
        let pin = roots.pin(0).unwrap();
        let persisted = serde_json::to_vec(&pin).unwrap();
        std::fs::rename(&root, scratch.join("original")).unwrap();
        std::fs::create_dir(&root).unwrap();
        std::fs::write(root.join("valuable"), b"keep").unwrap();
        let pin: crate::fs::PinnedRoot = serde_json::from_slice(&persisted).unwrap();
        let mut restarted = crate::fs::RootSet::default();
        restarted.set(std::slice::from_ref(&root)).unwrap();
        assert!(restarted.verify_pin(&pin).is_err());
        roots.set(std::slice::from_ref(&root)).unwrap();
        assert!(roots.verify_pin(&pin).is_err());
        assert_eq!(std::fs::read(root.join("valuable")).unwrap(), b"keep");
    }
}
