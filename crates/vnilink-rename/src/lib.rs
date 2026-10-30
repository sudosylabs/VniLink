// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! Atomic rename-without-replacement for the VniLink file manager.
//!
//! The shared `vnilink` library forbids unsafe code, so the two syscalls
//! that give atomic no-replace semantics live here, in one audited place:
//!
//! - Linux: `renameat2` with `RENAME_NOREPLACE`.
//! - macOS: `renameatx_np` with `RENAME_EXCL`.
//!
//! Windows uses MoveFileExW without replacement or copy flags. Remaining
//! targets use the documented check-then-rename fallback.
//!
//! [`anchored`] holds the handle-relative operations the shared disk is
//! walked with, on Unix and Windows alike.

#[cfg(windows)]
pub mod windows;

/// Handle-relative operations under a shared root: one API, opened with
/// `openat` on Unix and `NtCreateFile` relative to a handle on Windows.
#[cfg(unix)]
pub mod anchored;
#[cfg(windows)]
#[path = "anchored_windows.rs"]
pub mod anchored;

pub mod recording_retention;

use std::io;
use std::path::Path;

/// How a volume names its entries, read from the volume a directory
/// handle is on ([`anchored::volume_ids`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VolumeIds {
    /// File ids are object identities (NTFS, ReFS, APFS, HFS+, ext4,
    /// XFS, Btrfs and most others): an id names one object for its life.
    Stable,
    /// File ids derive from an entry's position in its folder (FAT and
    /// exFAT, and a Windows volume reporting no usable id): a replaced
    /// entry can take the listed one's id. Its times are kept to these
    /// granularities, in milliseconds.
    Positional { mtime_ms: u64, born_ms: u64 },
}

impl VolumeIds {
    /// FAT12, FAT16 and FAT32: modification times to 2 s, creation times
    /// to 10 ms.
    pub const FAT: Self = Self::Positional {
        mtime_ms: 2_000,
        born_ms: 10,
    };
    /// exFAT: both times to 10 ms.
    pub const EXFAT: Self = Self::Positional {
        mtime_ms: 10,
        born_ms: 10,
    };
}

/// Open an existing regular file for in-place recovery, refusing final symlinks
/// and multiply linked files. Callers must still validate their own file format.
pub fn open_single_link_file_rw(path: &Path) -> io::Result<std::fs::File> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(path)?;
        let metadata = file.metadata()?;
        if !metadata.is_file() || metadata.nlink() != 1 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "recovery requires a single-link regular file",
            ));
        }
        Ok(file)
    }
    #[cfg(windows)]
    {
        windows::open_single_link_file_rw(path)
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = path;
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "native recovery file checks unavailable",
        ))
    }
}

#[cfg(test)]
mod recovery_open_tests {
    use super::*;
    #[test]
    fn recovery_open_refuses_aliases_and_preserves_the_original() {
        let folder = std::env::temp_dir().join(format!(
            "vnilink-recovery-open-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&folder).unwrap();
        let file = folder.join("recording.part");
        std::fs::write(&file, b"unchanged fixture").unwrap();
        drop(open_single_link_file_rw(&file).unwrap());
        assert!(open_single_link_file_rw(&folder).is_err());
        let alias = folder.join("hardlink.part");
        std::fs::hard_link(&file, &alias).unwrap();
        assert!(open_single_link_file_rw(&file).is_err());
        assert!(open_single_link_file_rw(&alias).is_err());
        std::fs::remove_file(&alias).unwrap();
        #[cfg(unix)]
        {
            let symlink = folder.join("symlink.part");
            std::os::unix::fs::symlink(&file, &symlink).unwrap();
            assert!(open_single_link_file_rw(&symlink).is_err());
        }
        assert_eq!(std::fs::read(&file).unwrap(), b"unchanged fixture");
        drop(open_single_link_file_rw(&file).unwrap());
        std::fs::remove_dir_all(folder).unwrap();
    }
}

/// Renames `old` to `new` without replacing an existing destination.
///
/// On success the source is gone and the destination is the renamed object.
/// When the destination already exists the rename fails with
/// [`io::ErrorKind::AlreadyExists`] and both paths are untouched. A missing
/// source fails with [`io::ErrorKind::NotFound`].
///
/// Residual race: on platforms without an atomic primitive this checks for
/// the destination first and then renames, so a destination created between
/// the check and the rename can still be replaced. Linux, macOS and Windows take the
/// atomic path; remaining targets document this gap.
pub fn rename_noreplace(old: &Path, new: &Path) -> io::Result<()> {
    #[cfg(target_os = "linux")]
    {
        rename_linux(old, new)
    }
    #[cfg(target_os = "macos")]
    {
        rename_macos(old, new)
    }
    #[cfg(windows)]
    {
        windows::rename_noreplace(old, new)
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
    {
        rename_fallback(old, new)
    }
}

#[cfg(target_os = "linux")]
fn rename_linux(old: &Path, new: &Path) -> io::Result<()> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    let old = CString::new(old.as_os_str().as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "old path holds NUL"))?;
    let new = CString::new(new.as_os_str().as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "new path holds NUL"))?;
    // SAFETY: renameat2 takes borrowed NUL-terminated path pointers and an
    // open-coded flag set; both pointers outlive the call and no Rust
    // aliasing rules are involved.
    let result = unsafe {
        libc::renameat2(
            libc::AT_FDCWD,
            old.as_ptr(),
            libc::AT_FDCWD,
            new.as_ptr(),
            libc::RENAME_NOREPLACE,
        )
    };
    if result == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

#[cfg(target_os = "macos")]
fn rename_macos(old: &Path, new: &Path) -> io::Result<()> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    let old = CString::new(old.as_os_str().as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "old path holds NUL"))?;
    let new = CString::new(new.as_os_str().as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "new path holds NUL"))?;
    // SAFETY: same contract as renameat2 above; RENAME_EXCL gives the
    // no-replace behavior on volumes that support it.
    let result = unsafe {
        libc::renameatx_np(
            libc::AT_FDCWD,
            old.as_ptr(),
            libc::AT_FDCWD,
            new.as_ptr(),
            libc::RENAME_EXCL,
        )
    };
    if result == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
fn rename_fallback(old: &Path, new: &Path) -> io::Result<()> {
    if std::fs::symlink_metadata(new).is_ok() {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "destination already exists",
        ));
    }
    std::fs::rename(old, new)
}

/// Create a recorder-owned file with native retention rights. Exclusive
/// creation prevents overwriting, while Windows grants delete-by-object rights.
pub fn create_recording_file(path: &Path) -> io::Result<std::fs::File> {
    let mut options = std::fs::OpenOptions::new();
    options.read(true).write(true).create_new(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options
            .access_mode(0x8000_0000 | 0x4000_0000 | 0x0001_0000)
            .share_mode(1 | 2 | 4);
    }
    options.open(path)
}
