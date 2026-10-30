// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! The disks Files shares: `/` on Unix, and every drive on Windows.
//!
//! Sharing is all or nothing. The set is empty until the owner enables
//! filesystem access, and then holds the whole disk; each peer operation
//! still needs Files shared with that peer (the `Transfer` grant). A Windows drive's id is its letter's
//! (A: is 0, C: is 2), so a remembered folder still names the same drive
//! after another one comes or goes; Unix shares `/` as id 0.
//!
//! Each root is recorded with its directory object's identity (device and
//! inode on Unix, volume serial, file index and creation time on Windows).
//! On Windows a letter can name another volume once a drive is taken out
//! and another plugged in: an operation on the root then fails instead of
//! reaching the other volume, and a transfer leg pinned to the drive it
//! began on ([`PinnedRoot`]) fails instead of continuing on another.
//!
//! A path is resolved with its directory links followed, as a file manager
//! follows them, and the resolved folders are then opened one at a time
//! relative to their parent's handle without following a link
//! (`vnilink_rename::anchored`: `openat` with `O_NOFOLLOW` on Unix,
//! `NtCreateFile` with `FILE_OPEN_REPARSE_POINT` on Windows). An entry
//! swapped for a link between the resolution and the open refuses the
//! operation instead of redirecting it.

use std::path::{Path, PathBuf};

use crate::error::Error;

use super::path::{check_rel, UNSAFE_PATH};

/// Largest number of roots: Windows has 26 drive letters.
pub const MAX_ROOTS: usize = 32;

pub(crate) const NO_ROOTS: &str = "filesystem access is not enabled";
pub(crate) const UNKNOWN_ROOT: &str = "the drive is unknown";
pub(crate) const NOT_A_DIRECTORY: &str = "the path is not a directory";
pub(crate) const DRIVE_CHANGED: &str = "the drive changed during the transfer";
const UNUSABLE_ROOT: &str = "a drive is not usable";

#[derive(Debug, Clone, PartialEq, Eq)]
struct Root {
    id: u64,
    path: PathBuf,
    dev: u64,
    ino: u64,
    /// Creation time, where the platform reports one: a recycled id at the
    /// same path is another object.
    born: Option<u64>,
}

/// One job leg's pinned root: the drive's identity plus the epoch that
/// admitted it. The leg serves only this object; a drive set read again
/// with another volume at the letter fails the leg instead of switching
/// sources or destinations mid-job. The pin is kept with the job, so it
/// holds across a restart too.
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
}

impl Default for RootSet {
    fn default() -> Self {
        // Restart-unique base: epoch equality short-circuits pin
        // verification ("the drives were not read again since the pin"),
        // which a zero base cannot promise across a restart — a persisted
        // pin's epoch could collide with the fresh counter and skip the
        // identity check that would fail a leg on a replaced drive.
        Self {
            roots: Vec::new(),
            epoch: rand::random(),
        }
    }
}

impl RootSet {
    pub(crate) fn is_empty(&self) -> bool {
        self.roots.is_empty()
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
    /// A matching epoch short-circuits (the drives were not read again
    /// since the pin); a changed epoch re-verifies the full identity, so a
    /// drive still in place or a restart with the same drives continues
    /// while a replaced or removed one fails the leg. Callers must serve
    /// nothing on error.
    pub(crate) fn verify_pin(&self, pin: &PinnedRoot) -> Result<(), Error> {
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
            _ => Err(Error::InvalidSetting(DRIVE_CHANGED)),
        }
    }

    /// Shares the whole disk: `/` on Unix, every drive that opens as a
    /// directory on Windows. It never grants a peer Files permission or
    /// elevates the process's OS access.
    pub(crate) fn set_filesystem(&mut self) -> Result<(), Error> {
        let roots = admit(&disk_roots()?)?;
        self.replace(roots);
        Ok(())
    }

    /// Reads the drives again while the whole disk is shared, so one
    /// plugged in since appears, one taken out goes, and another volume
    /// at a letter is recorded as that volume. Ids follow drive letters,
    /// so every other root keeps its id; a transfer pinned to a drive that
    /// went or was replaced fails on its next step rather than reaching
    /// another. Unix shares `/`, whose drives are folders under it.
    pub(crate) fn refresh_drives(&mut self) -> Result<(), Error> {
        #[cfg(windows)]
        if !self.roots.is_empty() {
            let roots = admit(&disk_roots()?)?;
            if roots != self.roots {
                self.replace(roots);
            }
        }
        Ok(())
    }

    fn replace(&mut self, roots: Vec<Root>) {
        self.roots = roots;
        self.epoch = self.epoch.wrapping_add(1);
    }

    /// Shares `paths` as if they were drives, for tests that replace a
    /// drive under a transfer.
    #[cfg(any(all(test, unix), feature = "test-probes"))]
    pub(crate) fn share_as_drives(&mut self, paths: &[PathBuf]) -> Result<(), Error> {
        let roots = admit(paths)?;
        self.replace(roots);
        Ok(())
    }

    /// The directory `rel` names under root `id`, its directory links
    /// followed. The resolved path is opened again through handles, so a
    /// link swapped in after this resolution refuses the operation.
    #[cfg(any(unix, windows))]
    fn directory_path(&self, id: u64, rel: &str) -> Result<PathBuf, Error> {
        check_rel(rel)?;
        // One component at a time: a Windows root is a verbatim path, where
        // `/` is not a separator.
        let mut path = self.root(id)?.path.clone();
        path.extend(rel.split('/').filter(|component| !component.is_empty()));
        std::fs::canonicalize(path).map_err(|_| Error::InvalidSetting(NOT_A_DIRECTORY))
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

    /// Opens the root itself without following a final link and checks it
    /// is the recorded object, creation time included: another volume at
    /// the drive's letter fails here.
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
    /// component of its resolved path relative to its parent without
    /// following it. A link, file or missing component met there refuses.
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
    /// folder cannot be moved while it is written.
    #[cfg(any(unix, windows))]
    pub(crate) fn open_dir_held(&self, id: u64, rel: &str) -> Result<HeldDir, Error> {
        self.open_dir_with(id, rel, true)
    }

    #[cfg(any(unix, windows))]
    fn open_dir_with(&self, id: u64, rel: &str, held: bool) -> Result<HeldDir, Error> {
        check_rel(rel)?;
        let path = self.directory_path(id, rel)?;
        self.walk(id, &path, held)
    }

    /// Opens `path`, already resolved under root `id`, through handles: the
    /// root, then each folder relative to its parent without following it.
    /// A folder on the path swapped for a link since it was resolved, a
    /// file or a missing folder refuses.
    #[cfg(any(unix, windows))]
    fn walk(&self, id: u64, path: &Path, held: bool) -> Result<HeldDir, Error> {
        use vnilink_rename::anchored::AsDir;
        let mut current = self.open_root_with(id, held)?;
        let mut chain = Vec::new();
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
    /// mutate must use the handles above so the check and the use share
    /// one anchored object instead of a pathname race.
    pub(crate) fn resolve_dir(&self, id: u64, rel: &str) -> Result<PathBuf, Error> {
        check_rel(rel)?;
        #[cfg(any(unix, windows))]
        {
            let _verified = self.open_dir_fd(id, rel)?;
            self.directory_path(id, rel)
        }
        #[cfg(not(any(unix, windows)))]
        {
            let _ = id;
            Err(Error::Unsupported("native filesystem operations"))
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
        let parent = self.resolve_dir(id, rel)?;
        let dest = parent.join(name);
        Ok((parent, dest))
    }
}

/// The whole disk's roots: `/` on Unix, every drive that opens as a
/// directory on Windows.
fn disk_roots() -> Result<Vec<PathBuf>, Error> {
    #[cfg(unix)]
    {
        Ok(vec![PathBuf::from("/")])
    }
    #[cfg(windows)]
    {
        let drives: Vec<_> = vnilink_rename::windows::drive_roots()
            .map_err(|_| Error::Unsupported("Windows mounted drives"))?
            .into_iter()
            .filter(|path| vnilink_rename::anchored::open_root(path).is_ok())
            .collect();
        if drives.is_empty() {
            return Err(Error::Unsupported("no accessible drives"));
        }
        Ok(drives)
    }
    #[cfg(not(any(unix, windows)))]
    {
        Err(Error::Unsupported("whole-filesystem native operations"))
    }
}

/// Records each of `paths` as a root with its directory object's identity.
/// A drive's id is its letter's; any other root's is its position.
fn admit(paths: &[PathBuf]) -> Result<Vec<Root>, Error> {
    if paths.len() > MAX_ROOTS {
        return Err(Error::InvalidSetting("too many drives"));
    }
    let mut roots = Vec::with_capacity(paths.len());
    for (index, path) in paths.iter().enumerate() {
        let id = drive_id(path).unwrap_or(index as u64);
        let canonical =
            std::fs::canonicalize(path).map_err(|_| Error::InvalidSetting(UNUSABLE_ROOT))?;
        #[cfg(any(unix, windows))]
        let identity = {
            use vnilink_rename::anchored::AsDir;
            let dir = vnilink_rename::anchored::open_root(&canonical)
                .map_err(|_| Error::InvalidSetting(UNUSABLE_ROOT))?;
            vnilink_rename::anchored::fstat_identity(dir.as_dir())
                .map_err(|_| Error::InvalidSetting(UNUSABLE_ROOT))?
        };
        #[cfg(any(unix, windows))]
        roots.push(Root {
            id,
            path: canonical,
            dev: identity.dev,
            ino: identity.ino,
            born: identity.born,
        });
        #[cfg(not(any(unix, windows)))]
        roots.push(Root {
            id,
            path: canonical,
            dev: 0,
            ino: 0,
            born: None,
        });
    }
    Ok(roots)
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

#[cfg(all(test, windows))]
mod windows_tests {
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

    /// Every drive that opens is shared, whatever its file system: a FAT
    /// or exFAT stick is listed beside NTFS drives.
    #[test]
    fn every_drive_that_opens_is_shared() {
        let mut roots = RootSet::default();
        roots.set_filesystem().unwrap();
        let shared: Vec<_> = roots.snapshot().into_iter().map(|(_, path)| path).collect();
        for drive in vnilink_rename::windows::drive_roots().unwrap() {
            if vnilink_rename::anchored::open_root(&drive).is_ok() {
                let drive = std::fs::canonicalize(&drive).unwrap();
                assert!(shared.contains(&drive), "{drive:?} is shared");
            }
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

    /// A junction on a path is followed to its folder, as Explorer
    /// follows it, and the folder is then walked through handles.
    #[test]
    fn a_junction_on_the_path_is_followed_to_its_folder() {
        let fixture = std::env::temp_dir().join(format!(
            "vnilink-fs-mode-{}-{}",
            std::process::id(),
            rand::random::<u64>()
        ));
        let shared = fixture.join("shared");
        let other = fixture.join("other");
        std::fs::create_dir_all(&shared).unwrap();
        std::fs::create_dir_all(&other).unwrap();
        junction(&shared.join("link"), &other);
        let mut roots = RootSet::default();
        roots.set_filesystem().unwrap();
        let canonical = std::fs::canonicalize(shared.join("link")).unwrap();
        assert_eq!(canonical, std::fs::canonicalize(&other).unwrap());
        let (id, rel) = roots.locate(&shared).unwrap();
        let rel = format!("{rel}/link");
        assert!(roots.open_dir_fd(id, &rel).is_ok());
        assert_eq!(roots.resolve_dir(id, &rel).unwrap(), canonical);
        std::fs::remove_dir_all(fixture).unwrap();
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::test_support::Scratch;

    /// A directory link on the path is followed to its folder, as a file
    /// manager follows it, and paths that leave the path rules are refused.
    #[test]
    fn a_directory_link_on_the_path_is_followed_to_its_folder() {
        let scratch = Scratch::tagged("fs");
        let shared = scratch.join("shared");
        let other = scratch.join("other");
        std::fs::create_dir_all(&shared).unwrap();
        std::fs::create_dir_all(&other).unwrap();
        std::os::unix::fs::symlink(&other, shared.join("link")).unwrap();
        let mut roots = RootSet::default();
        roots.set_filesystem().unwrap();
        let (id, rel) = roots.locate(&shared).unwrap();
        let rel = format!("{rel}/link");
        assert!(roots.open_dir_fd(id, &rel).is_ok());
        assert_eq!(
            roots.resolve_dir(id, &rel).unwrap(),
            std::fs::canonicalize(&other).unwrap()
        );
        assert!(roots.open_dir_fd(id, "../etc").is_err());
    }

    /// A folder swapped for a link after its path was resolved is refused
    /// by the walk, held or not, instead of being followed to the link's
    /// target.
    #[test]
    fn a_link_planted_after_resolution_is_refused_by_the_walk() {
        let scratch = Scratch::tagged("fs");
        let shared = scratch.join("shared");
        let other = scratch.join("other");
        std::fs::create_dir_all(shared.join("inner")).unwrap();
        std::fs::create_dir_all(other.join("inner")).unwrap();
        let mut roots = RootSet::default();
        roots.set_filesystem().unwrap();
        let (id, rel) = roots.locate(&shared).unwrap();
        let resolved = roots.directory_path(id, &format!("{rel}/inner")).unwrap();
        assert!(roots.walk(id, &resolved, false).is_ok());
        std::fs::rename(&shared, scratch.join("moved")).unwrap();
        std::os::unix::fs::symlink(&other, &shared).unwrap();
        for held in [false, true] {
            assert!(matches!(
                roots.walk(id, &resolved, held),
                Err(Error::InvalidSetting(message)) if message == UNSAFE_PATH
            ));
        }
    }

    /// Nothing is shared until filesystem access is enabled.
    #[test]
    fn nothing_resolves_before_the_disk_is_shared() {
        let roots = RootSet::default();
        assert!(matches!(
            roots.resolve_dir(0, ""),
            Err(Error::Unsupported(message)) if message == NO_ROOTS
        ));
    }

    /// A pin kept with a job cannot adopt another drive at the same
    /// letter: a restart or a reading of the drives that finds another
    /// volume there fails the leg.
    #[test]
    fn a_kept_pin_refuses_another_drive_at_the_same_path() {
        let scratch = Scratch::tagged("fs");
        let drive = scratch.join("drive");
        std::fs::create_dir(&drive).unwrap();
        let mut roots = RootSet::default();
        roots.share_as_drives(std::slice::from_ref(&drive)).unwrap();
        let pin = roots.pin(0).unwrap();
        let persisted = serde_json::to_vec(&pin).unwrap();
        std::fs::rename(&drive, scratch.join("original")).unwrap();
        std::fs::create_dir(&drive).unwrap();
        std::fs::write(drive.join("valuable"), b"keep").unwrap();
        let pin: PinnedRoot = serde_json::from_slice(&persisted).unwrap();
        let mut restarted = RootSet::default();
        restarted
            .share_as_drives(std::slice::from_ref(&drive))
            .unwrap();
        assert!(restarted.verify_pin(&pin).is_err());
        roots.share_as_drives(std::slice::from_ref(&drive)).unwrap();
        assert!(roots.verify_pin(&pin).is_err());
        assert_eq!(std::fs::read(drive.join("valuable")).unwrap(), b"keep");
    }

    /// A drive read again unchanged keeps its pins.
    #[test]
    fn a_pin_holds_while_its_drive_stays() {
        let scratch = Scratch::tagged("fs");
        let drive = scratch.join("drive");
        std::fs::create_dir(&drive).unwrap();
        let mut roots = RootSet::default();
        roots.share_as_drives(std::slice::from_ref(&drive)).unwrap();
        let pin = roots.pin(0).unwrap();
        roots.share_as_drives(std::slice::from_ref(&drive)).unwrap();
        assert!(roots.verify_pin(&pin).is_ok());
    }
}
