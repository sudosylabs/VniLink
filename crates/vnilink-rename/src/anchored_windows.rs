//! Handle-relative operations under a shared root on Windows.
//!
//! The Windows side of [`anchored`](self): the same functions, meaning the
//! same things, so callers in `vnilink` walk a shared drive identically on
//! every platform. Each child is opened relative to its parent's directory
//! handle (`NtCreateFile` with `OBJECT_ATTRIBUTES.RootDirectory`) and with
//! `FILE_OPEN_REPARSE_POINT`, so a symbolic link, junction or mount point is
//! opened as itself and refused instead of followed, as Unix refuses a
//! symlink with `O_NOFOLLOW`. Renames and deletes act on the handle of the
//! object that was opened and checked, never on a path resolved again.
//!
//! A link is a reparse point whose tag is a name surrogate: it names another
//! object. A folder is walked only when it is no reparse point or a
//! cloud-file placeholder (`windows::walkable_directory`); every other
//! reparse folder is refused. A file that is any reparse point is listed
//! but never read here, since only its filter driver can supply its bytes.
//!
//! Handles a mutation holds (`*_held`) do not share delete access, so while
//! an operation runs no folder on its chain can be renamed or deleted, and
//! none can be moved out of the root under it. Identity is
//! `windows::object_identity`: volume serial number and file id.
//!
//! A name reaches the filesystem only when it is one entry of its directory
//! as Windows reads it: no separators, no `:` (alternate data streams), no
//! wildcard or control characters, no trailing dot or space (which Windows
//! strips), no device name such as `CON` or `COM1`, and no 8.3 short name
//! standing for an entry whose long name differs. Where a volume cannot
//! report an entry's long name (`FileNormalizedNameInformation`), every name
//! containing `~` is refused rather than risk an alias.

use std::ffi::OsString;
use std::io;
use std::os::windows::ffi::OsStringExt;
use std::os::windows::fs::OpenOptionsExt;
use std::os::windows::io::{AsHandle, AsRawHandle, BorrowedHandle, FromRawHandle, OwnedHandle};
use std::path::Path;

use windows_sys::Wdk::Foundation::OBJECT_ATTRIBUTES;
use windows_sys::Wdk::Storage::FileSystem::{
    FileDispositionInformation, FileDispositionInformationEx, FileFsFullSizeInformation,
    FileNamesInformation, FileNormalizedNameInformation, FileRenameInformation, NtCreateFile,
    NtQueryDirectoryFile, NtQueryInformationFile, NtQueryVolumeInformationFile,
    NtSetInformationFile, FILE_CREATE, FILE_DIRECTORY_FILE, FILE_DISPOSITION_DELETE,
    FILE_DISPOSITION_IGNORE_READONLY_ATTRIBUTE, FILE_DISPOSITION_INFORMATION,
    FILE_DISPOSITION_INFORMATION_EX, FILE_DISPOSITION_POSIX_SEMANTICS, FILE_NAMES_INFORMATION,
    FILE_NON_DIRECTORY_FILE, FILE_OPEN, FILE_OPEN_REPARSE_POINT, FILE_RENAME_INFORMATION,
    FILE_RENAME_INFORMATION_0, FILE_SYNCHRONOUS_IO_NONALERT,
};
use windows_sys::Win32::Foundation::{
    RtlNtStatusToDosError, FILETIME, HANDLE, NTSTATUS, OBJ_CASE_INSENSITIVE,
    STATUS_INVALID_INFO_CLASS, STATUS_INVALID_PARAMETER, STATUS_NOT_IMPLEMENTED,
    STATUS_NOT_SUPPORTED, STATUS_NO_MORE_FILES, UNICODE_STRING,
};
use windows_sys::Win32::Storage::FileSystem::{
    GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION, DELETE, FILE_ADD_FILE,
    FILE_ADD_SUBDIRECTORY, FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_NORMAL,
    FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
    FILE_GENERIC_READ, FILE_GENERIC_WRITE, FILE_LIST_DIRECTORY, FILE_READ_ATTRIBUTES,
    FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, FILE_TRAVERSE, SYNCHRONIZE,
};
use windows_sys::Win32::System::IO::IO_STATUS_BLOCK;

/// An open directory: the handle every child operation is relative to.
pub type Dir = OwnedHandle;
/// A borrowed [`Dir`].
pub type DirRef<'a> = BorrowedHandle<'a>;

/// Borrows a directory handle as [`DirRef`], whatever the platform's handle.
pub trait AsDir {
    fn as_dir(&self) -> DirRef<'_>;
}

impl<T: AsHandle> AsDir for T {
    fn as_dir(&self) -> DirRef<'_> {
        self.as_handle()
    }
}

/// Every sharing mode: an anchored handle never keeps others from reading,
/// writing, renaming or deleting, as a Unix descriptor never does.
const SHARE_ALL: u32 = FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE;
/// What a held handle shares: reading and writing, never renaming or
/// deleting the object while it is held.
const SHARE_HELD: u32 = FILE_SHARE_READ | FILE_SHARE_WRITE;
const DIRECTORY_ACCESS: u32 =
    FILE_LIST_DIRECTORY | FILE_TRAVERSE | FILE_READ_ATTRIBUTES | SYNCHRONIZE;
const ATTRIBUTES_ACCESS: u32 = FILE_READ_ATTRIBUTES | SYNCHRONIZE;
const DELETE_ACCESS: u32 = DELETE | FILE_READ_ATTRIBUTES | SYNCHRONIZE;
/// FILETIME counts 100 ns intervals since 1601-01-01.
const FILETIME_UNIX_EPOCH: u64 = 116_444_736_000_000_000;

/// Device names Windows resolves in any folder, with or without an
/// extension. Superscript digits are the forms Windows also reserves.
const DEVICE_NAMES: [&str; 6] = ["CON", "PRN", "AUX", "NUL", "CONIN$", "CONOUT$"];
const NUMBERED_DEVICES: [&str; 2] = ["COM", "LPT"];

fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message)
}

/// The name as one entry of its directory, in UTF-16, or a refusal for any
/// name Windows would read as something else.
fn component(name: &str) -> io::Result<Vec<u16>> {
    if name.is_empty() || name == "." || name == ".." {
        return Err(invalid("name is not a single component"));
    }
    if name
        .chars()
        .any(|c| c < ' ' || matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*'))
    {
        return Err(invalid("name is not a single component"));
    }
    // Windows strips a trailing dot or space when it resolves a path, so
    // such a name stands for a different entry than it spells.
    if name.ends_with('.') || name.ends_with(' ') {
        return Err(invalid("name ends with a dot or a space"));
    }
    if is_device_name(name) {
        return Err(invalid("name is a device Windows reserves"));
    }
    let wide: Vec<u16> = name.encode_utf16().collect();
    if wide.len() * 2 > usize::from(u16::MAX) {
        return Err(invalid("name is too long"));
    }
    Ok(wide)
}

/// Whether Windows resolves `name` to a device: its part before the first
/// dot, with trailing spaces removed, is a device name in any case.
fn is_device_name(name: &str) -> bool {
    let stem = name.split('.').next().unwrap_or(name).trim_end_matches(' ');
    if DEVICE_NAMES
        .iter()
        .any(|device| stem.eq_ignore_ascii_case(device))
    {
        return true;
    }
    let mut chars = stem.chars();
    let prefix: String = chars.by_ref().take(3).collect();
    let rest: Vec<char> = chars.collect();
    NUMBERED_DEVICES
        .iter()
        .any(|device| prefix.eq_ignore_ascii_case(device))
        && matches!(rest.as_slice(), [digit] if digit.is_ascii_digit() || matches!(digit, '¹' | '²' | '³'))
}

fn nt_result(status: NTSTATUS) -> io::Result<()> {
    if status >= 0 {
        return Ok(());
    }
    // SAFETY: RtlNtStatusToDosError only maps a status code; it takes no
    // pointers and touches no memory of ours.
    let code = unsafe { RtlNtStatusToDosError(status) };
    Err(io::Error::from_raw_os_error(code as i32))
}

/// `NtCreateFile` of one child `name` relative to `parent`. The final
/// component is never followed: `FILE_OPEN_REPARSE_POINT` opens a reparse
/// point as itself. Opens are synchronous, as `std::fs::File` expects.
fn create_at(
    parent: DirRef<'_>,
    name: &[u16],
    access: u32,
    share: u32,
    disposition: u32,
    options: u32,
) -> io::Result<OwnedHandle> {
    let bytes = (name.len() * 2) as u16;
    let object_name = UNICODE_STRING {
        Length: bytes,
        MaximumLength: bytes,
        Buffer: name.as_ptr().cast_mut(),
    };
    let attributes = OBJECT_ATTRIBUTES {
        Length: std::mem::size_of::<OBJECT_ATTRIBUTES>() as u32,
        RootDirectory: parent.as_raw_handle(),
        ObjectName: &object_name,
        Attributes: OBJ_CASE_INSENSITIVE,
        SecurityDescriptor: std::ptr::null(),
        SecurityQualityOfService: std::ptr::null(),
    };
    let mut handle: HANDLE = std::ptr::null_mut();
    let mut status_block = IO_STATUS_BLOCK::default();
    // SAFETY: `object_name` points into `name`, which outlives the call, and
    // its lengths are the slice's byte length; `attributes` points at it and
    // at the borrowed parent handle, both live for the call. The output
    // handle and status block are writable locals. No EA buffer is passed.
    let status = unsafe {
        NtCreateFile(
            &mut handle,
            access,
            &attributes,
            &mut status_block,
            std::ptr::null(),
            FILE_ATTRIBUTE_NORMAL,
            share,
            disposition,
            options | FILE_OPEN_REPARSE_POINT | FILE_SYNCHRONOUS_IO_NONALERT,
            std::ptr::null(),
            0,
        )
    };
    nt_result(status)?;
    // SAFETY: NtCreateFile succeeded, so `handle` is a fresh handle that
    // nothing else owns; OwnedHandle closes it exactly once.
    Ok(unsafe { OwnedHandle::from_raw_handle(handle) })
}

/// Opens an existing child `name` under `parent` without following it, and
/// refuses an 8.3 short name that stands for a different long name.
fn open_existing(
    parent: DirRef<'_>,
    name: &str,
    access: u32,
    share: u32,
    options: u32,
) -> io::Result<OwnedHandle> {
    let wide = component(name)?;
    let handle = create_at(parent, &wide, access, share, FILE_OPEN, options)?;
    refuse_short_alias(handle.as_handle(), name)?;
    Ok(handle)
}

/// A name with `~` may be the 8.3 short alias of an entry whose real name
/// differs. Such a name passes no rule a long name passes (the engine's own
/// temporary names, case checks), so it is refused: the opened object's long
/// name must be the name asked for.
fn refuse_short_alias(handle: BorrowedHandle<'_>, name: &str) -> io::Result<()> {
    if !name.contains('~') {
        return Ok(());
    }
    // FILE_NAME_INFORMATION: a byte length, then the path from the volume root.
    let mut buffer = vec![0u64; (4 + 65_536) / 8 + 1];
    let length = (buffer.len() * 8) as u32;
    let mut status_block = IO_STATUS_BLOCK::default();
    // SAFETY: the borrowed handle is live; the buffer is writable, 8-byte
    // aligned and `length` bytes long; the status block is a writable local.
    let status = unsafe {
        NtQueryInformationFile(
            handle.as_raw_handle(),
            &mut status_block,
            buffer.as_mut_ptr().cast(),
            length,
            FileNormalizedNameInformation,
        )
    };
    nt_result(status).map_err(|_| invalid("name may be a short alias"))?;
    let bytes = buffer[0] as u32 as usize;
    let available = buffer.len() * 8 - 4;
    if bytes > available || !bytes.is_multiple_of(2) {
        return Err(invalid("name may be a short alias"));
    }
    // SAFETY: the name starts 4 bytes in (2-byte aligned) and its byte length
    // was checked against the buffer, which stays alive and unchanged.
    let path = unsafe {
        std::slice::from_raw_parts(buffer.as_ptr().cast::<u8>().add(4).cast::<u16>(), bytes / 2)
    };
    let last = path
        .rsplit(|&unit| unit == u16::from(b'\\'))
        .next()
        .unwrap_or(path);
    if String::from_utf16_lossy(last).to_lowercase() != name.to_lowercase() {
        return Err(invalid("name is a short alias of another entry"));
    }
    Ok(())
}

/// A rename or create target with `~` must not be the short alias of an
/// existing entry with another long name: replacing through it would replace
/// an entry under a name nobody chose.
fn refuse_target_alias(dir: DirRef<'_>, name: &str) -> io::Result<()> {
    if !name.contains('~') {
        return Ok(());
    }
    match open_existing(dir, name, ATTRIBUTES_ACCESS, SHARE_ALL, 0) {
        Err(error) if error.kind() == io::ErrorKind::InvalidInput => Err(error),
        _ => Ok(()),
    }
}

/// What one handle's object is.
struct Info {
    attributes: u32,
    tag: u32,
    volume: u64,
    index: u64,
    size: u64,
    written: u64,
    created: u64,
}

fn filetime(time: FILETIME) -> u64 {
    (u64::from(time.dwHighDateTime) << 32) | u64::from(time.dwLowDateTime)
}

fn info(handle: BorrowedHandle<'_>) -> io::Result<Info> {
    let mut by_handle = BY_HANDLE_FILE_INFORMATION::default();
    // SAFETY: the borrowed handle is live and the output is a correctly laid
    // out, writable BY_HANDLE_FILE_INFORMATION.
    if unsafe { GetFileInformationByHandle(handle.as_raw_handle(), &mut by_handle) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let tag = if by_handle.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        crate::windows::reparse_tag(handle)?
    } else {
        0
    };
    // A volume reporting no usable `FileIdInfo` (some SMB servers) still
    // gives the older index. Its ids prove nothing, so its entries are
    // listed and changed by fingerprint (`windows::volume_ids`).
    let (volume, index) = crate::windows::object_identity(handle).unwrap_or((
        u64::from(by_handle.dwVolumeSerialNumber),
        (u64::from(by_handle.nFileIndexHigh) << 32) | u64::from(by_handle.nFileIndexLow),
    ));
    Ok(Info {
        attributes: by_handle.dwFileAttributes,
        tag,
        volume,
        index,
        size: (u64::from(by_handle.nFileSizeHigh) << 32) | u64::from(by_handle.nFileSizeLow),
        written: filetime(by_handle.ftLastWriteTime),
        created: filetime(by_handle.ftCreationTime),
    })
}

impl Info {
    fn is_reparse(&self) -> bool {
        self.attributes & FILE_ATTRIBUTE_REPARSE_POINT != 0
    }
    /// A reparse point that names another object: a link, as Unix reports
    /// a symlink.
    fn is_link(&self) -> bool {
        self.is_reparse() && crate::windows::is_link_tag(self.tag)
    }
    /// A directory that is not a link. Only [`walkable`](Self::walkable)
    /// ones are entered.
    fn is_dir(&self) -> bool {
        !self.is_link() && self.attributes & FILE_ATTRIBUTE_DIRECTORY != 0
    }
    fn walkable(&self) -> bool {
        crate::windows::walkable_directory(self.attributes, self.tag)
    }
    fn is_file(&self) -> bool {
        !self.is_link() && self.attributes & FILE_ATTRIBUTE_DIRECTORY == 0
    }
    /// A file whose bytes are its own, readable through this handle.
    fn is_plain_file(&self) -> bool {
        !self.is_reparse() && self.attributes & FILE_ATTRIBUTE_DIRECTORY == 0
    }
    /// Creation time in nanoseconds since 1970, as `std` reports it.
    fn born(&self) -> Option<u64> {
        self.created
            .checked_sub(FILETIME_UNIX_EPOCH)
            .and_then(|intervals| intervals.checked_mul(100))
    }
    fn identity(&self) -> Identity {
        Identity {
            dev: self.volume,
            ino: self.index,
            born: self.born(),
        }
    }
}

/// Opens a root itself. A root that is a link (a symbolic link,
/// junction or mount point) or not a directory is refused.
pub fn open_root(path: &Path) -> io::Result<Dir> {
    open_root_shared(path, SHARE_ALL)
}

/// [`open_root`] for a mutation: the root cannot be renamed or deleted
/// while the handle is held.
pub fn open_root_held(path: &Path) -> io::Result<Dir> {
    open_root_shared(path, SHARE_HELD)
}

fn open_root_shared(path: &Path, share: u32) -> io::Result<Dir> {
    let file = std::fs::OpenOptions::new()
        .access_mode(DIRECTORY_ACCESS)
        .share_mode(share)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)?;
    if !info(file.as_handle())?.walkable() {
        return Err(invalid("expected a real directory"));
    }
    Ok(file.into())
}

/// How the volume `dir` is on names its entries: FAT and exFAT by
/// position (named by `GetVolumeInformationByHandleW`), as is a volume
/// whose `FileIdInfo` reports no usable id; every other volume by stable
/// id.
pub fn volume_ids(dir: DirRef<'_>) -> io::Result<crate::VolumeIds> {
    crate::windows::volume_ids(dir)
}

/// Opens one child directory under `parent`, refusing a link and any other
/// reparse folder but a cloud placeholder.
pub fn open_child(parent: DirRef<'_>, name: &str) -> io::Result<Dir> {
    open_child_shared(parent, name, SHARE_ALL)
}

/// [`open_child`] for a mutation: the folder cannot be renamed or deleted
/// while the handle is held.
pub fn open_child_held(parent: DirRef<'_>, name: &str) -> io::Result<Dir> {
    open_child_shared(parent, name, SHARE_HELD)
}

fn open_child_shared(parent: DirRef<'_>, name: &str, share: u32) -> io::Result<Dir> {
    let child = open_existing(parent, name, DIRECTORY_ACCESS, share, FILE_DIRECTORY_FILE)?;
    if !info(child.as_handle())?.walkable() {
        return Err(invalid("a link is never followed"));
    }
    Ok(child)
}

/// Bytes this account may still write on `dir`'s volume, asked of the
/// handle rather than a path.
pub fn available_space(dir: DirRef<'_>) -> io::Result<u64> {
    #[repr(C)]
    #[derive(Default)]
    struct FullSize {
        total_units: i64,
        caller_available_units: i64,
        actual_available_units: i64,
        sectors_per_unit: u32,
        bytes_per_sector: u32,
    }
    let mut size = FullSize::default();
    let mut status_block = IO_STATUS_BLOCK::default();
    // SAFETY: the borrowed handle is live; the output is a writable struct
    // laid out as FILE_FS_FULL_SIZE_INFORMATION, of the size passed.
    let status = unsafe {
        NtQueryVolumeInformationFile(
            dir.as_raw_handle(),
            &mut status_block,
            (&mut size as *mut FullSize).cast(),
            std::mem::size_of::<FullSize>() as u32,
            FileFsFullSizeInformation,
        )
    };
    nt_result(status)?;
    Ok(u64::try_from(size.caller_available_units)
        .unwrap_or(0)
        .saturating_mul(u64::from(size.sectors_per_unit))
        .saturating_mul(u64::from(size.bytes_per_sector)))
}

/// Creates one directory level under `parent`. An existing entry, including
/// one differing only in case, fails with `ErrorKind::AlreadyExists`.
pub fn mkdir_at(parent: DirRef<'_>, name: &str) -> io::Result<()> {
    let wide = component(name)?;
    create_at(
        parent,
        &wide,
        FILE_LIST_DIRECTORY | SYNCHRONIZE,
        SHARE_ALL,
        FILE_CREATE,
        FILE_DIRECTORY_FILE,
    )
    .map(drop)
}

/// Lists entry names (excluding `.`/`..`) under `dir`. Unbounded: only for
/// callers that need every name; callers that page use
/// [`list_names_bounded`].
pub fn list_names(dir: DirRef<'_>) -> io::Result<Vec<OsString>> {
    Ok(list_names_inner(dir, usize::MAX)?.0)
}

/// Single bounded enumeration: at most `limit` names, and whether more
/// exist. Each scan restarts at the first entry, so repeated scans of one
/// handle each see the whole directory.
pub fn list_names_bounded(dir: DirRef<'_>, limit: usize) -> io::Result<(Vec<OsString>, bool)> {
    list_names_inner(dir, limit)
}

fn list_names_inner(dir: DirRef<'_>, limit: usize) -> io::Result<(Vec<OsString>, bool)> {
    const HEADER: usize = std::mem::offset_of!(FILE_NAMES_INFORMATION, FileName);
    let mut buffer = vec![0u64; 8 * 1024];
    let length = (buffer.len() * 8) as u32;
    let mut names = Vec::new();
    let mut restart = true;
    loop {
        let mut status_block = IO_STATUS_BLOCK::default();
        // SAFETY: the borrowed directory handle is live and synchronous; the
        // buffer is writable, 8-byte aligned and `length` bytes long; no
        // event, APC or name filter is passed.
        let status = unsafe {
            NtQueryDirectoryFile(
                dir.as_raw_handle(),
                std::ptr::null_mut(),
                None,
                std::ptr::null(),
                &mut status_block,
                buffer.as_mut_ptr().cast(),
                length,
                FileNamesInformation,
                false,
                std::ptr::null(),
                restart,
            )
        };
        restart = false;
        if status == STATUS_NO_MORE_FILES {
            return Ok((names, false));
        }
        nt_result(status)?;
        let written = status_block.Information.min(buffer.len() * 8);
        let base = buffer.as_ptr().cast::<u8>();
        let mut offset = 0usize;
        while offset + HEADER <= written {
            if !offset.is_multiple_of(8) {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "directory entry is misaligned",
                ));
            }
            // SAFETY: `offset + HEADER` lies inside the bytes the call wrote,
            // and entries start 8-byte aligned, as FILE_NAMES_INFORMATION
            // requires.
            let (next, bytes) = unsafe {
                let entry = base.add(offset).cast::<FILE_NAMES_INFORMATION>();
                (
                    (*entry).NextEntryOffset as usize,
                    (*entry).FileNameLength as usize,
                )
            };
            if offset + HEADER + bytes > written || !bytes.is_multiple_of(2) {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "directory entry exceeds its listing",
                ));
            }
            // SAFETY: the name's bytes were bounds-checked just above and the
            // name starts 2-byte aligned inside the live buffer.
            let name = unsafe {
                std::slice::from_raw_parts(base.add(offset + HEADER).cast::<u16>(), bytes / 2)
            };
            if name != [u16::from(b'.')] && name != [u16::from(b'.'), u16::from(b'.')] {
                if names.len() >= limit {
                    return Ok((names, true));
                }
                names.push(OsString::from_wide(name));
            }
            if next == 0 {
                break;
            }
            offset += next;
        }
    }
}

/// Volume serial number and file index of an open directory, which pin a
/// root to its object as device and inode do on Unix.
pub fn fstat_dev_ino(dir: DirRef<'_>) -> io::Result<(u64, u64)> {
    let opened = info(dir)?;
    Ok((opened.volume, opened.index))
}

/// [`fstat_dev_ino`] with the creation time, which tells a recycled id apart.
pub fn fstat_identity(dir: DirRef<'_>) -> io::Result<Identity> {
    Ok(info(dir)?.identity())
}

/// What names one filesystem object: its volume serial number and file
/// index, and its creation time in nanoseconds since 1970.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Identity {
    pub dev: u64,
    pub ino: u64,
    pub born: Option<u64>,
}

/// Kind/size/mtime plus identity for one entry under `parent`, not
/// followed. `is_symlink` is any reparse point naming another object: a
/// symbolic link, junction or mount point.
pub struct EntryStat {
    pub is_symlink: bool,
    pub is_dir: bool,
    pub is_file: bool,
    pub size: u64,
    pub mtime_ms: u64,
    pub dev: u64,
    pub ino: u64,
    pub born: Option<u64>,
    /// The attributes a person sets: read-only, hidden, system, archive.
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

/// States one entry under `parent` through a handle holding only attribute
/// access, which needs no read permission and passes any sharing mode.
pub fn stat_at(parent: DirRef<'_>, name: &str) -> io::Result<EntryStat> {
    let handle = open_existing(parent, name, ATTRIBUTES_ACCESS, SHARE_ALL, 0)?;
    stat_handle(handle.as_handle())
}

/// The object `handle` is open on, as [`stat_at`] states an entry.
fn stat_handle(handle: BorrowedHandle<'_>) -> io::Result<EntryStat> {
    let opened = info(handle)?;
    Ok(EntryStat {
        is_symlink: opened.is_link(),
        is_dir: opened.is_dir(),
        is_file: opened.is_file(),
        size: if opened.is_file() { opened.size } else { 0 },
        mtime_ms: opened.written.saturating_sub(FILETIME_UNIX_EPOCH) / 10_000,
        dev: opened.volume,
        ino: opened.index,
        born: opened.born(),
        attributes: opened.attributes & 0x27,
    })
}

/// Deletes the file `name` from `dir` when `check`, shown the file as the
/// deleting handle states it, holds: the object deleted is the one checked.
/// No one may write it while it is checked. Nothing is moved aside first,
/// so it suits a volume whose ids are entry positions (FAT, exFAT), where
/// a moved entry takes another id. A folder is refused.
pub fn delete_file_checked(
    dir: DirRef<'_>,
    name: &str,
    check: impl FnOnce(&EntryStat) -> bool,
) -> io::Result<()> {
    let node = open_existing(
        dir,
        name,
        DELETE_ACCESS,
        FILE_SHARE_READ | FILE_SHARE_DELETE,
        FILE_NON_DIRECTORY_FILE,
    )?;
    let stated = stat_handle(node.as_handle())?;
    if stated.is_symlink || !check(&stated) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "delete source changed",
        ));
    }
    delete_by_handle(node.as_handle())
}

/// Renames the object `object` is open on to `name` in `dir`.
/// `FileRenameInformation` resolves `name` relative to `dir`, so neither
/// side is looked up by a public path.
fn rename_by_handle(
    object: BorrowedHandle<'_>,
    dir: DirRef<'_>,
    name: &[u16],
    replace: bool,
) -> io::Result<()> {
    const HEADER: usize = std::mem::offset_of!(FILE_RENAME_INFORMATION, FileName);
    let bytes = HEADER + name.len() * 2;
    let mut buffer = vec![
        0u64;
        bytes
            .div_ceil(8)
            .max(std::mem::size_of::<FILE_RENAME_INFORMATION>() / 8)
    ];
    let information = buffer.as_mut_ptr().cast::<FILE_RENAME_INFORMATION>();
    // SAFETY: the buffer is 8-byte aligned, zeroed and at least
    // size_of::<FILE_RENAME_INFORMATION>() and `bytes` long, so each field
    // write and the name copy after HEADER stay inside it and padding stays
    // zeroed; `name` does not overlap the freshly allocated buffer.
    unsafe {
        std::ptr::addr_of_mut!((*information).Anonymous).write(FILE_RENAME_INFORMATION_0 {
            Flags: u32::from(replace),
        });
        std::ptr::addr_of_mut!((*information).RootDirectory).write(dir.as_raw_handle());
        std::ptr::addr_of_mut!((*information).FileNameLength).write((name.len() * 2) as u32);
        std::ptr::copy_nonoverlapping(
            name.as_ptr(),
            buffer.as_mut_ptr().cast::<u8>().add(HEADER).cast::<u16>(),
            name.len(),
        );
    }
    let mut status_block = IO_STATUS_BLOCK::default();
    // SAFETY: the object and directory handles are borrowed and live; the
    // buffer holds a complete FILE_RENAME_INFORMATION, padded to the struct's
    // size for a short name, and its whole length is passed.
    let status = unsafe {
        NtSetInformationFile(
            object.as_raw_handle(),
            &mut status_block,
            buffer.as_ptr().cast(),
            (buffer.len() * 8) as u32,
            FileRenameInformation,
        )
    };
    nt_result(status)
}

/// Marks the object `handle` is open on for deletion. POSIX semantics take
/// the name away at once; volumes without them delete when the handle
/// closes. A directory must be empty.
fn delete_by_handle(handle: BorrowedHandle<'_>) -> io::Result<()> {
    let extended = FILE_DISPOSITION_INFORMATION_EX {
        // Unix removes a read-only file from a writable folder; so does this.
        Flags: FILE_DISPOSITION_DELETE
            | FILE_DISPOSITION_POSIX_SEMANTICS
            | FILE_DISPOSITION_IGNORE_READONLY_ATTRIBUTE,
    };
    let mut status_block = IO_STATUS_BLOCK::default();
    // SAFETY: the borrowed handle is live and holds DELETE access; the input
    // is a complete FILE_DISPOSITION_INFORMATION_EX of the size passed.
    let status = unsafe {
        NtSetInformationFile(
            handle.as_raw_handle(),
            &mut status_block,
            (&extended as *const FILE_DISPOSITION_INFORMATION_EX).cast(),
            std::mem::size_of::<FILE_DISPOSITION_INFORMATION_EX>() as u32,
            FileDispositionInformationEx,
        )
    };
    if !matches!(
        status,
        STATUS_INVALID_INFO_CLASS
            | STATUS_INVALID_PARAMETER
            | STATUS_NOT_IMPLEMENTED
            | STATUS_NOT_SUPPORTED
    ) {
        return nt_result(status);
    }
    let plain = FILE_DISPOSITION_INFORMATION { DeleteFile: true };
    // SAFETY: as above, with a complete FILE_DISPOSITION_INFORMATION.
    let status = unsafe {
        NtSetInformationFile(
            handle.as_raw_handle(),
            &mut status_block,
            (&plain as *const FILE_DISPOSITION_INFORMATION).cast(),
            std::mem::size_of::<FILE_DISPOSITION_INFORMATION>() as u32,
            FileDispositionInformation,
        )
    };
    nt_result(status)
}

/// Same-directory atomic no-replace rename of the entry itself (a link is
/// renamed, never its target).
pub fn rename_at_noreplace(dir: DirRef<'_>, old: &str, new: &str) -> io::Result<()> {
    let new = component(new)?;
    let source = open_existing(dir, old, DELETE_ACCESS, SHARE_ALL, 0)?;
    rename_by_handle(source.as_handle(), dir, &new, false)
}

/// Cross-directory rename for publication: `staged` (outside the root) into
/// the anchored destination dir. `replace` allows overwriting a file;
/// otherwise no-replace. Strictly a rename: across volumes it fails with
/// `ErrorKind::CrossesDevices` and never copies.
pub fn rename_from_staged(
    staged: &Path,
    dest_dir: DirRef<'_>,
    dest_name: &str,
    replace: bool,
) -> io::Result<()> {
    let name = component(dest_name)?;
    if replace {
        refuse_target_alias(dest_dir, dest_name)?;
    }
    let source = std::fs::OpenOptions::new()
        .access_mode(DELETE_ACCESS)
        .share_mode(SHARE_ALL)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(staged)?;
    if !info(source.as_handle())?.is_plain_file() {
        return Err(invalid("not a regular file"));
    }
    rename_by_handle(source.as_handle(), dest_dir, &name, replace)
}

/// True when `staged` and the open destination dir are on one volume, by
/// volume serial number. A false match only costs a rename that fails as
/// crossing devices, which publication already treats as retryable.
pub fn same_filesystem(staged: &Path, dir: DirRef<'_>) -> io::Result<bool> {
    let source = std::fs::OpenOptions::new()
        .access_mode(ATTRIBUTES_ACCESS)
        .share_mode(SHARE_ALL)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .open(staged)?;
    Ok(info(source.as_handle())?.volume == info(dir)?.volume)
}

/// A temp sibling inside the destination directory, created exclusively so
/// a concurrent publisher never truncates it. The caller streams staged
/// bytes in without holding global locks, syncs, then commits with a
/// same-volume rename of this handle. Dropping without commit deletes it
/// through the same handle.
pub struct SiblingStage<'a> {
    dir: DirRef<'a>,
    file: Option<std::fs::File>,
}

impl<'a> SiblingStage<'a> {
    pub fn create(dir: DirRef<'a>) -> io::Result<Self> {
        for attempt in 0..32u32 {
            let temp = component(&format!(".vnlpub.{attempt}.tmp"))?;
            match create_at(
                dir,
                &temp,
                FILE_GENERIC_WRITE | DELETE,
                SHARE_ALL,
                FILE_CREATE,
                FILE_NON_DIRECTORY_FILE,
            ) {
                Ok(handle) => {
                    return Ok(Self {
                        dir,
                        file: Some(handle.into()),
                    })
                }
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error),
            }
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

    /// Renames the staged temp onto `dest_name` with replace/no-replace
    /// semantics, durability-syncing first. Consumes the stage.
    pub fn commit(self, dest_name: &str, replace: bool) -> io::Result<()> {
        self.commit_inner(dest_name, replace, true)
    }

    /// Rename-only commit for a stage the caller already synced before
    /// taking its authority gate. Consumes the stage.
    pub fn commit_prepared(self, dest_name: &str, replace: bool) -> io::Result<()> {
        self.commit_inner(dest_name, replace, false)
    }

    fn commit_inner(mut self, dest_name: &str, replace: bool, sync_first: bool) -> io::Result<()> {
        let file = self
            .file
            .take()
            .ok_or_else(|| io::Error::new(io::ErrorKind::BrokenPipe, "stage committed"))?;
        let result = (|| {
            let name = component(dest_name)?;
            if sync_first {
                file.sync_all()?;
            }
            if replace {
                refuse_target_alias(self.dir, dest_name)?;
            }
            rename_by_handle(file.as_handle(), self.dir, &name, replace)
        })();
        if result.is_err() {
            // `file` is taken, so Drop treats the stage as settled: remove
            // the orphan here, by the handle that made it.
            let _ = delete_by_handle(file.as_handle());
        }
        result
    }
}

impl Drop for SiblingStage<'_> {
    fn drop(&mut self) {
        if let Some(file) = self.file.take() {
            // Best effort: the temp is engine-named and swept later otherwise.
            let _ = delete_by_handle(file.as_handle());
        }
    }
}

/// Opens a regular file under `parent` and verifies it is the object
/// previously stated (`stat_at`): volume and file index equality proves the
/// name was not swapped between stat and open.
pub fn open_file_verified(
    parent: DirRef<'_>,
    name: &str,
    expected_dev: u64,
    expected_ino: u64,
) -> io::Result<Dir> {
    let file = open_file_at(parent, name)?;
    let opened = info(file.as_handle())?;
    if opened.volume != expected_dev || opened.index != expected_ino {
        return Err(invalid("file changed between stat and open"));
    }
    Ok(file)
}

/// Opens a regular file under `parent` for reading, without following a
/// reparse point; any reparse point is refused, a placeholder included. It
/// is shared for reading only, so while it is open the file is not
/// written, renamed or deleted through any of its names.
pub fn open_file_at(parent: DirRef<'_>, name: &str) -> io::Result<Dir> {
    let file = open_existing(
        parent,
        name,
        FILE_GENERIC_READ,
        FILE_SHARE_READ,
        FILE_NON_DIRECTORY_FILE,
    )?;
    if !info(file.as_handle())?.is_plain_file() {
        return Err(invalid("not a regular file"));
    }
    Ok(file)
}

/// Removes one managed subtree through directory handles. Links are
/// deleted as entries, never traversed. Work is bounded to 64 levels and
/// 10,000 entries; failure leaves remaining entries for a later cleanup.
pub fn remove_tree_at(parent: DirRef<'_>, name: &str) -> io::Result<()> {
    let mut remaining = 10_000usize;
    remove_tree_inner(parent, name, 0, &mut remaining, None)
}

fn changed() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "managed subtree changed")
}

fn remove_tree_inner(
    parent: DirRef<'_>,
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
    // An attribute-only handle names the object without conflicting with
    // the held listing below or the deleting handle after it.
    let probe = open_existing(parent, name, ATTRIBUTES_ACCESS, SHARE_ALL, 0)?;
    let before = info(probe.as_handle())?;
    if expected.is_some_and(|identity| identity != before.identity()) {
        return Err(changed());
    }
    if before.is_dir() {
        if !before.walkable() {
            return Err(invalid("a reparse folder is never entered"));
        }
        if before.volume != info(parent)?.volume {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "managed subtree crosses a volume",
            ));
        }
        // Held without delete sharing: the folder cannot be renamed out
        // from under the walk while its children are removed.
        let listing = open_existing(
            parent,
            name,
            DIRECTORY_ACCESS,
            SHARE_HELD,
            FILE_DIRECTORY_FILE,
        )?;
        if info(listing.as_handle())?.identity() != before.identity() {
            return Err(changed());
        }
        for entry in list_names(listing.as_handle())? {
            let entry = entry.to_str().ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidData, "managed entry encoding")
            })?;
            remove_tree_inner(listing.as_handle(), entry, depth + 1, remaining, None)?;
        }
    }
    // The deleting handle is checked against the probe: the object removed
    // is the object inspected, whatever happened to its name meanwhile.
    let node = open_existing(parent, name, DELETE_ACCESS, SHARE_ALL, 0)?;
    if info(node.as_handle())?.identity() != before.identity() {
        return Err(changed());
    }
    delete_by_handle(node.as_handle())
}

/// A selected object isolated from public filename churn before destructive
/// work: moved, by the handle whose identity was checked, into an
/// exclusively created private directory. A failed or abandoned cleanup
/// leaves that recovery namespace intact.
pub struct DeleteCandidate {
    parent: OwnedHandle,
    /// Held without delete sharing or delete access: it cannot be moved
    /// while held, and a rename into it is not refused by its own handle.
    private: Option<OwnedHandle>,
    private_name: String,
    expected: Identity,
}

impl DeleteCandidate {
    pub fn stage(parent: DirRef<'_>, name: &str, expected: Identity) -> io::Result<Self> {
        let node = open_existing(parent, name, DELETE_ACCESS, SHARE_ALL, 0)?;
        let actual = info(node.as_handle())?;
        if actual.is_link() || actual.identity() != expected {
            // Nothing moved: the replacement keeps its public name.
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "delete source changed",
            ));
        }
        let mut random = [0u8; 16];
        crate::windows::random_bytes(&mut random)?;
        let private_name = format!(
            ".vnldelete.{}",
            random
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>()
        );
        let private = create_at(
            parent,
            &component(&private_name)?,
            DIRECTORY_ACCESS | FILE_ADD_FILE | FILE_ADD_SUBDIRECTORY,
            SHARE_HELD,
            FILE_CREATE,
            FILE_DIRECTORY_FILE,
        )?;
        let candidate = Self {
            parent: parent.try_clone_to_owned()?,
            private: Some(private),
            private_name,
            expected,
        };
        let held = candidate.private.as_ref().expect("held until drop");
        rename_by_handle(
            node.as_handle(),
            held.as_handle(),
            &component("candidate")?,
            false,
        )?;
        Ok(candidate)
    }

    pub fn finish(self) -> io::Result<()> {
        let mut remaining = 10_000usize;
        let held = self.private.as_ref().expect("held until drop");
        remove_tree_inner(
            held.as_handle(),
            "candidate",
            0,
            &mut remaining,
            Some(self.expected),
        )
    }
}

impl Drop for DeleteCandidate {
    fn drop(&mut self) {
        // Removes the private directory only when it is still the one made
        // and is empty: a cleanup that stopped part way keeps what remains
        // recoverable inside it.
        let Some(held) = self.private.take() else {
            return;
        };
        let Ok(made) = info(held.as_handle()).map(|made| made.identity()) else {
            return;
        };
        drop(held);
        if let Ok(private) = open_existing(
            self.parent.as_handle(),
            &self.private_name,
            DELETE_ACCESS,
            SHARE_ALL,
            0,
        ) {
            if info(private.as_handle()).is_ok_and(|now| now.identity() == made) {
                let _ = delete_by_handle(private.as_handle());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    struct Scratch(PathBuf);

    impl Scratch {
        fn new() -> Self {
            use std::sync::atomic::{AtomicU64, Ordering};
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "vnilink-anchored-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(path.join("root")).unwrap();
            std::fs::create_dir_all(path.join("outside")).unwrap();
            std::fs::write(path.join("outside/secret.txt"), b"outside").unwrap();
            Self(path)
        }
        fn root(&self) -> PathBuf {
            self.0.join("root")
        }
        fn outside(&self) -> PathBuf {
            self.0.join("outside")
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
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

    fn names(dir: DirRef<'_>) -> Vec<String> {
        let mut names: Vec<String> = list_names(dir)
            .unwrap()
            .into_iter()
            .map(|name| name.into_string().unwrap())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn a_junction_is_listed_as_a_link_and_never_entered() {
        let scratch = Scratch::new();
        junction(&scratch.root().join("link"), &scratch.outside());
        let root = open_root(&scratch.root()).unwrap();
        assert_eq!(names(root.as_dir()), ["link"]);
        let stat = stat_at(root.as_dir(), "link").unwrap();
        assert!(stat.is_symlink && !stat.is_dir && !stat.is_file);
        assert!(open_child(root.as_dir(), "link").is_err());
        assert!(open_file_at(root.as_dir(), "link").is_err());
        // The junction as a root is refused too.
        assert!(open_root(&scratch.root().join("link")).is_err());
        // Removing the junction removes the link, not what it names.
        remove_tree_at(root.as_dir(), "link").unwrap();
        assert!(names(root.as_dir()).is_empty());
        assert_eq!(
            std::fs::read(scratch.outside().join("secret.txt")).unwrap(),
            b"outside"
        );
    }

    #[test]
    fn a_symbolic_link_is_never_followed() {
        let scratch = Scratch::new();
        let link = scratch.root().join("link.txt");
        if let Err(error) =
            std::os::windows::fs::symlink_file(scratch.outside().join("secret.txt"), &link)
        {
            // Creating a symbolic link needs Developer Mode or elevation;
            // the junction test covers the same refusal without either.
            eprintln!("skipped: this account cannot create symbolic links ({error})");
            return;
        }
        let root = open_root(&scratch.root()).unwrap();
        let stat = stat_at(root.as_dir(), "link.txt").unwrap();
        assert!(stat.is_symlink && !stat.is_file);
        assert!(open_file_at(root.as_dir(), "link.txt").is_err());
        rename_at_noreplace(root.as_dir(), "link.txt", "moved.txt").unwrap();
        assert!(std::fs::symlink_metadata(scratch.root().join("moved.txt"))
            .unwrap()
            .is_symlink());
        assert_eq!(
            std::fs::read(scratch.outside().join("secret.txt")).unwrap(),
            b"outside"
        );
    }

    #[test]
    fn names_windows_would_reinterpret_are_refused_before_any_call() {
        let scratch = Scratch::new();
        std::fs::write(scratch.root().join("file"), b"data").unwrap();
        let root = open_root(&scratch.root()).unwrap();
        for name in [
            "CON",
            "con.txt",
            "Nul",
            "AUX .log",
            "PRN",
            "COM1",
            "lpt9.txt",
            "COM¹",
            "CONIN$",
            "file:stream",
            "file::$DATA",
            "trailing.",
            "trailing ",
            "a<b",
            "a?b",
            "a*b",
            "a|b",
            "a\"b",
            "a/b",
            "a\\b",
            "tab\tname",
            ".",
            "..",
            "",
        ] {
            assert_eq!(
                mkdir_at(root.as_dir(), name).unwrap_err().kind(),
                io::ErrorKind::InvalidInput,
                "{name:?}"
            );
            assert!(stat_at(root.as_dir(), name).is_err(), "{name:?}");
            assert!(open_child(root.as_dir(), name).is_err(), "{name:?}");
        }
        assert!(open_file_at(root.as_dir(), "file:stream").is_err());
        assert!(rename_at_noreplace(root.as_dir(), "file", "file:stream").is_err());
        assert!(rename_at_noreplace(root.as_dir(), "file", "CON").is_err());
        // Names that only look close are ordinary entries.
        for name in ["CONSOLE", "COM10", "nul-file", "a.b.c", " leading"] {
            mkdir_at(root.as_dir(), name).unwrap();
        }
        let mut expected = vec![" leading", "COM10", "CONSOLE", "a.b.c", "file", "nul-file"];
        expected.sort();
        assert_eq!(names(root.as_dir()), expected);
        assert_eq!(std::fs::read(scratch.root().join("file")).unwrap(), b"data");
    }

    #[test]
    fn a_short_name_alias_of_another_entry_is_refused() {
        use std::os::windows::ffi::OsStrExt;
        let scratch = Scratch::new();
        let long = scratch.root().join("a folder with a long name");
        std::fs::create_dir(&long).unwrap();
        let wide: Vec<u16> = long.as_os_str().encode_wide().chain([0]).collect();
        let mut short = vec![0u16; 1024];
        // SAFETY: both buffers are NUL-terminated or sized as passed, and
        // live through the call.
        let length = unsafe {
            windows_sys::Win32::Storage::FileSystem::GetShortPathNameW(
                wide.as_ptr(),
                short.as_mut_ptr(),
                short.len() as u32,
            )
        } as usize;
        if length == 0 || length > short.len() {
            eprintln!("skipped: the short path could not be read");
            return;
        }
        let short = PathBuf::from(OsString::from_wide(&short[..length]));
        let alias = short.file_name().unwrap().to_str().unwrap().to_owned();
        if alias == "a folder with a long name" {
            eprintln!("skipped: this volume does not make 8.3 short names");
            return;
        }
        let root = open_root(&scratch.root()).unwrap();
        assert!(open_child(root.as_dir(), &alias).is_err());
        assert!(stat_at(root.as_dir(), &alias).is_err());
        assert!(open_child(root.as_dir(), "a folder with a long name").is_ok());
        // A real long name with `~` is an ordinary entry.
        mkdir_at(root.as_dir(), "backup~1").unwrap();
        assert!(open_child(root.as_dir(), "backup~1").is_ok());
    }

    #[test]
    fn renames_stay_in_their_directory_and_never_replace() {
        let scratch = Scratch::new();
        std::fs::write(scratch.root().join("a"), b"a").unwrap();
        std::fs::write(scratch.root().join("b"), b"b").unwrap();
        let root = open_root(&scratch.root()).unwrap();
        assert_eq!(
            rename_at_noreplace(root.as_dir(), "a", "b")
                .unwrap_err()
                .kind(),
            io::ErrorKind::AlreadyExists
        );
        // A case variant of another entry is that entry on a case-insensitive
        // volume.
        assert_eq!(
            rename_at_noreplace(root.as_dir(), "a", "B")
                .unwrap_err()
                .kind(),
            io::ErrorKind::AlreadyExists
        );
        assert_eq!(
            rename_at_noreplace(root.as_dir(), "missing", "c")
                .unwrap_err()
                .kind(),
            io::ErrorKind::NotFound
        );
        rename_at_noreplace(root.as_dir(), "a", "c").unwrap();
        // A case-only rename goes through a temporary name, as `vnilink` does.
        rename_at_noreplace(root.as_dir(), "c", ".vnlcase.0.tmp").unwrap();
        rename_at_noreplace(root.as_dir(), ".vnlcase.0.tmp", "C").unwrap();
        assert_eq!(names(root.as_dir()), ["C", "b"]);
        assert_eq!(std::fs::read(scratch.root().join("C")).unwrap(), b"a");
        assert_eq!(std::fs::read(scratch.root().join("b")).unwrap(), b"b");
        assert_eq!(
            mkdir_at(root.as_dir(), "c").unwrap_err().kind(),
            io::ErrorKind::AlreadyExists
        );
    }

    #[test]
    fn a_child_handle_keeps_naming_its_folder_after_a_swap() {
        let scratch = Scratch::new();
        std::fs::create_dir(scratch.root().join("real")).unwrap();
        let root = open_root(&scratch.root()).unwrap();
        let real = open_child(root.as_dir(), "real").unwrap();
        let identity = fstat_dev_ino(real.as_dir()).unwrap();
        std::fs::rename(scratch.root().join("real"), scratch.root().join("moved")).unwrap();
        junction(&scratch.root().join("real"), &scratch.outside());
        assert!(open_child(root.as_dir(), "real").is_err());
        mkdir_at(real.as_dir(), "made").unwrap();
        assert!(scratch.root().join("moved/made").is_dir());
        assert!(!scratch.outside().join("made").exists());
        assert_eq!(fstat_dev_ino(real.as_dir()).unwrap(), identity);
    }

    #[test]
    fn deletion_removes_the_listed_tree_and_only_unlinks_its_junctions() {
        let scratch = Scratch::new();
        let pack = scratch.root().join("pack");
        std::fs::create_dir_all(pack.join("sub")).unwrap();
        std::fs::write(pack.join("sub/a.txt"), b"a").unwrap();
        junction(&pack.join("escape"), &scratch.outside());
        let root = open_root(&scratch.root()).unwrap();
        let before = stat_at(root.as_dir(), "pack").unwrap();
        let candidate = DeleteCandidate::stage(root.as_dir(), "pack", before.identity()).unwrap();
        candidate.finish().unwrap();
        assert!(names(root.as_dir()).is_empty());
        assert_eq!(
            std::fs::read(scratch.outside().join("secret.txt")).unwrap(),
            b"outside"
        );
    }

    #[test]
    fn deletion_refuses_a_replacement_and_leaves_it_in_place() {
        let scratch = Scratch::new();
        std::fs::write(scratch.root().join("selected"), b"original").unwrap();
        let root = open_root(&scratch.root()).unwrap();
        let before = stat_at(root.as_dir(), "selected").unwrap();
        std::fs::rename(
            scratch.root().join("selected"),
            scratch.root().join("original"),
        )
        .unwrap();
        std::fs::create_dir(scratch.root().join("selected")).unwrap();
        std::fs::write(scratch.root().join("selected/sentinel"), b"replacement").unwrap();
        assert_eq!(
            DeleteCandidate::stage(root.as_dir(), "selected", before.identity())
                .err()
                .unwrap()
                .kind(),
            io::ErrorKind::InvalidData
        );
        assert_eq!(
            std::fs::read(scratch.root().join("selected/sentinel")).unwrap(),
            b"replacement"
        );
        assert_eq!(names(root.as_dir()), ["original", "selected"]);
    }

    #[test]
    fn a_staged_publish_lands_once_and_an_abandoned_stage_leaves_nothing() {
        let scratch = Scratch::new();
        let root = open_root(&scratch.root()).unwrap();
        let mut stage = SiblingStage::create(root.as_dir()).unwrap();
        stage.write_all(b"prepared bytes").unwrap();
        stage.fsync().unwrap();
        stage.commit_prepared("landed.txt", false).unwrap();
        assert_eq!(
            std::fs::read(scratch.root().join("landed.txt")).unwrap(),
            b"prepared bytes"
        );
        let mut stage = SiblingStage::create(root.as_dir()).unwrap();
        stage.write_all(b"replacement").unwrap();
        assert_eq!(
            stage.commit("landed.txt", false).unwrap_err().kind(),
            io::ErrorKind::AlreadyExists
        );
        drop(SiblingStage::create(root.as_dir()).unwrap());
        assert_eq!(names(root.as_dir()), ["landed.txt"]);
        let mut stage = SiblingStage::create(root.as_dir()).unwrap();
        stage.write_all(b"replaced").unwrap();
        stage.commit("landed.txt", true).unwrap();
        assert_eq!(
            std::fs::read(scratch.root().join("landed.txt")).unwrap(),
            b"replaced"
        );
        assert_eq!(names(root.as_dir()), ["landed.txt"]);
    }

    #[test]
    fn a_staged_file_moves_into_the_anchored_folder_only() {
        let scratch = Scratch::new();
        let staged = scratch.0.join("staged.part");
        std::fs::write(&staged, b"staged").unwrap();
        std::fs::create_dir(scratch.root().join("inbox")).unwrap();
        let root = open_root(&scratch.root()).unwrap();
        let inbox = open_child(root.as_dir(), "inbox").unwrap();
        assert!(same_filesystem(&staged, inbox.as_dir()).unwrap());
        // The folder is swapped for a junction after it was opened: the
        // file still lands in the folder the handle names.
        std::fs::rename(scratch.root().join("inbox"), scratch.root().join("held")).unwrap();
        junction(&scratch.root().join("inbox"), &scratch.outside());
        rename_from_staged(&staged, inbox.as_dir(), "got.txt", false).unwrap();
        assert_eq!(
            std::fs::read(scratch.root().join("held/got.txt")).unwrap(),
            b"staged"
        );
        assert!(!scratch.outside().join("got.txt").exists());
        std::fs::write(&staged, b"again").unwrap();
        assert_eq!(
            rename_from_staged(&staged, inbox.as_dir(), "got.txt", false)
                .unwrap_err()
                .kind(),
            io::ErrorKind::AlreadyExists
        );
        rename_from_staged(&staged, inbox.as_dir(), "got.txt", true).unwrap();
        assert_eq!(
            std::fs::read(scratch.root().join("held/got.txt")).unwrap(),
            b"again"
        );
    }

    #[test]
    fn a_verified_open_refuses_a_swapped_file_and_holds_the_one_it_opened() {
        let scratch = Scratch::new();
        std::fs::write(scratch.root().join("source.bin"), b"source").unwrap();
        let root = open_root(&scratch.root()).unwrap();
        let stat = stat_at(root.as_dir(), "source.bin").unwrap();
        assert!(stat.is_file && stat.size == 6);
        let held = open_file_verified(root.as_dir(), "source.bin", stat.dev, stat.ino).unwrap();
        // Shared for reading only: no writer, rename or delete meanwhile.
        assert!(std::fs::OpenOptions::new()
            .write(true)
            .open(scratch.root().join("source.bin"))
            .is_err());
        assert!(
            std::fs::rename(scratch.root().join("source.bin"), scratch.root().join("x")).is_err()
        );
        drop(held);
        std::fs::rename(
            scratch.root().join("source.bin"),
            scratch.root().join("old.bin"),
        )
        .unwrap();
        std::fs::write(scratch.root().join("source.bin"), b"other").unwrap();
        assert!(open_file_verified(root.as_dir(), "source.bin", stat.dev, stat.ino).is_err());
        assert!(open_file_at(root.as_dir(), "missing").is_err());
        std::fs::create_dir(scratch.root().join("folder")).unwrap();
        assert!(open_file_at(root.as_dir(), "folder").is_err());
    }

    #[test]
    fn bounded_scans_report_overflow_and_each_scan_sees_everything() {
        let scratch = Scratch::new();
        for i in 0..10 {
            std::fs::write(scratch.root().join(format!("entry-{i:04}")), []).unwrap();
        }
        let root = open_root(&scratch.root()).unwrap();
        let (listed, truncated) = list_names_bounded(root.as_dir(), 4).unwrap();
        assert_eq!((listed.len(), truncated), (4, true));
        let (listed, truncated) = list_names_bounded(root.as_dir(), 10).unwrap();
        assert_eq!((listed.len(), truncated), (10, false));
        assert_eq!(list_names(root.as_dir()).unwrap().len(), 10);
        assert_eq!(list_names(root.as_dir()).unwrap().len(), 10);
    }

    #[test]
    fn stat_reports_what_std_reports() {
        let scratch = Scratch::new();
        let path = scratch.root().join("file.txt");
        std::fs::write(&path, b"twelve bytes").unwrap();
        let root = open_root(&scratch.root()).unwrap();
        let stat = stat_at(root.as_dir(), "file.txt").unwrap();
        let meta = std::fs::symlink_metadata(&path).unwrap();
        let since =
            |time: std::time::SystemTime| time.duration_since(std::time::UNIX_EPOCH).unwrap();
        assert_eq!(stat.size, meta.len());
        assert_eq!(
            stat.mtime_ms,
            since(meta.modified().unwrap()).as_millis() as u64
        );
        assert_eq!(
            stat.born,
            Some(since(meta.created().unwrap()).as_nanos() as u64)
        );
        assert_eq!(
            (stat.dev, stat.ino),
            crate::windows::entry_identity(&path).unwrap()
        );
    }

    #[test]
    fn one_character_names_rename_and_publish() {
        let scratch = Scratch::new();
        std::fs::write(scratch.root().join("source"), b"s").unwrap();
        let root = open_root(&scratch.root()).unwrap();
        rename_at_noreplace(root.as_dir(), "source", "a").unwrap();
        let staged = scratch.0.join("staged.part");
        std::fs::write(&staged, b"staged").unwrap();
        rename_from_staged(&staged, root.as_dir(), "b", false).unwrap();
        let mut stage = SiblingStage::create(root.as_dir()).unwrap();
        stage.write_all(b"c").unwrap();
        stage.commit("c", false).unwrap();
        assert_eq!(names(root.as_dir()), ["a", "b", "c"]);
        assert_eq!(std::fs::read(scratch.root().join("b")).unwrap(), b"staged");
    }

    #[test]
    fn a_held_chain_cannot_be_renamed_or_deleted_while_held() {
        let scratch = Scratch::new();
        std::fs::create_dir_all(scratch.root().join("a/b")).unwrap();
        let root = open_root_held(&scratch.root()).unwrap();
        let a = open_child_held(root.as_dir(), "a").unwrap();
        let b = open_child_held(a.as_dir(), "b").unwrap();
        assert!(std::fs::rename(scratch.root().join("a"), scratch.0.join("moved")).is_err());
        assert!(std::fs::rename(scratch.root().join("a/b"), scratch.0.join("moved")).is_err());
        assert!(std::fs::rename(scratch.root(), scratch.0.join("moved")).is_err());
        // Work inside the held folder goes on, renames into it included.
        mkdir_at(b.as_dir(), "made").unwrap();
        let staged = scratch.0.join("staged.part");
        std::fs::write(&staged, b"x").unwrap();
        rename_from_staged(&staged, b.as_dir(), "x", false).unwrap();
        rename_at_noreplace(b.as_dir(), "x", "y").unwrap();
        let before = stat_at(b.as_dir(), "made").unwrap();
        DeleteCandidate::stage(b.as_dir(), "made", before.identity())
            .unwrap()
            .finish()
            .unwrap();
        assert_eq!(names(b.as_dir()), ["y"]);
        drop((b, a, root));
        std::fs::rename(scratch.root().join("a"), scratch.0.join("moved")).unwrap();
    }

    #[test]
    fn read_only_files_are_deleted_as_unix_deletes_them() {
        let scratch = Scratch::new();
        std::fs::create_dir(scratch.root().join("pack")).unwrap();
        for path in [
            scratch.root().join("pack/locked.txt"),
            scratch.root().join("alone.txt"),
        ] {
            std::fs::write(&path, b"locked").unwrap();
            let mut permissions = std::fs::metadata(&path).unwrap().permissions();
            permissions.set_readonly(true);
            std::fs::set_permissions(&path, permissions).unwrap();
        }
        let root = open_root(&scratch.root()).unwrap();
        for name in ["pack", "alone.txt"] {
            let before = stat_at(root.as_dir(), name).unwrap();
            DeleteCandidate::stage(root.as_dir(), name, before.identity())
                .unwrap()
                .finish()
                .unwrap();
        }
        assert!(names(root.as_dir()).is_empty());
    }

    #[test]
    fn free_space_is_asked_of_the_handle() {
        let scratch = Scratch::new();
        let root = open_root(&scratch.root()).unwrap();
        assert!(available_space(root.as_dir()).unwrap() > 0);
        assert_eq!(volume_ids(root.as_dir()).unwrap(), crate::VolumeIds::Stable);
        let identity = fstat_identity(root.as_dir()).unwrap();
        assert!(identity.born.is_some());
        assert_eq!(
            (identity.dev, identity.ino),
            crate::windows::entry_identity(&scratch.root()).unwrap()
        );
    }

    #[test]
    fn only_plain_and_cloud_folders_are_walked() {
        use crate::windows::walkable_directory;
        const DIRECTORY: u32 = 0x10;
        const REPARSE: u32 = 0x400;
        assert!(walkable_directory(DIRECTORY, 0));
        assert!(!walkable_directory(0, 0));
        // IO_REPARSE_TAG_CLOUD and a numbered variant.
        assert!(walkable_directory(DIRECTORY | REPARSE, 0x9000_001A));
        assert!(walkable_directory(DIRECTORY | REPARSE, 0x9000_F01A));
        // Junction, symbolic link, deduplication, WCI.
        for tag in [0xA000_0003, 0xA000_000C, 0x8000_0013, 0x9000_1018] {
            assert!(!walkable_directory(DIRECTORY | REPARSE, tag), "{tag:#x}");
        }
    }
}
