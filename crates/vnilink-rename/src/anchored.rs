//! Handle-relative operations under a shared root.
//!
//! The shared `vnilink` library forbids unsafe code, so every `openat`,
//! `mkdirat`, `renameat` and `fdopendir` dance lives here. Callers in
//! `vnilink` hold directory handles (`File` opened `O_NOFOLLOW|O_DIRECTORY`)
//! from the root down, never bare path strings: once a caller has resolved
//! a path, a folder on it replaced by a symlink before or during the walk
//! is refused instead of redirecting the operation, and an entry acted on
//! through a handle is acted on as itself, never followed. Which links a
//! path may follow before the walk is the caller's choice.

use std::ffi::{CString, OsString};
use std::io;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::io::{AsRawFd, BorrowedFd, FromRawFd, OwnedFd};
use std::path::Path;

/// An open directory: the handle every child operation is relative to.
pub type Dir = OwnedFd;
/// A borrowed [`Dir`].
pub type DirRef<'a> = BorrowedFd<'a>;

/// Borrows a directory handle as [`DirRef`], whatever the platform's handle.
pub trait AsDir {
    fn as_dir(&self) -> DirRef<'_>;
}

impl<T: std::os::fd::AsFd> AsDir for T {
    fn as_dir(&self) -> DirRef<'_> {
        self.as_fd()
    }
}

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

/// Opens a root itself: `O_NOFOLLOW|O_DIRECTORY|O_CLOEXEC`, so a root
/// replaced by a symlink fails here instead of being followed.
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

/// [`open_root`] for a mutation. A Unix descriptor cannot keep a folder
/// from being renamed, so this is the same open; Windows holds the folder.
pub fn open_root_held(path: &Path) -> io::Result<OwnedFd> {
    open_root(path)
}

/// [`open_child`] for a mutation; the same open on Unix (see
/// [`open_root_held`]).
pub fn open_child_held(parent: BorrowedFd, name: &str) -> io::Result<OwnedFd> {
    open_child(parent, name)
}

/// How the volume `dir` is on names its entries. Most Unix file systems
/// keep stable inode numbers; FAT and exFAT derive them from an entry's
/// position. macOS names the file system in `f_fstypename`: `msdos` and
/// `exfat` for its own drivers, and a third-party FAT driver's name holds
/// `fat` or `msdos` (`ufsd_ExFAT`). A macFUSE mount names only `macfuse`
/// or `osxfuse`, whatever it serves, and is taken as stable. Linux reports
/// `MSDOS_SUPER_MAGIC` (vfat and msdos) and `EXFAT_SUPER_MAGIC` in `f_type`;
/// a FUSE mount reports FUSE's magic, and its subtype (`fuseblk.exfat`,
/// `fuse.fusefat`) is read from `/proc/self/mountinfo` by the descriptor's
/// mount id.
pub fn volume_ids(dir: BorrowedFd) -> io::Result<crate::VolumeIds> {
    let mut st: libc::statfs = unsafe { std::mem::zeroed() };
    // SAFETY: `fstatfs` writes the struct it is given for a live fd.
    if unsafe { libc::fstatfs(dir.as_raw_fd(), &mut st) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(volume_ids_of(&st, dir))
}

/// A file system named `name` (any case) that is FAT or exFAT: exFAT when
/// the name says so, FAT when it holds `fat` or `msdos`.
#[cfg_attr(
    not(any(test, target_os = "macos", target_os = "linux")),
    allow(dead_code)
)]
fn positional_by_name(name: &str) -> Option<crate::VolumeIds> {
    let name = name.to_ascii_lowercase();
    if name.contains("exfat") {
        Some(crate::VolumeIds::EXFAT)
    } else if name.contains("fat") || name.contains("msdos") {
        Some(crate::VolumeIds::FAT)
    } else {
        None
    }
}

#[cfg(target_os = "macos")]
fn volume_ids_of(st: &libc::statfs, _dir: BorrowedFd) -> crate::VolumeIds {
    let name: Vec<u8> = st
        .f_fstypename
        .iter()
        .take_while(|&&c| c != 0)
        .map(|&c| c as u8)
        .collect();
    positional_by_name(&String::from_utf8_lossy(&name)).unwrap_or(crate::VolumeIds::Stable)
}

#[cfg(target_os = "linux")]
fn volume_ids_of(st: &libc::statfs, dir: BorrowedFd) -> crate::VolumeIds {
    const MSDOS_SUPER_MAGIC: u64 = 0x4d44;
    const EXFAT_SUPER_MAGIC: u64 = 0x2011_bab0;
    const FUSE_SUPER_MAGIC: u64 = 0x6573_5546;
    #[allow(clippy::unnecessary_cast)]
    match st.f_type as u64 {
        MSDOS_SUPER_MAGIC => crate::VolumeIds::FAT,
        EXFAT_SUPER_MAGIC => crate::VolumeIds::EXFAT,
        FUSE_SUPER_MAGIC => fuse_type(dir)
            .as_deref()
            .and_then(fuse_positional)
            .unwrap_or(crate::VolumeIds::Stable),
        _ => crate::VolumeIds::Stable,
    }
}

/// The file system type `/proc/self/mountinfo` gives the mount `dir` is
/// on, found by the mount id `/proc/self/fdinfo` gives its descriptor.
#[cfg(target_os = "linux")]
fn fuse_type(dir: BorrowedFd) -> Option<String> {
    let fdinfo = std::fs::read_to_string(format!("/proc/self/fdinfo/{}", dir.as_raw_fd())).ok()?;
    let mount = fdinfo
        .lines()
        .find_map(|line| line.strip_prefix("mnt_id:"))?
        .trim()
        .to_owned();
    let mountinfo = std::fs::read_to_string("/proc/self/mountinfo").ok()?;
    mountinfo_type(&mountinfo, &mount).map(str::to_owned)
}

/// The file system type of mount `mount` in a mountinfo text: the field
/// after the ` - ` separator.
#[cfg_attr(not(any(test, target_os = "linux")), allow(dead_code))]
fn mountinfo_type<'a>(mountinfo: &'a str, mount: &str) -> Option<&'a str> {
    mountinfo.lines().find_map(|line| {
        (line.split(' ').next()? == mount)
            .then(|| line.split(" - ").nth(1)?.split(' ').next())
            .flatten()
    })
}

/// A FUSE type whose subtype is FAT or exFAT (`fuseblk.exfat`,
/// `fuse.fusefat`).
#[cfg_attr(not(any(test, target_os = "linux")), allow(dead_code))]
fn fuse_positional(fs_type: &str) -> Option<crate::VolumeIds> {
    let (_, subtype) = fs_type.split_once('.')?;
    positional_by_name(subtype)
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn volume_ids_of(_st: &libc::statfs, _dir: BorrowedFd) -> crate::VolumeIds {
    crate::VolumeIds::Stable
}

/// Whether a RENAME_EXCL the volume refused may be done as check, then
/// rename: only `ENOTSUP` on a volume whose ids are positions (FSKit's
/// msdos and exfat). Elsewhere (smbfs, nfs, webdav, macFUSE) no-replace
/// stays refused rather than weakened.
#[cfg_attr(not(any(test, target_os = "macos")), allow(dead_code))]
fn excl_fallback(errno: Option<i32>, ids: io::Result<crate::VolumeIds>) -> bool {
    errno == Some(libc::ENOTSUP) && matches!(ids, Ok(crate::VolumeIds::Positional { .. }))
}

/// A name is free only when stating it finds nothing; any other failure
/// is no answer and refuses.
#[cfg_attr(not(any(test, target_os = "macos")), allow(dead_code))]
fn name_free(stated: io::Result<EntryStat>) -> io::Result<()> {
    match stated {
        Ok(_) => Err(io::Error::from(io::ErrorKind::AlreadyExists)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

/// Deletes the file `name` from `dir` when `check`, shown the file as it
/// is once moved aside, holds. It is renamed to a private name in the same
/// folder first, which on FAT and exFAT keeps its size and times, so what
/// is checked is the object then removed, not whatever took its name; if
/// the check fails it goes back. Nothing is compared by id afterwards, so
/// it suits a volume whose ids are entry positions, where a moved entry
/// takes another id. A folder is refused.
pub fn delete_file_checked(
    dir: BorrowedFd,
    name: &str,
    check: impl FnOnce(&EntryStat) -> bool,
) -> io::Result<()> {
    use std::io::Read;
    let mut random = [0u8; 16];
    std::fs::File::open("/dev/urandom")?.read_exact(&mut random)?;
    let aside = format!(
        ".vnldelete.{}",
        random
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    );
    rename_at_noreplace(dir, name, &aside)?;
    let stated = stat_at(dir, &aside);
    let holds = stated
        .as_ref()
        .is_ok_and(|stated| stated.is_file && !stated.is_symlink && check(stated));
    if !holds {
        // A name made meanwhile keeps it; the entry stays aside, to be swept.
        rename_at_noreplace(dir, &aside, name)?;
        stated?;
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "delete source changed",
        ));
    }
    let c = cstr_name(&aside)?;
    // SAFETY: `unlinkat` takes a borrowed dir fd plus a borrowed name; no
    // AT_REMOVEDIR, so a folder is refused.
    if unsafe { libc::unlinkat(dir.as_raw_fd(), c.as_ptr(), 0) } == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
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
/// Unbounded: only for callers that need every name for completeness
/// (conflict choice, cleanup, deletion). Callers that page or budget use
/// [`list_names_bounded`].
pub fn list_names(dir: BorrowedFd) -> io::Result<Vec<OsString>> {
    Ok(list_names_inner(dir, usize::MAX)?.0)
}

/// Single bounded enumeration: collects at most `limit` names and reports
/// whether more entries exist, without allocating past the bound. The
/// stream is rewound first, so repeated scans of one handle each see the
/// whole directory (a bare `dup` would share the caller's offset and a
/// second scan would see nothing).
pub fn list_names_bounded(dir: BorrowedFd, limit: usize) -> io::Result<(Vec<OsString>, bool)> {
    list_names_inner(dir, limit)
}

fn list_names_inner(dir: BorrowedFd, limit: usize) -> io::Result<(Vec<OsString>, bool)> {
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
    // The dup shares the caller's offset: rewind so every scan starts at
    // the first entry even after an earlier scan exhausted the stream.
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    unsafe {
        libc::rewinddir(dirp)
    };
    let mut out = Vec::new();
    let mut truncated = false;
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
        if out.len() >= limit {
            truncated = true;
            // SAFETY: always close our DIR*.
            unsafe { libc::closedir(dirp) };
            break;
        }
        out.push(OsString::from_vec(name.to_vec()));
    }
    Ok((out, truncated))
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

/// [`fstat_dev_ino`] with the creation time where the platform reports one
/// through `fstat` (macOS), which tells a recycled inode apart.
pub fn fstat_identity(fd: BorrowedFd) -> io::Result<Identity> {
    let mut st: libc::stat = unsafe { std::mem::zeroed() };
    // SAFETY: `fstat` writes the borrowed stat struct for a live fd.
    if unsafe { libc::fstat(fd.as_raw_fd(), &mut st) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(Identity {
        dev: st.st_dev as u64,
        ino: st.st_ino as u64,
        born: born(&st),
    })
}

/// What names one filesystem object: its device and inode, and its creation
/// time in nanoseconds since 1970 where the filesystem keeps one. A freed
/// inode number is given to the next object made, at once on ext4 and
/// overlayfs; only the creation time tells the two apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Identity {
    pub dev: u64,
    pub ino: u64,
    pub born: Option<u64>,
}

/// Kind/size/mtime plus identity for one entry under `parent`, not
/// followed: `statx` on Linux, `fstatat` elsewhere.
pub struct EntryStat {
    pub is_symlink: bool,
    pub is_dir: bool,
    pub is_file: bool,
    pub size: u64,
    pub mtime_ms: u64,
    pub dev: u64,
    pub ino: u64,
    pub born: Option<u64>,
    /// Permission bits (`st_mode & 0o7777`).
    pub attributes: u32,
}

impl EntryStat {
    pub fn identity(&self) -> Identity {
        Identity {
            dev: self.dev,
            ino: self.ino,
            born: self.born,
        }
    }
}

pub fn stat_at(parent: BorrowedFd, name: &str) -> io::Result<EntryStat> {
    let c = cstr_name(name)?;
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    if let Some(stat) = statx_at(parent, &c) {
        return stat;
    }
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
        born: born(&st),
        attributes: u32::from(mode) & 0o7777,
    })
}

/// `statx` for one entry, with its birth time where the filesystem keeps
/// one; `None` where the kernel or a seccomp policy refuses the call, so
/// the caller falls back to `fstatat`.
#[cfg(all(target_os = "linux", target_env = "gnu"))]
fn statx_at(parent: BorrowedFd, name: &CString) -> Option<io::Result<EntryStat>> {
    let mut st: libc::statx = unsafe { std::mem::zeroed() };
    // SAFETY: `statx` takes a borrowed dir fd plus a borrowed name and
    // writes the struct it is given.
    let r = unsafe {
        libc::statx(
            parent.as_raw_fd(),
            name.as_ptr(),
            libc::AT_SYMLINK_NOFOLLOW,
            libc::STATX_BASIC_STATS | libc::STATX_BTIME,
            &mut st,
        )
    };
    if r != 0 {
        let error = io::Error::last_os_error();
        return match error.raw_os_error() {
            Some(libc::ENOSYS) | Some(libc::EPERM) => None,
            _ => Some(Err(error)),
        };
    }
    let mode = libc::mode_t::from(st.stx_mode);
    let nanos = |time: libc::statx_timestamp| {
        u64::try_from(time.tv_sec)
            .ok()
            .and_then(|seconds| seconds.checked_mul(1_000_000_000))
            .and_then(|nanos| nanos.checked_add(u64::from(time.tv_nsec)))
    };
    Some(Ok(EntryStat {
        is_symlink: (mode & libc::S_IFMT) == libc::S_IFLNK,
        is_dir: (mode & libc::S_IFMT) == libc::S_IFDIR,
        is_file: (mode & libc::S_IFMT) == libc::S_IFREG,
        size: st.stx_size,
        mtime_ms: nanos(st.stx_mtime).map_or(0, |nanos| nanos / 1_000_000),
        // As `st_dev` and `std` compose it.
        dev: libc::makedev(st.stx_dev_major, st.stx_dev_minor) as u64,
        ino: st.stx_ino,
        born: if st.stx_mask & libc::STATX_BTIME != 0 {
            nanos(st.stx_btime)
        } else {
            None
        },
        attributes: u32::from(mode) & 0o7777,
    }))
}

/// `fstatat` reports no birth time on Linux; `statx_at` does.
#[cfg(target_os = "linux")]
fn born(_st: &libc::stat) -> Option<u64> {
    None
}

#[cfg(target_os = "macos")]
fn born(st: &libc::stat) -> Option<u64> {
    u64::try_from(st.st_birthtime)
        .ok()
        .and_then(|seconds| seconds.checked_mul(1_000_000_000))
        .and_then(|nanos| nanos.checked_add(u64::try_from(st.st_birthtime_nsec).ok()?))
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn born(_st: &libc::stat) -> Option<u64> {
    None
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
            return Ok(());
        }
        let error = io::Error::last_os_error();
        if !excl_fallback(error.raw_os_error(), volume_ids(dir)) {
            return Err(error);
        }
        // FSKit's msdos and exfat refuse RENAME_EXCL. There the name is
        // checked free and then taken: a file made under it in between is
        // replaced. Nothing narrower exists on those volumes.
        name_free(stat_at(dir, new))?;
        // SAFETY: `renameat` takes borrowed dir fds plus borrowed names.
        let r = unsafe {
            libc::renameat(
                dir.as_raw_fd(),
                old_c.as_ptr(),
                dir.as_raw_fd(),
                new_c.as_ptr(),
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
/// Strictly a rename: across filesystems it returns the `EXDEV` error and
/// never copies. Callers pre-stage a [`SiblingStage`] on the destination
/// volume first (outside global locks), so the commit under locks stays a
/// fast same-filesystem rename.
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
                    Err(e)
                } else {
                    Err(e)
                }
            }
        }
        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
        {
            // Fallback: staged is outside the anchored walk; dest anchoring still
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
                    Err(e)
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
                    Err(e)
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

/// True when `staged` and the open destination dir live on the same
/// filesystem (`st_dev` equality). Publication pre-stages a temp sibling
/// on the destination volume first when they differ, so the commit under
/// global locks is always a fast same-filesystem rename.
pub fn same_filesystem(staged: &Path, dir: BorrowedFd) -> io::Result<bool> {
    use std::os::unix::io::AsRawFd;
    let staged_c = cstr(staged)?;
    let mut src: libc::stat = unsafe { std::mem::zeroed() };
    // SAFETY: `stat` takes a borrowed NUL-terminated path; `fstat` takes
    // our own open dir fd.
    if unsafe { libc::stat(staged_c.as_ptr(), &mut src) } != 0 {
        return Err(io::Error::last_os_error());
    }
    let mut dst: libc::stat = unsafe { std::mem::zeroed() };
    if unsafe { libc::fstat(dir.as_raw_fd(), &mut dst) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(src.st_dev == dst.st_dev)
}

/// A temp sibling inside the destination directory, created `O_EXCL` so a
/// concurrent publisher never truncates it. The caller streams staged
/// bytes in without holding global locks, fsyncs, then commits with a
/// same-filesystem rename. Dropping without commit unlinks the temp.
pub struct SiblingStage<'a> {
    dir: BorrowedFd<'a>,
    temp: CString,
    file: Option<std::fs::File>,
}

impl<'a> SiblingStage<'a> {
    pub fn create(dir: BorrowedFd<'a>) -> io::Result<Self> {
        use std::os::unix::io::{AsRawFd, FromRawFd};
        for attempt in 0..32u32 {
            let temp = cstr_name(&format!(".vnlpub.{attempt}.tmp"))?;
            // SAFETY: `openat` with borrowed dir fd + name; the fd is
            // converted to `File` exactly once on success.
            let fd = unsafe {
                libc::openat(
                    dir.as_raw_fd(),
                    temp.as_ptr(),
                    libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_CLOEXEC,
                    0o644,
                )
            };
            if fd < 0 {
                let error = io::Error::last_os_error();
                if error.kind() == io::ErrorKind::AlreadyExists {
                    continue;
                }
                return Err(error);
            }
            let file = unsafe { std::fs::File::from_raw_fd(fd) };
            return Ok(Self {
                dir,
                temp,
                file: Some(file),
            });
        }
        Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "no free temp name",
        ))
    }

    pub fn write_all(&mut self, buf: &[u8]) -> io::Result<()> {
        use std::io::Write;
        self.file
            .as_mut()
            .ok_or_else(|| io::Error::new(io::ErrorKind::BrokenPipe, "stage committed"))?
            .write_all(buf)
    }

    pub fn fsync(&self) -> io::Result<()> {
        self.file
            .as_ref()
            .ok_or_else(|| io::Error::new(io::ErrorKind::BrokenPipe, "stage committed"))?
            .sync_all()
    }

    /// Atomically renames the staged temp onto `dest_name` with
    /// replace/no-replace semantics. Same filesystem by construction, so
    /// this never copies. Consumes the stage without unlinking.
    /// Durability-syncs the staged contents first; when the caller
    /// already synced before taking an authority gate, use
    /// [`commit_prepared`](Self::commit_prepared) so no slow I/O runs
    /// under locks.
    pub fn commit(self, dest_name: &str, replace: bool) -> io::Result<()> {
        self.commit_inner(dest_name, replace, true)
    }

    /// Rename-only commit for a stage whose contents the caller already
    /// made durable (write + `fsync`) before taking its authority gate.
    /// Performs no I/O besides the atomic rename, so it is safe under
    /// short-held locks. Consumes the stage without unlinking.
    pub fn commit_prepared(self, dest_name: &str, replace: bool) -> io::Result<()> {
        self.commit_inner(dest_name, replace, false)
    }

    fn commit_inner(mut self, dest_name: &str, replace: bool, sync_first: bool) -> io::Result<()> {
        use std::os::unix::io::AsRawFd;
        let dest_c = cstr_name(dest_name)?;
        let dir_fd = self.dir.as_raw_fd();
        let temp = self.temp.clone();
        let file = self
            .file
            .take()
            .ok_or_else(|| io::Error::new(io::ErrorKind::BrokenPipe, "stage committed"))?;
        // `file` is taken, so every failure below unlinks explicitly;
        // Drop treats the stage as settled once `file` is gone.
        if sync_first {
            if let Err(error) = file.sync_all() {
                drop(file);
                unsafe { libc::unlinkat(dir_fd, temp.as_ptr(), 0) };
                return Err(error);
            }
        }
        drop(file);
        #[cfg(target_os = "linux")]
        let result = if replace {
            // SAFETY: `renameat` within one dir fd (overwrite allowed).
            let r = unsafe { libc::renameat(dir_fd, temp.as_ptr(), dir_fd, dest_c.as_ptr()) };
            if r == 0 {
                Ok(())
            } else {
                Err(io::Error::last_os_error())
            }
        } else {
            // SAFETY: no-replace commit decides atomically.
            let r = unsafe {
                libc::renameat2(
                    dir_fd,
                    temp.as_ptr(),
                    dir_fd,
                    dest_c.as_ptr(),
                    libc::RENAME_NOREPLACE,
                )
            };
            if r == 0 {
                Ok(())
            } else {
                Err(io::Error::last_os_error())
            }
        };
        #[cfg(target_os = "macos")]
        let result = if replace {
            // SAFETY: `renameat` within one dir (overwrite allowed).
            let r = unsafe { libc::renameat(dir_fd, temp.as_ptr(), dir_fd, dest_c.as_ptr()) };
            if r == 0 {
                Ok(())
            } else {
                Err(io::Error::last_os_error())
            }
        } else {
            // SAFETY: `renameatx_np` with `RENAME_EXCL` decides atomically.
            let r = unsafe {
                libc::renameatx_np(
                    dir_fd,
                    temp.as_ptr(),
                    dir_fd,
                    dest_c.as_ptr(),
                    libc::RENAME_EXCL,
                )
            };
            if r == 0 {
                Ok(())
            } else {
                Err(io::Error::last_os_error())
            }
        };
        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
        let result = {
            let _ = (dir_fd, &temp, &dest_c, replace);
            Err::<(), io::Error>(io::Error::new(
                io::ErrorKind::Unsupported,
                "anchored publish needs a native primitive",
            ))
        };
        if result.is_err() {
            // Unlink the orphan here: `file` is already taken, so Drop
            // treats the stage as settled.
            unsafe { libc::unlinkat(dir_fd, temp.as_ptr(), 0) };
        }
        result
    }
}

impl Drop for SiblingStage<'_> {
    fn drop(&mut self) {
        use std::os::unix::io::AsRawFd;
        if self.file.is_none() {
            return;
        }
        // SAFETY: `unlinkat` with borrowed dir fd + name; best effort.
        unsafe { libc::unlinkat(self.dir.as_raw_fd(), self.temp.as_ptr(), 0) };
    }
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

/// Removes one managed subtree through directory handles. Symlinks are
/// unlinked as entries, never traversed. Work is bounded to 64 levels and
/// 10,000 entries; failure leaves remaining entries for a later cleanup.
pub fn remove_tree_at(parent: BorrowedFd, name: &str) -> io::Result<()> {
    if name == "." || name == ".." {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "managed subtree must be a child",
        ));
    }
    let mut remaining = 10_000usize;
    remove_tree_inner(parent, name, 0, &mut remaining, None)
}
fn remove_tree_inner(
    parent: BorrowedFd,
    name: &str,
    depth: usize,
    remaining: &mut usize,
    expected: Option<Identity>,
) -> io::Result<()> {
    if depth > 64 || *remaining == 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "managed subtree exceeds cleanup bounds",
        ));
    }
    *remaining -= 1;
    let before = stat_at(parent, name)?;
    if expected.is_some_and(|identity| identity != before.identity()) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "managed subtree changed",
        ));
    }
    if before.is_dir && !before.is_symlink {
        if before.dev != fstat_dev_ino(parent)?.0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "managed subtree crosses a volume",
            ));
        }
        use std::os::fd::AsFd;
        let child = open_child(parent, name)?;
        if fstat_dev_ino(child.as_fd())? != (before.dev, before.ino) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "managed subtree changed",
            ));
        }
        for entry in list_names(child.as_fd())? {
            let entry = entry.to_str().ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidData, "managed entry encoding")
            })?;
            remove_tree_inner(child.as_fd(), entry, depth + 1, remaining, None)?;
        }
        let current = stat_at(parent, name)?;
        if (current.dev, current.ino) != (before.dev, before.ino)
            || !current.is_dir
            || current.is_symlink
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "managed subtree changed",
            ));
        }
    }
    let name = cstr_name(name)?;
    // SAFETY: the borrowed parent FD and checked component live through the
    // call. unlinkat removes an entry relative to that FD; it never traverses
    // the final symlink. Directory removal is only attempted after anchored
    // traversal and object identity checks.
    let result = unsafe {
        libc::unlinkat(
            parent.as_raw_fd(),
            name.as_ptr(),
            if before.is_dir && !before.is_symlink {
                libc::AT_REMOVEDIR
            } else {
                0
            },
        )
    };
    if result == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

#[cfg(test)]
mod stage_commit_tests {
    use super::*;
    use std::os::unix::io::AsFd;

    fn scratch(tag: &str) -> std::path::PathBuf {
        let root =
            std::env::temp_dir().join(format!("vnilink-stage-{}-{}", std::process::id(), tag));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    /// A caller-synced stage lands identical bytes through the rename-only
    /// commit, and no-replace still refuses an existing destination.
    #[test]
    fn commit_prepared_lands_synced_bytes_and_keeps_no_replace_atomic() {
        let root = scratch("prepared");
        let dir = std::fs::File::open(&root).unwrap();
        let mut stage = SiblingStage::create(dir.as_fd()).unwrap();
        stage.write_all(b"prepared bytes").unwrap();
        stage.fsync().unwrap();
        stage.commit_prepared("landed.txt", false).unwrap();
        assert_eq!(
            std::fs::read(root.join("landed.txt")).unwrap(),
            b"prepared bytes"
        );
        let mut stage = SiblingStage::create(dir.as_fd()).unwrap();
        stage.write_all(b"replacement").unwrap();
        stage.fsync().unwrap();
        let conflict = stage.commit_prepared("landed.txt", false).unwrap_err();
        assert_eq!(conflict.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(
            std::fs::read(root.join("landed.txt")).unwrap(),
            b"prepared bytes"
        );
        std::fs::remove_dir_all(&root).unwrap();
    }
}

#[cfg(test)]
mod bounded_list_tests {
    use super::*;
    use std::os::unix::io::AsFd;

    fn scratch(tag: &str, count: usize) -> std::path::PathBuf {
        let root =
            std::env::temp_dir().join(format!("vnilink-list-{}-{}", std::process::id(), tag));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        for i in 0..count {
            std::fs::write(root.join(format!("entry-{i:04}")), []).unwrap();
        }
        root
    }

    /// A bounded scan stops at the limit and reports overflow from the
    /// same pass; an exact fit reports no truncation.
    #[test]
    fn bounded_scan_reports_overflow_without_over_allocating() {
        let root = scratch("bounded", 10);
        let dir = std::fs::File::open(&root).unwrap();
        let (names, truncated) = list_names_bounded(dir.as_fd(), 4).unwrap();
        assert_eq!(names.len(), 4);
        assert!(truncated);
        let (names, truncated) = list_names_bounded(dir.as_fd(), 10).unwrap();
        assert_eq!(names.len(), 10);
        assert!(!truncated);
        let (names, truncated) = list_names_bounded(dir.as_fd(), 11).unwrap();
        assert_eq!(names.len(), 10);
        assert!(!truncated);
        std::fs::remove_dir_all(&root).unwrap();
    }

    /// Repeated scans of one handle each see the whole directory: the
    /// dup must not inherit an exhausted offset from an earlier scan.
    #[test]
    fn repeated_scans_each_see_the_whole_directory() {
        let root = scratch("rewind", 6);
        let dir = std::fs::File::open(&root).unwrap();
        let first = list_names(dir.as_fd()).unwrap();
        let second = list_names(dir.as_fd()).unwrap();
        assert_eq!(first.len(), 6);
        assert_eq!(second.len(), 6);
        assert_eq!(first, second);
        std::fs::remove_dir_all(&root).unwrap();
    }
}

/// A selected object isolated from public filename churn before destructive work.
/// A failed or abandoned cleanup leaves its private recovery namespace intact.
pub struct DeleteCandidate {
    parent: OwnedFd,
    private: OwnedFd,
    name: String,
    expected: Identity,
}
impl DeleteCandidate {
    pub fn stage(parent: BorrowedFd<'_>, name: &str, expected: Identity) -> io::Result<Self> {
        use std::{io::Read, os::fd::AsFd};
        let original = cstr_name(name)?;
        let parent = parent.try_clone_to_owned()?;
        let mut random = [0u8; 16];
        std::fs::File::open("/dev/urandom")?.read_exact(&mut random)?;
        let private_name = format!(
            ".vnldelete.{}",
            random
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>()
        );
        let private_c = cstr_name(&private_name)?;
        // SAFETY: the owned parent fd and terminated name remain live. Only
        // this owner can access entries in the new private namespace.
        if unsafe { libc::mkdirat(parent.as_raw_fd(), private_c.as_ptr(), 0o700) } != 0 {
            return Err(io::Error::last_os_error());
        }
        let private = match open_child(parent.as_fd(), &private_name) {
            Ok(private) => private,
            Err(error) => {
                // SAFETY: remove only the exclusively created, empty directory.
                unsafe {
                    libc::unlinkat(parent.as_raw_fd(), private_c.as_ptr(), libc::AT_REMOVEDIR);
                }
                return Err(error);
            }
        };
        let candidate = Self {
            parent,
            private,
            name: private_name,
            expected,
        };
        let child = cstr_name("candidate")?;
        crate::recording_retention::rename(
            candidate.parent.as_raw_fd(),
            &original,
            candidate.private.as_raw_fd(),
            &child,
        )?;
        let actual = stat_at(candidate.private.as_fd(), "candidate")?;
        if actual.is_symlink || actual.identity() != expected {
            // No replacement is destroyed. Restore without overwriting another
            // public entry; a collision leaves the object recoverable inside.
            crate::recording_retention::rename(
                candidate.private.as_raw_fd(),
                &child,
                candidate.parent.as_raw_fd(),
                &original,
            )?;
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "delete source changed",
            ));
        }
        Ok(candidate)
    }
    pub fn finish(self) -> io::Result<()> {
        use std::os::fd::AsFd;
        let mut remaining = 10_000usize;
        remove_tree_inner(
            self.private.as_fd(),
            "candidate",
            0,
            &mut remaining,
            Some(self.expected),
        )
    }
}
impl Drop for DeleteCandidate {
    fn drop(&mut self) {
        use std::os::fd::AsFd;
        let owned = fstat_dev_ino(self.private.as_fd());
        let current = stat_at(self.parent.as_fd(), &self.name);
        if matches!((owned, current), (Ok(identity), Ok(current)) if identity == (current.dev, current.ino))
        {
            if let Ok(name) = cstr_name(&self.name) {
                // SAFETY: best-effort empty-directory removal, never recursion.
                unsafe {
                    libc::unlinkat(self.parent.as_raw_fd(), name.as_ptr(), libc::AT_REMOVEDIR);
                }
            }
        }
    }
}

#[cfg(test)]
mod delete_candidate_tests {
    use super::*;
    use std::os::fd::AsFd;
    fn scratch() -> std::path::PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "vnilink-delete-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&path).unwrap();
        path
    }
    #[test]
    fn replacing_the_public_staging_directory_never_redirects_cleanup() {
        let path = scratch();
        std::fs::create_dir(path.join("selected")).unwrap();
        std::fs::write(path.join("selected/owned"), b"selected").unwrap();
        let parent = open_root(&path).unwrap();
        let before = stat_at(parent.as_fd(), "selected").unwrap();
        let candidate =
            DeleteCandidate::stage(parent.as_fd(), "selected", before.identity()).unwrap();
        let namespace = std::fs::read_dir(&path)
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        std::fs::rename(&namespace, path.join("saved-private")).unwrap();
        std::fs::create_dir(&namespace).unwrap();
        std::fs::write(namespace.join("sentinel"), b"unrelated").unwrap();
        candidate.finish().unwrap();
        assert_eq!(
            std::fs::read(namespace.join("sentinel")).unwrap(),
            b"unrelated"
        );
        assert_eq!(
            std::fs::read_dir(path.join("saved-private"))
                .unwrap()
                .count(),
            0
        );
        std::fs::remove_dir_all(path).unwrap();
    }
    #[test]
    fn replacing_a_listed_file_with_a_folder_restores_the_replacement() {
        let path = scratch();
        std::fs::write(path.join("selected"), b"original").unwrap();
        let parent = open_root(&path).unwrap();
        let before = stat_at(parent.as_fd(), "selected").unwrap();
        std::fs::rename(path.join("selected"), path.join("original")).unwrap();
        std::fs::create_dir(path.join("selected")).unwrap();
        std::fs::write(path.join("selected/sentinel"), b"replacement").unwrap();
        assert!(DeleteCandidate::stage(parent.as_fd(), "selected", before.identity()).is_err());
        assert_eq!(
            std::fs::read(path.join("selected/sentinel")).unwrap(),
            b"replacement"
        );
        assert_eq!(std::fs::read(path.join("original")).unwrap(), b"original");
        assert_eq!(std::fs::read_dir(&path).unwrap().count(), 2);
        std::fs::remove_dir_all(path).unwrap();
    }
}

/// Whether the system's file manager shows the volume mounted at `path`.
/// macOS mounts system volumes such as Recovery with `MNT_DONTBROWSE`, and
/// Finder leaves them out; elsewhere every mounted volume is shown.
pub fn browsable_volume(path: &Path) -> bool {
    #[cfg(target_os = "macos")]
    {
        let Ok(path) = CString::new(path.as_os_str().as_bytes()) else {
            return false;
        };
        let mut stat = std::mem::MaybeUninit::<libc::statfs>::uninit();
        // SAFETY: `statfs` reads a borrowed NUL-terminated path and fills
        // the buffer it is given, which is only read after it succeeds.
        if unsafe { libc::statfs(path.as_ptr(), stat.as_mut_ptr()) } != 0 {
            return false;
        }
        // SAFETY: `statfs` returned 0, so it initialized the whole struct.
        let stat = unsafe { stat.assume_init() };
        stat.f_flags & (libc::MNT_DONTBROWSE as u32) == 0
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = path;
        true
    }
}

#[cfg(test)]
mod volume_tests {
    use super::*;
    use crate::VolumeIds;

    #[test]
    fn fat_and_exfat_are_named_so_by_any_driver() {
        for (name, ids) in [
            ("msdos", Some(VolumeIds::FAT)),
            ("exfat", Some(VolumeIds::EXFAT)),
            ("ufsd_ExFAT", Some(VolumeIds::EXFAT)),
            ("ufsd_FAT32", Some(VolumeIds::FAT)),
            ("vfat", Some(VolumeIds::FAT)),
            ("apfs", None),
            ("smbfs", None),
            ("macfuse", None),
        ] {
            assert_eq!(positional_by_name(name), ids, "{name}");
        }
    }

    #[test]
    fn a_fuse_mount_is_named_by_its_subtype() {
        let mountinfo = "\
36 35 98:0 / /mnt rw,noatime master:1 - ext4 /dev/root rw
41 36 8:17 / /media/stick rw,nosuid shared:2 - fuseblk.exfat /dev/sdb1 rw,user_id=0
42 36 0:40 / /media/fat rw shared:3 - fuse.fusefat /dev/sdc1 rw
43 36 8:33 / /media/ntfs rw shared:4 - fuseblk /dev/sdd1 rw";
        assert_eq!(mountinfo_type(mountinfo, "41"), Some("fuseblk.exfat"));
        assert_eq!(mountinfo_type(mountinfo, "99"), None);
        assert_eq!(fuse_positional("fuseblk.exfat"), Some(VolumeIds::EXFAT));
        assert_eq!(
            fuse_positional(mountinfo_type(mountinfo, "42").unwrap()),
            Some(VolumeIds::FAT)
        );
        assert_eq!(fuse_positional("fuseblk"), None, "ntfs-3g names no subtype");
        assert_eq!(fuse_positional("fuse.sshfs"), None);
    }

    #[test]
    fn no_replace_is_weakened_only_on_a_positional_volume_that_refuses_it() {
        assert!(excl_fallback(Some(libc::ENOTSUP), Ok(VolumeIds::EXFAT)));
        assert!(!excl_fallback(Some(libc::ENOTSUP), Ok(VolumeIds::Stable)));
        assert!(!excl_fallback(
            Some(libc::ENOTSUP),
            Err(io::Error::from(io::ErrorKind::PermissionDenied))
        ));
        assert!(!excl_fallback(Some(libc::EEXIST), Ok(VolumeIds::FAT)));
    }

    #[test]
    fn a_name_is_free_only_when_nothing_is_found() {
        assert!(name_free(Err(io::Error::from(io::ErrorKind::NotFound))).is_ok());
        assert_eq!(
            name_free(Err(io::Error::from(io::ErrorKind::PermissionDenied)))
                .unwrap_err()
                .kind(),
            io::ErrorKind::PermissionDenied
        );
        let stated = EntryStat {
            is_symlink: false,
            is_dir: false,
            is_file: true,
            size: 0,
            mtime_ms: 0,
            dev: 0,
            ino: 0,
            born: None,
            attributes: 0,
        };
        assert_eq!(
            name_free(Ok(stated)).unwrap_err().kind(),
            io::ErrorKind::AlreadyExists
        );
    }

    #[test]
    fn a_checked_delete_removes_what_it_checked_and_restores_what_fails() {
        use std::os::fd::AsFd;
        let folder = std::env::temp_dir().join(format!(
            "vnilink-checked-delete-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&folder).unwrap();
        std::fs::write(folder.join("kept.txt"), b"kept").unwrap();
        std::fs::write(folder.join("gone.txt"), b"gone").unwrap();
        std::fs::create_dir(folder.join("folder")).unwrap();
        let dir = open_root(&folder).unwrap();
        let refused = delete_file_checked(dir.as_fd(), "kept.txt", |_| false);
        assert_eq!(refused.unwrap_err().kind(), io::ErrorKind::InvalidData);
        assert_eq!(std::fs::read(folder.join("kept.txt")).unwrap(), b"kept");
        assert!(delete_file_checked(dir.as_fd(), "folder", |_| true).is_err());
        assert!(folder.join("folder").is_dir(), "a folder is refused");
        delete_file_checked(dir.as_fd(), "gone.txt", |stated| stated.size == 4).unwrap();
        let mut left: Vec<_> = std::fs::read_dir(&folder)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect();
        left.sort();
        assert_eq!(left, ["folder", "kept.txt"], "nothing is left aside");
        std::fs::remove_dir_all(&folder).unwrap();
    }
}
