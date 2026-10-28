use std::path::{Path, PathBuf};

/// A folder of a test's own under the system's temporary folder, removed
/// when it is dropped. It is made anew, private to this user, under a name
/// no other process chose: an existing folder or a link planted at that
/// name is never used.
pub struct Scratch {
    path: PathBuf,
    /// Owns the folder: dropping it removes it.
    folder: tempfile::TempDir,
}

impl Scratch {
    pub fn new(tag: &str) -> Self {
        // Engines compare canonical paths: on macOS the temporary folder is
        // reached through a link, which is resolved before the folder is
        // made so the folder itself is never reached through one. Windows
        // would answer with a verbatim `\\?\` path, which cmd refuses as
        // its directory.
        let parent = std::env::temp_dir();
        #[cfg(unix)]
        let parent = std::fs::canonicalize(&parent).expect("the temporary folder");
        let prefix = format!("vnilink-testkit-{tag}-{}-", std::process::id());
        let mut builder = tempfile::Builder::new();
        builder.prefix(&prefix);
        #[cfg(unix)]
        builder.permissions(std::os::unix::fs::PermissionsExt::from_mode(0o700));
        let folder = builder.tempdir_in(parent).expect("a scratch folder");
        Self {
            path: folder.path().to_path_buf(),
            folder,
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn join(&self, name: impl AsRef<Path>) -> PathBuf {
        self.path.join(name)
    }

    /// Removes the folder, saying why when it could not be: dropping it
    /// removes it too, silently.
    pub fn close(self) -> std::io::Result<()> {
        self.folder.close()
    }

    /// Removes the folder like [`Self::close`], trying again until
    /// `deadline` while something still holds it: Windows releases an ended
    /// process's working folder some time after the process has ended.
    pub fn close_by(self, deadline: std::time::Instant) -> std::io::Result<()> {
        let path = self.folder.keep();
        loop {
            match std::fs::remove_dir_all(&path) {
                Err(error)
                    if error.kind() != std::io::ErrorKind::NotFound
                        && std::time::Instant::now() < deadline =>
                {
                    std::thread::sleep(std::time::Duration::from_millis(20));
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
                result => return result,
            }
        }
    }
}

/// The file systems [`ScratchVolume`] makes: those whose ids are entry
/// positions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PositionalFs {
    Fat32,
    ExFat,
}

/// A FAT or exFAT volume mounted inside a test's scratch folder, where
/// this computer can make one without administrator rights: on macOS a
/// disk image made and attached with `hdiutil`. Elsewhere mounting needs
/// an administrator (Linux without a user mount for it, Windows), so
/// [`Self::new`] says why there is none. Detached and removed on drop.
pub struct ScratchVolume {
    mount: PathBuf,
    /// Holds the image and the mount point; dropped after the detach.
    _scratch: Scratch,
}

impl ScratchVolume {
    /// A new, empty volume of `fs`, or why this computer cannot make one.
    pub fn new(fs: PositionalFs) -> Result<Self, String> {
        #[cfg(target_os = "macos")]
        {
            let scratch = Scratch::new("volume");
            let image = scratch.join("volume.dmg");
            let mount = scratch.join("mounted");
            std::fs::create_dir(&mount).map_err(|error| error.to_string())?;
            let format = match fs {
                PositionalFs::Fat32 => "MS-DOS FAT32",
                PositionalFs::ExFat => "ExFAT",
            };
            let run = |args: &[&std::ffi::OsStr]| {
                std::process::Command::new("hdiutil")
                    .args(args)
                    .status()
                    .map_err(|error| format!("hdiutil could not run: {error}"))
                    .and_then(|status| {
                        status
                            .success()
                            .then_some(())
                            .ok_or_else(|| format!("hdiutil {:?} failed: {status}", args[0]))
                    })
            };
            run(&[
                "create".as_ref(),
                "-quiet".as_ref(),
                "-size".as_ref(),
                "64m".as_ref(),
                "-fs".as_ref(),
                format.as_ref(),
                "-volname".as_ref(),
                "VNLTEST".as_ref(),
                image.as_os_str(),
            ])?;
            run(&[
                "attach".as_ref(),
                "-quiet".as_ref(),
                "-nobrowse".as_ref(),
                "-mountpoint".as_ref(),
                mount.as_os_str(),
                image.as_os_str(),
            ])?;
            Ok(Self {
                mount,
                _scratch: scratch,
            })
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = fs;
            Err("mounting a FAT or exFAT volume here needs an administrator".to_owned())
        }
    }

    /// Where the volume is mounted.
    pub fn path(&self) -> &Path {
        &self.mount
    }
}

impl Drop for ScratchVolume {
    fn drop(&mut self) {
        #[cfg(target_os = "macos")]
        {
            let detached = std::process::Command::new("hdiutil")
                .args(["detach", "-quiet", "-force"])
                .arg(&self.mount)
                .status();
            if !detached.is_ok_and(|status| status.success()) {
                eprintln!(
                    "the scratch volume at {} was not detached",
                    self.mount.display()
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Each scratch folder is new, private and gone once dropped.
    #[test]
    fn a_scratch_folder_is_new_private_and_removed() {
        let first = Scratch::new("scratch");
        let second = Scratch::new("scratch");
        assert_ne!(first.path(), second.path(), "never a folder another made");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(first.path())
                .expect("the folder")
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o700, "readable by this user only");
            assert!(
                !std::fs::symlink_metadata(first.path())
                    .expect("the folder")
                    .file_type()
                    .is_symlink(),
                "a folder, not a link"
            );
        }
        let path = first.path().to_path_buf();
        drop(first);
        assert!(!path.exists(), "removed when dropped");
    }
}
