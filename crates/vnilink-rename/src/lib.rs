//! Atomic rename-without-replacement for the VniLink file manager.
//!
//! The shared `vnilink` library forbids unsafe code, so the two syscalls
//! that give atomic no-replace semantics live here, in one audited place:
//!
//! - Linux: `renameat2` with `RENAME_NOREPLACE`.
//! - macOS: `renameatx_np` with `RENAME_EXCL`.
//!
//! Every other platform uses a check-then-rename fallback whose residual
//! race is documented at [`rename_noreplace`]. Windows gets native
//! no-replace behavior later through its own adapter (`MoveFileEx` without
//! `MOVEFILE_REPLACE_EXISTING`); that validation belongs to the Windows
//! milestone, not to this primitive.

#[cfg(unix)]
pub mod anchored;

use std::io;
use std::path::Path;

/// Renames `old` to `new` without replacing an existing destination.
///
/// On success the source is gone and the destination is the renamed object.
/// When the destination already exists the rename fails with
/// [`io::ErrorKind::AlreadyExists`] and both paths are untouched. A missing
/// source fails with [`io::ErrorKind::NotFound`].
///
/// Residual race: on platforms without an atomic primitive this checks for
/// the destination first and then renames, so a destination created between
/// the check and the rename can still be replaced. Linux and macOS take the
/// atomic path; every other target documents this gap until its native
/// no-replace primitive lands.
pub fn rename_noreplace(old: &Path, new: &Path) -> io::Result<()> {
    #[cfg(target_os = "linux")]
    {
        rename_linux(old, new)
    }
    #[cfg(target_os = "macos")]
    {
        rename_macos(old, new)
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
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

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn rename_fallback(old: &Path, new: &Path) -> io::Result<()> {
    if std::fs::symlink_metadata(new).is_ok() {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "destination already exists",
        ));
    }
    std::fs::rename(old, new)
}
