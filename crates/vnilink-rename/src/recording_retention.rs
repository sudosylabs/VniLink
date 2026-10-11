//! Retention and publication authority is an open file, never a reusable filename.
//! Move the candidate atomically into an exclusively created private namespace
//! before inspecting it. Public-name churn cannot race the eventual unlink or publish.
use std::{fs::File, io, path::Path, time::SystemTime};

pub struct RecordedFile {
    file: File,
    size: u64,
    modified: SystemTime,
}
impl RecordedFile {
    pub fn new(file: &File) -> io::Result<Self> {
        let meta = file.metadata()?;
        Ok(Self {
            file: file.try_clone()?,
            size: meta.len(),
            modified: meta.modified()?,
        })
    }
    /// Removes only the same unchanged object from the recorded pathname.
    /// A replacement is restored without overwriting a concurrently created
    /// entry. If restoration collides, both files survive and the candidate
    /// remains in the private recovery directory; return a storage error.
    pub fn remove(self, path: &Path) -> io::Result<bool> {
        #[cfg(unix)]
        {
            self.isolate_unix(path, None)
        }
        #[cfg(windows)]
        {
            self.remove_windows(path)
        }
        #[cfg(not(any(unix, windows)))]
        {
            let _ = path;
            Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "native recording retention unavailable",
            ))
        }
    }
    /// Publishes only this unchanged single-link file, after isolating the
    /// public source in an exclusively created private namespace. A mismatch
    /// or destination collision restores the candidate without replacement;
    /// restoration collisions leave it in the private recovery directory.
    pub fn publish(&self, source: &Path, destination: &Path) -> io::Result<()> {
        if source.parent() != destination.parent() || source == destination {
            return Err(io::ErrorKind::InvalidInput.into());
        }
        #[cfg(unix)]
        let published = self.isolate_unix(source, Some(destination))?;
        #[cfg(windows)]
        let published = self.publish_windows(source, destination)?;
        #[cfg(not(any(unix, windows)))]
        let published = return Err(io::ErrorKind::Unsupported.into());
        if published {
            Ok(())
        } else {
            Err(io::Error::other("recording source identity changed"))
        }
    }
    /// Budget only an unchanged owned single-link object at this path.
    /// This observation does not authorize deletion: remove isolates and
    /// verifies the object again before unlinking it.
    pub fn verified_size(&self, path: &Path) -> io::Result<Option<u64>> {
        #[cfg(unix)]
        let candidate = {
            use std::os::unix::fs::OpenOptionsExt;
            std::fs::OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
                .open(path)
        };
        #[cfg(windows)]
        let candidate = crate::windows::open_recorded_candidate(path);
        #[cfg(not(any(unix, windows)))]
        let candidate: io::Result<File> = Err(io::ErrorKind::Unsupported.into());
        let candidate = match candidate {
            Ok(file) => file,
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::NotFound | io::ErrorKind::InvalidInput
                ) =>
            {
                return Ok(None)
            }
            #[cfg(unix)]
            Err(error) if error.raw_os_error() == Some(libc::ELOOP) => return Ok(None),
            Err(error) => return Err(error),
        };
        Ok(self.unchanged(&candidate)?.then_some(self.size))
    }
    fn unchanged(&self, candidate: &File) -> io::Result<bool> {
        let meta = candidate.metadata()?;
        Ok(single_link(&meta)
            && meta.is_file()
            && meta.len() == self.size
            && meta.modified()? == self.modified
            && file_identity(candidate)? == file_identity(&self.file)?)
    }
    #[cfg(unix)]
    fn isolate_unix(&self, path: &Path, destination: Option<&Path>) -> io::Result<bool> {
        use std::{
            ffi::CString,
            os::unix::{
                ffi::OsStrExt,
                io::{AsFd, AsRawFd},
            },
        };
        let parent_path = path
            .parent()
            .ok_or_else(|| io::Error::from(io::ErrorKind::InvalidInput))?;
        let parent = crate::anchored::open_root(parent_path)?;
        let name = CString::new(
            path.file_name()
                .ok_or_else(|| io::Error::from(io::ErrorKind::InvalidInput))?
                .as_bytes(),
        )
        .map_err(|_| io::Error::from(io::ErrorKind::InvalidInput))?;
        let destination = destination
            .map(|path| {
                CString::new(
                    path.file_name()
                        .ok_or(io::ErrorKind::InvalidInput)?
                        .as_bytes(),
                )
                .map_err(|_| io::Error::from(io::ErrorKind::InvalidInput))
            })
            .transpose()?;
        let mut random = [0u8; 16];
        std::io::Read::read_exact(&mut File::open("/dev/urandom")?, &mut random)?;
        let private_name = format!(
            ".vnilink-retention-{}",
            random
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<String>()
        );
        let private_c = CString::new(private_name.as_str()).unwrap();
        // SAFETY: borrowed directory fd and NUL-terminated name remain live.
        if unsafe { libc::mkdirat(parent.as_raw_fd(), private_c.as_ptr(), 0o700) } != 0 {
            return Err(io::Error::last_os_error());
        }
        let private = crate::anchored::open_child(parent.as_fd(), &private_name)?;
        let candidate = CString::new("candidate").unwrap();
        let result = (|| {
            match rename(parent.as_raw_fd(), &name, private.as_raw_fd(), &candidate) {
                Ok(()) => {}
                Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
                Err(error) => return Err(error),
            }
            #[cfg(test)]
            if let Some(hook) = TEST_AFTER_MOVE.with(|h| h.borrow_mut().take()) {
                hook(path);
            }
            // SAFETY: live directory fd; no-follow and nonblocking prevent
            // a foreign symlink or FIFO from redirecting/stalling cleanup.
            let fd = unsafe {
                libc::openat(
                    private.as_raw_fd(),
                    candidate.as_ptr(),
                    libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC,
                )
            };
            let same = if fd < 0 {
                false
            } else {
                use std::os::fd::FromRawFd;
                // SAFETY: openat returned a new owned fd.
                let candidate = unsafe { File::from_raw_fd(fd) };
                self.unchanged(&candidate).unwrap_or(false)
            };
            if same {
                if let Some(destination) = &destination {
                    if let Err(error) = rename(
                        private.as_raw_fd(),
                        &candidate,
                        parent.as_raw_fd(),
                        destination,
                    ) {
                        rename(private.as_raw_fd(), &candidate, parent.as_raw_fd(), &name)?;
                        return Err(error);
                    }
                    return Ok(true);
                }
                // SAFETY: only this operation owns entries in the private
                // namespace. No public pathname is dereferenced by unlink.
                if unsafe { libc::unlinkat(private.as_raw_fd(), candidate.as_ptr(), 0) } != 0 {
                    return Err(io::Error::last_os_error());
                }
                Ok(true)
            } else {
                rename(private.as_raw_fd(), &candidate, parent.as_raw_fd(), &name)?;
                Ok(false)
            }
        })();
        // Never sweep an unrecognized candidate or recursive directory. A
        // nonempty recovery directory deliberately remains after collision.
        // SAFETY: removes only the empty private directory entry.
        unsafe {
            libc::unlinkat(parent.as_raw_fd(), private_c.as_ptr(), libc::AT_REMOVEDIR);
        }
        result
    }
    #[cfg(windows)]
    fn publish_windows(&self, source: &Path, destination: &Path) -> io::Result<bool> {
        let mut random = [0u8; 16];
        crate::windows::random_bytes(&mut random)?;
        let private = source
            .parent()
            .ok_or(io::ErrorKind::InvalidInput)?
            .join(format!(
                ".vnilink-publication-{}",
                random
                    .iter()
                    .map(|b| format!("{b:02x}"))
                    .collect::<String>()
            ));
        std::fs::create_dir(&private)?;
        let candidate = private.join("candidate");
        let result = (|| {
            crate::rename_noreplace(source, &candidate)?;
            let same = crate::windows::open_recorded_candidate(&candidate)
                .and_then(|candidate| self.unchanged(&candidate))
                .unwrap_or(false);
            if !same {
                crate::rename_noreplace(&candidate, source)?;
                return Ok(false);
            }
            if let Err(error) = crate::rename_noreplace(&candidate, destination) {
                crate::rename_noreplace(&candidate, source)?;
                return Err(error);
            }
            Ok(true)
        })();
        let _ = std::fs::remove_dir(&private);
        result
    }
    #[cfg(windows)]
    fn remove_windows(self, path: &Path) -> io::Result<bool> {
        let mut random = [0u8; 16];
        crate::windows::random_bytes(&mut random)?;
        let private = path
            .parent()
            .ok_or_else(|| io::Error::from(io::ErrorKind::InvalidInput))?
            .join(format!(
                ".vnilink-retention-{}",
                random
                    .iter()
                    .map(|b| format!("{b:02x}"))
                    .collect::<String>()
            ));
        std::fs::create_dir(&private)?;
        let candidate = private.join("candidate");
        let result = (|| {
            match crate::rename_noreplace(path, &candidate) {
                Ok(()) => {}
                Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
                Err(error) => return Err(error),
            }
            let same = crate::windows::open_recorded_candidate(&candidate)
                .and_then(|candidate| self.unchanged(&candidate))
                .unwrap_or(false);
            if same {
                crate::windows::delete_file_object(&self.file)?;
                Ok(true)
            } else {
                crate::rename_noreplace(&candidate, path)?;
                Ok(false)
            }
        })();
        // Close the POSIX-disposition handle before removing its namespace.
        drop(self);
        let _ = std::fs::remove_dir(&private);
        result
    }
}

fn file_identity(file: &File) -> io::Result<(u64, u64)> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let m = file.metadata()?;
        Ok((m.dev(), m.ino()))
    }
    #[cfg(windows)]
    {
        crate::windows::file_identity(file)
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = file;
        Err(io::ErrorKind::Unsupported.into())
    }
}

#[cfg(unix)]
fn rename(
    old_dir: std::os::fd::RawFd,
    old: &std::ffi::CStr,
    new_dir: std::os::fd::RawFd,
    new: &std::ffi::CStr,
) -> io::Result<()> {
    #[cfg(target_os = "linux")]
    // SAFETY: live directory fds and borrowed NUL-terminated names.
    let result = unsafe {
        libc::renameat2(
            old_dir,
            old.as_ptr(),
            new_dir,
            new.as_ptr(),
            libc::RENAME_NOREPLACE,
        )
    };
    #[cfg(target_os = "macos")]
    // SAFETY: live directory fds and borrowed NUL-terminated names.
    let result = unsafe {
        libc::renameatx_np(
            old_dir,
            old.as_ptr(),
            new_dir,
            new.as_ptr(),
            libc::RENAME_EXCL,
        )
    };
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        let _ = (old_dir, old, new_dir, new);
        return Err(io::ErrorKind::Unsupported.into());
    }
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    if result == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

#[cfg(all(test, unix))]
thread_local! { static TEST_AFTER_MOVE: std::cell::RefCell<Option<Box<dyn FnOnce(&Path)>>> = const { std::cell::RefCell::new(None) }; }

fn single_link(meta: &std::fs::Metadata) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        meta.nlink() == 1
    }
    #[cfg(not(unix))]
    {
        let _ = meta;
        true
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    fn fixture() -> (std::path::PathBuf, std::path::PathBuf, RecordedFile) {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "vnilink-retention-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&dir).unwrap();
        let path = dir.join("segment.mkv");
        let mut file = crate::create_recording_file(&path).unwrap();
        std::io::Write::write_all(&mut file, b"recorded segment").unwrap();
        let owner = RecordedFile::new(&file).unwrap();
        (dir, path, owner)
    }
    #[test]
    fn owned_file_deletes_but_concurrent_public_replacement_survives() {
        let (dir, path, owner) = fixture();
        TEST_AFTER_MOVE.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(|path| {
                std::fs::write(path, b"concurrent user file").unwrap()
            }))
        });
        assert!(owner.remove(&path).unwrap());
        assert_eq!(std::fs::read(&path).unwrap(), b"concurrent user file");
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn replacement_restore_collision_preserves_both_foreign_objects() {
        let (dir, path, owner) = fixture();
        std::fs::rename(&path, dir.join("saved.mkv")).unwrap();
        std::fs::write(&path, b"first foreign file").unwrap();
        TEST_AFTER_MOVE.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(|path| {
                std::fs::write(path, b"second foreign file").unwrap()
            }))
        });
        assert_eq!(
            owner.remove(&path).unwrap_err().kind(),
            io::ErrorKind::AlreadyExists
        );
        assert_eq!(std::fs::read(&path).unwrap(), b"second foreign file");
        let recovery = std::fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().path())
            .find(|p| p.is_dir())
            .unwrap();
        assert_eq!(
            std::fs::read(recovery.join("candidate")).unwrap(),
            b"first foreign file"
        );
        assert_eq!(
            std::fs::read(dir.join("saved.mkv")).unwrap(),
            b"recorded segment"
        );
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn changed_file_and_hardlink_alias_are_preserved() {
        let (dir, path, owner) = fixture();
        std::fs::write(&path, b"edited content").unwrap();
        assert!(!owner.remove(&path).unwrap());
        assert_eq!(std::fs::read(&path).unwrap(), b"edited content");
        let file = File::open(&path).unwrap();
        let owner = RecordedFile::new(&file).unwrap();
        std::fs::hard_link(&path, dir.join("alias")).unwrap();
        assert!(!owner.remove(&path).unwrap());
        assert_eq!(std::fs::read(dir.join("alias")).unwrap(), b"edited content");
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn symlink_and_fifo_replacements_are_preserved_without_blocking() {
        use std::os::unix::{ffi::OsStrExt, fs::symlink};
        let (dir, path, owner) = fixture();
        std::fs::rename(&path, dir.join("saved.mkv")).unwrap();
        symlink("saved.mkv", &path).unwrap();
        assert!(!owner.remove(&path).unwrap());
        assert_eq!(
            std::fs::read_link(&path).unwrap(),
            std::path::PathBuf::from("saved.mkv")
        );
        std::fs::remove_file(&path).unwrap();
        let owner = RecordedFile::new(&File::open(dir.join("saved.mkv")).unwrap()).unwrap();
        let name = std::ffi::CString::new(path.as_os_str().as_bytes()).unwrap();
        // SAFETY: valid NUL-terminated path, test-owned temporary directory.
        assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
        assert!(!owner.remove(&path).unwrap());
        use std::os::unix::fs::FileTypeExt;
        assert!(std::fs::symlink_metadata(&path)
            .unwrap()
            .file_type()
            .is_fifo());
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn publication_isolates_original_before_public_source_churn() {
        let (dir, path, owner) = fixture();
        let destination = dir.join("finished.mkv");
        TEST_AFTER_MOVE.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(|path| {
                std::fs::write(path, b"concurrent user file").unwrap()
            }))
        });
        owner.publish(&path, &destination).unwrap();
        assert_eq!(std::fs::read(destination).unwrap(), b"recorded segment");
        assert_eq!(std::fs::read(path).unwrap(), b"concurrent user file");
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 2);
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn publication_restore_collision_keeps_all_objects() {
        let (dir, path, owner) = fixture();
        let destination = dir.join("finished.mkv");
        std::fs::rename(&path, dir.join("saved.mkv")).unwrap();
        std::fs::write(&path, b"first foreign file").unwrap();
        TEST_AFTER_MOVE.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(|path| {
                std::fs::write(path, b"second foreign file").unwrap()
            }))
        });
        assert_eq!(
            owner.publish(&path, &destination).unwrap_err().kind(),
            io::ErrorKind::AlreadyExists
        );
        assert!(!destination.exists());
        assert_eq!(std::fs::read(&path).unwrap(), b"second foreign file");
        let recovery = std::fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().path())
            .find(|p| p.is_dir())
            .unwrap();
        assert_eq!(
            std::fs::read(recovery.join("candidate")).unwrap(),
            b"first foreign file"
        );
        assert_eq!(
            std::fs::read(dir.join("saved.mkv")).unwrap(),
            b"recorded segment"
        );
        std::fs::remove_dir_all(dir).unwrap();
    }
}
