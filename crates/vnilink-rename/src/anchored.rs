//! Directory-handle confinement for owner-approved roots.
//!
//! The shared `vnilink` library forbids unsafe code, so every `openat`,
//! `mkdirat`, `renameat` and `fdopendir` dance lives here. Callers in
//! `vnilink` hold directory handles (`File` opened `O_NOFOLLOW|O_DIRECTORY`)
//! from the approved root down, never bare path strings, so a parent
//! replaced by a symlink between listing and mutation is refused instead of
//! escaping the approved folder.

use std::ffi::{CString, OsString};
use std::io;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::io::{AsRawFd, BorrowedFd, FromRawFd, OwnedFd};
use std::path::Path;

fn cstr(path: &Path) -> io::Result<CString> {
    CString::new(path.as_os_str().as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "path holds NUL"))
}

fn cstr_name(name: &str) -> io::Result<CString> {
    if name.is_empty() || name.contains('/') || name.contains('\0') {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "name is not a single component",
        ));
    }
    CString::new(name).map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "name holds NUL"))
}

/// Opens an approved root itself: `O_NOFOLLOW|O_DIRECTORY|O_CLOEXEC`, so a
/// root replaced by a symlink fails here instead of escaping.
pub fn open_root(path: &Path) -> io::Result<OwnedFd> {
    let c = cstr(path)?;
    // SAFETY: `open` takes a borrowed NUL-terminated path plus flags/mode;
    // the pointer outlives the call.
    let fd = unsafe {
        libc::open(
            c.as_ptr(),
            libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_DIRECTORY | libc::O_CLOEXEC,
        )
    };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: `open` returned a fresh owned fd on success.
    Ok(unsafe { OwnedFd::from_raw_fd(fd) })
}

/// Opens one child directory under `parent` without following symlinks.
pub fn open_child(parent: BorrowedFd, name: &str) -> io::Result<OwnedFd> {
    let c = cstr_name(name)?;
    // SAFETY: `openat` takes a borrowed dir fd plus a borrowed name; both
    // outlive the call.
    let fd = unsafe {
        libc::openat(
            parent.as_raw_fd(),
            c.as_ptr(),
            libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_DIRECTORY | libc::O_CLOEXEC,
        )
    };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: fresh owned fd on success.
    Ok(unsafe { OwnedFd::from_raw_fd(fd) })
}

/// Creates one directory level under `parent`. `AlreadyExists` maps to
/// `ErrorKind::AlreadyExists` for conflict handling.
pub fn mkdir_at(parent: BorrowedFd, name: &str) -> io::Result<()> {
    let c = cstr_name(name)?;
    // SAFETY: `mkdirat` takes a borrowed dir fd plus a borrowed name.
    let r = unsafe { libc::mkdirat(parent.as_raw_fd(), c.as_ptr(), 0o755) };
    if r == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

/// Lists entry names (excluding `.`/`..`) under `dir` via `fdopendir`.
pub fn list_names(dir: BorrowedFd) -> io::Result<Vec<OsString>> {
    // Duplicate: `fdopendir` takes ownership; the caller's fd must stay alive.
    // SAFETY: `dup` returns a fresh fd on success.
    let dup = unsafe { libc::dup(dir.as_raw_fd()) };
    if dup < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: `fdopendir` takes ownership of a fresh `dup` fd; on failure we
    // close it ourselves.
    let dirp = unsafe { libc::fdopendir(dup) };
    if dirp.is_null() {
        unsafe { libc::close(dup) };
        return Err(io::Error::last_os_error());
    }
    // Clear errno before the loop so a null `readdir` with preserved zero
    // means clean end-of-directory.
    #[cfg(target_os = "linux")]
    unsafe {
        *libc::__errno_location() = 0
    };
    #[cfg(target_os = "macos")]
    unsafe {
        *libc::__error() = 0
    };
    let mut out = Vec::new();
    loop {
        // SAFETY: `readdir` on our own DIR*; null means end or error.
        let ent = unsafe { libc::readdir(dirp) };
        if ent.is_null() {
            #[cfg(target_os = "linux")]
            let e = unsafe { *libc::__errno_location() };
            #[cfg(target_os = "macos")]
            let e = unsafe { *libc::__error() };
            #[cfg(not(any(target_os = "linux", target_os = "macos")))]
            let e = 0;
            // SAFETY: always close our DIR*.
            unsafe { libc::closedir(dirp) };
            if e != 0 {
                return Err(io::Error::from_raw_os_error(e));
            }
            break;
        }
        // SAFETY: `d_name` is NUL-terminated inside the dirent.
        let name = unsafe {
            let ptr = (*ent).d_name.as_ptr() as *const libc::c_char;
            std::ffi::CStr::from_ptr(ptr).to_bytes()
        };
        if name == b"." || name == b".." {
            continue;
        }
        out.push(OsString::from_vec(name.to_vec()));
    }
    Ok(out)
}

/// `fstat` device+inode for pinning a root to its directory object.
pub fn fstat_dev_ino(fd: BorrowedFd) -> io::Result<(u64, u64)> {
    let mut st: libc::stat = unsafe { std::mem::zeroed() };
    // SAFETY: `fstat` writes a borrowed stat struct.
    let r = unsafe { libc::fstat(fd.as_raw_fd(), &mut st) };
    if r != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok((st.st_dev as u64, st.st_ino as u64))
}

/// `fstatat` with `AT_SYMLINK_NOFOLLOW`: kind/size/mtime plus identity for
/// one entry under `parent` without following it.
pub struct EntryStat {
    pub is_symlink: bool,
    pub is_dir: bool,
    pub is_file: bool,
    pub size: u64,
    pub mtime_ms: u64,
    pub dev: u64,
    pub ino: u64,
}

pub fn stat_at(parent: BorrowedFd, name: &str) -> io::Result<EntryStat> {
    let c = cstr_name(name)?;
    let mut st: libc::stat = unsafe { std::mem::zeroed() };
    // SAFETY: `fstatat` takes a borrowed dir fd plus a borrowed name.
    let r = unsafe {
        libc::fstatat(
            parent.as_raw_fd(),
            c.as_ptr(),
            &mut st,
            libc::AT_SYMLINK_NOFOLLOW,
        )
    };
    if r != 0 {
        return Err(io::Error::last_os_error());
    }
    let mode = st.st_mode as libc::mode_t;
    Ok(EntryStat {
        is_symlink: (mode & libc::S_IFMT) == libc::S_IFLNK,
        is_dir: (mode & libc::S_IFMT) == libc::S_IFDIR,
        is_file: (mode & libc::S_IFMT) == libc::S_IFREG,
        size: st.st_size as u64,
        mtime_ms: mtime_ms(&st),
        dev: st.st_dev as u64,
        ino: st.st_ino as u64,
    })
}

#[cfg(target_os = "linux")]
fn mtime_ms(st: &libc::stat) -> u64 {
    let sec = st.st_mtime as i128;
    let nsec = st.st_mtime_nsec as i128;
    if sec < 0 {
        return 0;
    }
    ((sec as u128 * 1_000_000_000 + nsec as u128) / 1_000_000) as u64
}

#[cfg(target_os = "macos")]
fn mtime_ms(st: &libc::stat) -> u64 {
    let sec = st.st_mtime as i128;
    let nsec = st.st_mtime_nsec as i128;
    if sec < 0 {
        return 0;
    }
    ((sec as u128 * 1_000_000_000 + nsec as u128) / 1_000_000) as u64
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn mtime_ms(_st: &libc::stat) -> u64 {
    0
}

/// Same-directory atomic no-replace rename using dir handles.
pub fn rename_at_noreplace(dir: BorrowedFd, old: &str, new: &str) -> io::Result<()> {
    let old_c = cstr_name(old)?;
    let new_c = cstr_name(new)?;
    #[cfg(target_os = "linux")]
    {
        // SAFETY: `renameat2` takes borrowed dir fds plus borrowed names.
        let r = unsafe {
            libc::renameat2(
                dir.as_raw_fd(),
                old_c.as_ptr(),
                dir.as_raw_fd(),
                new_c.as_ptr(),
                libc::RENAME_NOREPLACE,
            )
        };
        if r == 0 {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        }
    }
    #[cfg(target_os = "macos")]
    {
        // SAFETY: same contract with RENAME_EXCL.
        let r = unsafe {
            libc::renameatx_np(
                dir.as_raw_fd(),
                old_c.as_ptr(),
                dir.as_raw_fd(),
                new_c.as_ptr(),
                libc::RENAME_EXCL,
            )
        };
        if r == 0 {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        }
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        let _ = (dir, old_c, new_c);
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "anchored rename needs a native primitive",
        ))
    }
}

/// Cross-directory rename for publication: staged (outside) into an anchored
/// destination dir. `replace` allows overwrite; otherwise no-replace.
/// Across filesystems (`EXDEV`) falls back to copy + atomic rename of a
/// temp sibling inside the destination (same filesystem), preserving
/// no-replace semantics for KeepBoth via the final `renameat`.
pub fn rename_from_staged(
    staged: &Path,
    dest_dir: BorrowedFd,
    dest_name: &str,
    replace: bool,
) -> io::Result<()> {
    let old_c = cstr(staged)?;
    let new_c = cstr_name(dest_name)?;
    let new_fd = dest_dir.as_raw_fd();
    if replace {
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        {
            // SAFETY: `renameat` takes AT_FDCWD plus absolute old path and a
            // borrowed new dir fd plus name. The staged path was verified
            // (size-checked export) before this commit point.
            let r =
                unsafe { libc::renameat(libc::AT_FDCWD, old_c.as_ptr(), new_fd, new_c.as_ptr()) };
            if r == 0 {
                Ok(())
            } else {
                let e = io::Error::last_os_error();
                if e.raw_os_error() == Some(libc::EXDEV) {
                    copy_then_rename(staged, dest_dir, dest_name, true)
                } else {
                    Err(e)
                }
            }
        }
        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
        {
            // Fallback: staged is outside the jail; dest anchoring still
            // applies to the new name's parent via dir fd is unavailable,
            // so refuse rather than pathname-rename.
            let _ = (old_c, new_fd, new_c);
            Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "anchored publish needs a native primitive",
            ))
        }
    } else {
        // KeepBoth free-name commit must not replace: use the noreplace
        // primitive with the staged absolute source.
        #[cfg(target_os = "linux")]
        {
            // SAFETY: same as above with RENAME_NOREPLACE.
            let r = unsafe {
                libc::renameat2(
                    libc::AT_FDCWD,
                    old_c.as_ptr(),
                    new_fd,
                    new_c.as_ptr(),
                    libc::RENAME_NOREPLACE,
                )
            };
            if r == 0 {
                Ok(())
            } else {
                let e = io::Error::last_os_error();
                if e.raw_os_error() == Some(libc::EXDEV) {
                    copy_then_rename(staged, dest_dir, dest_name, false)
                } else {
                    Err(e)
                }
            }
        }
        #[cfg(target_os = "macos")]
        {
            let r = unsafe {
                libc::renameatx_np(
                    libc::AT_FDCWD,
                    old_c.as_ptr(),
                    new_fd,
                    new_c.as_ptr(),
                    libc::RENAME_EXCL,
                )
            };
            if r == 0 {
                Ok(())
            } else {
                let e = io::Error::last_os_error();
                // macOS `renameatx_np` reports EXDEV for cross-device moves.
                if e.raw_os_error() == Some(libc::EXDEV) {
                    copy_then_rename(staged, dest_dir, dest_name, false)
                } else {
                    Err(e)
                }
            }
        }
        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
        {
            let _ = (old_c, new_fd, new_c);
            Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "anchored publish needs a native primitive",
            ))
        }
    }
}

/// Cross-filesystem fallback for `rename_from_staged`: copies `staged`
/// bytes to a temp sibling inside `dest_dir` (same filesystem), fsyncs,
/// then atomically renames the temp onto `dest_name` with replace/no-replace
/// semantics. The temp is unlinked on any failure; the staged source is
/// removed on success (move semantics, like `rename`), left for the caller
/// to clean on failure paths that return `AlreadyExists` for conflict
/// re-entry.
#[cfg(any(target_os = "linux", target_os = "macos"))]
fn copy_then_rename(
    staged: &Path,
    dest_dir: BorrowedFd,
    dest_name: &str,
    replace: bool,
) -> io::Result<()> {
    use std::os::unix::io::AsRawFd;
    let dest_c = cstr_name(dest_name)?;
    // Unique temp sibling inside the destination (same filesystem for the
    // final atomic rename).
    for attempt in 0..32u32 {
        let temp = format!(".vnlpub.{}.tmp", attempt);
        // `O_EXCL` ensures we never truncate an existing temp.
        let temp_c = cstr_name(&temp)?;
        // SAFETY: `openat` with borrowed dir fd + name.
        let wfd = unsafe {
            libc::openat(
                dest_dir.as_raw_fd(),
                temp_c.as_ptr(),
                libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_CLOEXEC,
                0o644,
            )
        };
        if wfd < 0 {
            let e = io::Error::last_os_error();
            if e.kind() == io::ErrorKind::AlreadyExists {
                continue;
            }
            return Err(e);
        }
        // SAFETY: fresh owned fd; closed on every path below via `close`
        // or conversion. Use manual `read`/`write` loop to stream without
        // holding whole files in memory (up to manifest limits).
        let result: io::Result<()> = (|| {
            // SAFETY: `open` staged for reading (regular file from export).
            let staged_c = cstr(staged)?;
            let rfd = unsafe { libc::open(staged_c.as_ptr(), libc::O_RDONLY | libc::O_CLOEXEC) };
            if rfd < 0 {
                unsafe { libc::close(wfd) };
                // Best effort: remove temp.
                unsafe { libc::unlinkat(dest_dir.as_raw_fd(), temp_c.as_ptr(), 0) };
                return Err(io::Error::last_os_error());
            }
            let mut buf = vec![0u8; 64 * 1024];
            loop {
                // SAFETY: `read` into borrowed buffer.
                let n =
                    unsafe { libc::read(rfd, buf.as_mut_ptr() as *mut libc::c_void, buf.len()) };
                if n < 0 {
                    unsafe { libc::close(rfd) };
                    unsafe { libc::close(wfd) };
                    unsafe { libc::unlinkat(dest_dir.as_raw_fd(), temp_c.as_ptr(), 0) };
                    return Err(io::Error::last_os_error());
                }
                if n == 0 {
                    break;
                }
                let mut written = 0isize;
                while written < n {
                    // SAFETY: `write` from borrowed buffer range.
                    let m = unsafe {
                        libc::write(
                            wfd,
                            buf.as_ptr().add(written as usize) as *const libc::c_void,
                            (n - written) as usize,
                        )
                    };
                    if m <= 0 {
                        unsafe { libc::close(rfd) };
                        unsafe { libc::close(wfd) };
                        unsafe { libc::unlinkat(dest_dir.as_raw_fd(), temp_c.as_ptr(), 0) };
                        return Err(io::Error::last_os_error());
                    }
                    written += m;
                }
            }
            unsafe { libc::close(rfd) };
            // SAFETY: `fsync` the temp before the atomic rename.
            if unsafe { libc::fsync(wfd) } != 0 {
                unsafe { libc::close(wfd) };
                unsafe { libc::unlinkat(dest_dir.as_raw_fd(), temp_c.as_ptr(), 0) };
                return Err(io::Error::last_os_error());
            }
            unsafe { libc::close(wfd) };
            // Atomic commit inside the destination (same filesystem).
            #[cfg(target_os = "linux")]
            {
                if replace {
                    // SAFETY: `renameat` within one dir fd (overwrite allowed).
                    let r = unsafe {
                        libc::renameat(
                            dest_dir.as_raw_fd(),
                            temp_c.as_ptr(),
                            dest_dir.as_raw_fd(),
                            dest_c.as_ptr(),
                        )
                    };
                    if r != 0 {
                        unsafe { libc::unlinkat(dest_dir.as_raw_fd(), temp_c.as_ptr(), 0) };
                        return Err(io::Error::last_os_error());
                    }
                    Ok(())
                } else {
                    // SAFETY: no-replace commit decides atomically.
                    let r = unsafe {
                        libc::renameat2(
                            dest_dir.as_raw_fd(),
                            temp_c.as_ptr(),
                            dest_dir.as_raw_fd(),
                            dest_c.as_ptr(),
                            libc::RENAME_NOREPLACE,
                        )
                    };
                    if r != 0 {
                        let e = io::Error::last_os_error();
                        unsafe { libc::unlinkat(dest_dir.as_raw_fd(), temp_c.as_ptr(), 0) };
                        return Err(e);
                    }
                    Ok(())
                }
            }
            #[cfg(target_os = "macos")]
            {
                if replace {
                    // SAFETY: `renameat` within one dir (overwrite allowed).
                    let r = unsafe {
                        libc::renameat(
                            dest_dir.as_raw_fd(),
                            temp_c.as_ptr(),
                            dest_dir.as_raw_fd(),
                            dest_c.as_ptr(),
                        )
                    };
                    if r != 0 {
                        unsafe { libc::unlinkat(dest_dir.as_raw_fd(), temp_c.as_ptr(), 0) };
                        return Err(io::Error::last_os_error());
                    }
                    Ok(())
                } else {
                    let r = unsafe {
                        libc::renameatx_np(
                            dest_dir.as_raw_fd(),
                            temp_c.as_ptr(),
                            dest_dir.as_raw_fd(),
                            dest_c.as_ptr(),
                            libc::RENAME_EXCL,
                        )
                    };
                    if r != 0 {
                        let e = io::Error::last_os_error();
                        unsafe { libc::unlinkat(dest_dir.as_raw_fd(), temp_c.as_ptr(), 0) };
                        return Err(e);
                    }
                    Ok(())
                }
            }
        })();
        match result {
            Ok(()) => {
                // Move semantics like `rename`: remove the staged source.
                let _ = std::fs::remove_file(staged);
                return Ok(());
            }
            Err(e) => {
                // Temp-name collision: try next temp. Other errors (including
                // `AlreadyExists` on the final no-replace commit) propagate
                // so the caller re-enters conflict instead of failing.
                if e.kind() == io::ErrorKind::AlreadyExists {
                    // Could be temp collision (retry) or final dest collision
                    // (propagate for conflict). Distinguish by checking dest?
                    // Our temp names are unique per attempt; `AlreadyExists`
                    // here comes from the final commit (dest exists) since temp
                    // creation uses `O_EXCL` and retries on collision above.
                    // Actually temp creation collision continues above, so any
                    // `AlreadyExists` here is the final commit → propagate.
                    return Err(e);
                }
                // For other errors, do not retry different temps endlessly;
                // propagate (caller marks failed).
                return Err(e);
            }
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "no free temp name",
    ))
}

/// Opens a regular file under `parent` and verifies it is the same object
/// previously stated (`stat_at`): device+inode equality proves the pathname
/// was not swapped between stat and open. Refuses symlinks and non-files.
pub fn open_file_verified(
    parent: BorrowedFd,
    name: &str,
    expected_dev: u64,
    expected_ino: u64,
) -> io::Result<OwnedFd> {
    let fd = open_file_at(parent, name)?;
    let mut st: libc::stat = unsafe { std::mem::zeroed() };
    use std::os::unix::io::AsRawFd;
    // SAFETY: `fstat` on our own open fd.
    let r = unsafe { libc::fstat(fd.as_raw_fd(), &mut st) };
    if r != 0 {
        return Err(io::Error::last_os_error());
    }
    if st.st_dev as u64 != expected_dev || st.st_ino as u64 != expected_ino {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "file changed between stat and open",
        ));
    }
    Ok(fd)
}

/// Path that re-opens an already-open file handle for APIs taking paths
/// (`FsStore::add_path`). The handle was opened `O_NOFOLLOW` from an
/// anchored parent, so reading through it cannot escape even when the
/// original pathname was swapped for a symlink afterwards.
pub fn fd_path(fd: BorrowedFd) -> std::path::PathBuf {
    use std::os::unix::io::AsRawFd;
    #[cfg(target_os = "linux")]
    {
        std::path::PathBuf::from(format!("/proc/self/fd/{}", fd.as_raw_fd()))
    }
    #[cfg(not(target_os = "linux"))]
    {
        // macOS (and other Unix): `/dev/fd/N` refers to the same open
        // description process-wide, like `/proc/self/fd` on Linux.
        std::path::PathBuf::from(format!("/dev/fd/{}", fd.as_raw_fd()))
    }
}

/// Opens a regular file under `parent` without following symlinks, for
/// source reads. Refuses non-files.
pub fn open_file_at(parent: BorrowedFd, name: &str) -> io::Result<OwnedFd> {
    let c = cstr_name(name)?;
    // SAFETY: `openat` with NOFOLLOW; non-regulars are refused via fstat below.
    let fd = unsafe {
        libc::openat(
            parent.as_raw_fd(),
            c.as_ptr(),
            libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        )
    };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: fresh owned fd.
    let owned = unsafe { OwnedFd::from_raw_fd(fd) };
    let mut st: libc::stat = unsafe { std::mem::zeroed() };
    let r = unsafe { libc::fstat(owned.as_raw_fd(), &mut st) };
    if r != 0 {
        return Err(io::Error::last_os_error());
    }
    if (st.st_mode as libc::mode_t & libc::S_IFMT) != libc::S_IFREG {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "not a regular file",
        ));
    }
    Ok(owned)
}
