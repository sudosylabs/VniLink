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
