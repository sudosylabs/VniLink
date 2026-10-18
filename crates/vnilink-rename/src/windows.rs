//! Small Windows filesystem primitives; callers still own path confinement.
use std::{io, os::windows::ffi::OsStrExt, path::Path};
#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetLogicalDrives() -> u32;
    fn GetDiskFreeSpaceExW(
        path: *const u16,
        available: *mut u64,
        total: *mut u64,
        free: *mut u64,
    ) -> i32;
    fn MoveFileExW(old: *const u16, new: *const u16, flags: u32) -> i32;
    fn GetFileInformationByHandle(
        handle: *mut std::ffi::c_void,
        information: *mut HandleInformation,
    ) -> i32;
}
#[repr(C)]
#[derive(Default)]
struct HandleInformation {
    attributes: u32,
    created: [u32; 2],
    accessed: [u32; 2],
    written: [u32; 2],
    volume: u32,
    size_high: u32,
    size_low: u32,
    links: u32,
    index_high: u32,
    index_low: u32,
}
/// Open a directory object without following a final reparse point and
/// return its volume/file identity. The owned handle lets callers keep the
/// identity observation tied to the actual object through their operation.
pub fn open_directory(path: &Path) -> io::Result<std::fs::File> {
    use std::os::windows::fs::OpenOptionsExt;
    let file = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(1 | 2 | 4)
        .custom_flags(0x0200_0000 | 0x0020_0000)
        .open(path)?;
    directory_identity(&file)?;
    Ok(file)
}

pub fn directory_identity(file: &std::fs::File) -> io::Result<(u64, u64)> {
    use std::os::windows::io::AsRawHandle;
    let mut information = HandleInformation::default();
    // SAFETY: the live owned File and correctly laid out output remain
    // valid throughout GetFileInformationByHandle.
    if unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut information) } == 0 {
        return Err(io::Error::last_os_error());
    }
    if information.attributes & 0x400 != 0 || information.attributes & 0x10 == 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "expected a real directory",
        ));
    }
    Ok((
        u64::from(information.volume),
        (u64::from(information.index_high) << 32) | u64::from(information.index_low),
    ))
}

/// Queries a listed object's identity without following a final reparse
/// point. Unlike recording admission, listings also identify directories
/// and files with multiple hard links, and need no content-read permission.
pub fn entry_identity(path: &Path) -> io::Result<(u64, u64)> {
    use std::os::windows::{fs::OpenOptionsExt, io::AsRawHandle};
    let file = std::fs::OpenOptions::new()
        .access_mode(0)
        .share_mode(1 | 2 | 4)
        .custom_flags(0x0200_0000 | 0x0020_0000)
        .open(path)?;
    let mut info = HandleInformation::default();
    // SAFETY: the owned File keeps the handle live, and the correctly laid
    // out information structure remains writable throughout the syscall.
    if unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut info) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok((
        u64::from(info.volume),
        (u64::from(info.index_high) << 32) | u64::from(info.index_low),
    ))
}

pub fn open_single_link_file_rw(path: &Path) -> io::Result<std::fs::File> {
    use std::os::windows::{fs::OpenOptionsExt, io::AsRawHandle};
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .share_mode(0)
        .custom_flags(0x0020_0000)
        .open(path)?; // FILE_FLAG_OPEN_REPARSE_POINT
    let mut information = HandleInformation::default();
    // SAFETY: a live owned File supplies the handle and the correctly laid-out
    // BY_HANDLE_FILE_INFORMATION output remains valid throughout this call.
    if unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut information) } == 0 {
        return Err(io::Error::last_os_error());
    }
    if information.attributes & (0x400 | 0x10) != 0 || information.links != 1 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "recovery requires a single-link regular file",
        ));
    }
    Ok(file)
}
fn wide(path: &Path) -> io::Result<Vec<u16>> {
    let mut result: Vec<_> = path.as_os_str().encode_wide().collect();
    if result.is_empty() || result.contains(&0) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid filesystem path",
        ));
    }
    result.push(0);
    Ok(result)
}
pub fn available_storage(path: &Path) -> io::Result<u64> {
    let mut path = wide(path)?;
    // Directory paths, including UNC share roots, require a trailing separator.
    if path[path.len() - 2] != b'\\' as u16 && path[path.len() - 2] != b'/' as u16 {
        path.pop();
        path.extend([b'\\' as u16, 0]);
    }
    let mut available = 0u64;
    // SAFETY: path is NUL-terminated and lives through the call. The one
    // requested output points to an initialized, aligned 64-bit integer;
    // documented optional outputs are null. No buffer escapes this call.
    if unsafe {
        GetDiskFreeSpaceExW(
            path.as_ptr(),
            &mut available,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok(available)
}
pub fn rename_noreplace(old: &Path, new: &Path) -> io::Result<()> {
    let old = wide(old)?;
    let new = wide(new)?;
    // SAFETY: borrowed NUL-terminated UTF-16 buffers live through the syscall.
    // Zero flags refuse replacement and cross-volume copy emulation. Windows
    // performs the rename; no check-then-rename race or source copying.
    if unsafe { MoveFileExW(old.as_ptr(), new.as_ptr(), 0) } == 0 {
        let error = io::Error::last_os_error();
        return Err(match error.raw_os_error() {
            Some(80 | 183) => io::Error::new(io::ErrorKind::AlreadyExists, error),
            _ => error,
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_no_replace_preserves_both_files_and_renames_directory_object() {
        let dir = std::env::temp_dir().join(format!("vnilink-win-rename-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        struct Cleanup(std::path::PathBuf);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
        let _cleanup = Cleanup(dir.clone());
        std::fs::write(dir.join("source"), b"source").unwrap();
        std::fs::write(dir.join("existing"), b"destination").unwrap();
        assert_eq!(
            rename_noreplace(&dir.join("source"), &dir.join("existing"))
                .unwrap_err()
                .kind(),
            io::ErrorKind::AlreadyExists
        );
        assert_eq!(std::fs::read(dir.join("source")).unwrap(), b"source");
        assert_eq!(std::fs::read(dir.join("existing")).unwrap(), b"destination");
        std::fs::create_dir(dir.join("folder")).unwrap();
        std::fs::write(dir.join("folder/child"), b"child").unwrap();
        let open = open_directory(&dir.join("folder")).unwrap();
        let identity = directory_identity(&open).unwrap();
        rename_noreplace(&dir.join("folder"), &dir.join("renamed")).unwrap();
        assert!(!dir.join("folder").exists());
        assert_eq!(std::fs::read(dir.join("renamed/child")).unwrap(), b"child");
        assert_eq!(directory_identity(&open).unwrap(), identity);
        assert_eq!(
            directory_identity(&open_directory(&dir.join("renamed")).unwrap()).unwrap(),
            identity
        );
        assert_eq!(entry_identity(&dir.join("renamed")).unwrap(), identity);
        let source_identity = entry_identity(&dir.join("source")).unwrap();
        let destination_identity = entry_identity(&dir.join("existing")).unwrap();
        assert_ne!(source_identity, destination_identity);
        assert_ne!(source_identity, identity);
        assert!(available_storage(&dir).unwrap() > 0);
    }
}

pub fn file_identity(file: &std::fs::File) -> io::Result<(u64, u64)> {
    use std::os::windows::io::AsRawHandle;
    let mut info = HandleInformation::default();
    // SAFETY: live File handle and correctly laid-out output.
    if unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut info) } == 0 {
        return Err(io::Error::last_os_error());
    }
    if info.attributes & (0x400 | 0x10) != 0 || info.links != 1 {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    Ok((
        u64::from(info.volume),
        (u64::from(info.index_high) << 32) | u64::from(info.index_low),
    ))
}
/// A source file held so that its absolute path resolves to this object for
/// as long as the value lives: every ancestor directory is pinned against
/// rename, and the file is open with read sharing only, so it cannot be
/// written, renamed or deleted, through any of its links, until dropped.
pub struct VerifiedSource {
    file: std::fs::File,
    identity: (u64, u64),
    size: u64,
    mtime_ms: u64,
    _ancestors: Vec<std::fs::File>,
}

impl VerifiedSource {
    /// The open source, for reading. Clones share the same file object.
    pub fn file(&self) -> &std::fs::File {
        &self.file
    }

    /// Volume serial and file index, as recorded by [`entry_identity`].
    pub fn identity(&self) -> (u64, u64) {
        self.identity
    }

    pub fn size(&self) -> u64 {
        self.size
    }

    /// Last write time in milliseconds since the Unix epoch, truncated as
    /// `std::fs::Metadata::modified` reports it.
    pub fn mtime_ms(&self) -> u64 {
        self.mtime_ms
    }
}

/// Opens a regular file for reading as a [`VerifiedSource`]. A final reparse
/// point is opened as itself and refused, as is a directory. Files with more
/// than one hard link are accepted: sharing applies to the file object, so
/// another link cannot write it either while the source is held.
pub fn open_source_verified(path: &Path) -> io::Result<VerifiedSource> {
    use std::os::windows::{fs::OpenOptionsExt, io::AsRawHandle};
    if !path.is_absolute() {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    let ancestors: Vec<_> = path.ancestors().skip(1).collect();
    if ancestors.len() > 64 {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    let mut pins = Vec::with_capacity(ancestors.len());
    for ancestor in ancestors.into_iter().rev() {
        let pin = pin_delete_entry(ancestor, false)?;
        directory_identity(&pin)?;
        pins.push(pin);
    }
    let file = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(1) // FILE_SHARE_READ: refuse writers, renames and deletes
        .custom_flags(0x0020_0000) // FILE_FLAG_OPEN_REPARSE_POINT
        .open(path)?;
    let mut info = HandleInformation::default();
    // SAFETY: the owned File keeps the handle live, and the correctly laid
    // out information structure remains writable throughout the syscall.
    if unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut info) } == 0 {
        return Err(io::Error::last_os_error());
    }
    if info.attributes & (0x400 | 0x10) != 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "expected a regular file",
        ));
    }
    // FILETIME counts 100 ns intervals since 1601-01-01.
    const UNIX_EPOCH_MS: u64 = 11_644_473_600_000;
    let written = (u64::from(info.written[1]) << 32) | u64::from(info.written[0]);
    Ok(VerifiedSource {
        file,
        identity: (
            u64::from(info.volume),
            (u64::from(info.index_high) << 32) | u64::from(info.index_low),
        ),
        size: (u64::from(info.size_high) << 32) | u64::from(info.size_low),
        mtime_ms: (written / 10_000).saturating_sub(UNIX_EPOCH_MS),
        _ancestors: pins,
    })
}

pub fn open_recorded_candidate(path: &Path) -> io::Result<std::fs::File> {
    use std::os::windows::fs::OpenOptionsExt;
    let file = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(1 | 2 | 4)
        .custom_flags(0x0020_0000)
        .open(path)?;
    file_identity(&file)?;
    Ok(file)
}
#[link(name = "bcrypt")]
unsafe extern "system" {
    fn BCryptGenRandom(
        algorithm: *mut std::ffi::c_void,
        buffer: *mut u8,
        len: u32,
        flags: u32,
    ) -> i32;
}
pub fn random_bytes(bytes: &mut [u8]) -> io::Result<()> {
    // SAFETY: writable buffer is live for the syscall; system RNG flag needs no handle.
    if unsafe {
        BCryptGenRandom(
            std::ptr::null_mut(),
            bytes.as_mut_ptr(),
            bytes.len() as u32,
            2,
        )
    } < 0
    {
        return Err(io::Error::other(
            "recording namespace random generation failed",
        ));
    }
    Ok(())
}
#[link(name = "kernel32")]
unsafe extern "system" {
    fn SetFileInformationByHandle(
        handle: *mut std::ffi::c_void,
        class: u32,
        info: *const std::ffi::c_void,
        size: u32,
    ) -> i32;
}
pub fn delete_file_object(file: &std::fs::File) -> io::Result<()> {
    use std::os::windows::io::AsRawHandle;
    // FILE_DISPOSITION_INFO_EX: DELETE | POSIX_SEMANTICS. The caller closes
    // this last original handle before removing the private directory.
    let flags: u32 = 1 | 2;
    // SAFETY: live DELETE-authorized File and a 4-byte disposition buffer.
    if unsafe {
        SetFileInformationByHandle(file.as_raw_handle(), 21, (&flags as *const u32).cast(), 4)
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// Mounted drive roots, without requesting additional filesystem privileges.
pub fn drive_roots() -> io::Result<Vec<std::path::PathBuf>> {
    let drives = unsafe { GetLogicalDrives() };
    if drives == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok((0..26)
        .filter(|i| drives & (1 << i) != 0)
        .map(|i| std::path::PathBuf::from(format!("{}:\\", char::from(b'A' + i))))
        .collect())
}

/// Delete exactly the listed object. Every ancestor and visited directory is
/// held without FILE_SHARE_DELETE, so pathname enumeration cannot be redirected
/// through a renamed ancestor or a replacement junction. Leaves are removed by
/// their DELETE-authorized handle, never by re-resolving a public pathname.
pub fn delete_tree_checked(path: &Path, expected: (u64, u64)) -> io::Result<()> {
    if !path.is_absolute() {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    let ancestors: Vec<_> = path.ancestors().skip(1).collect();
    if ancestors.len() > 64 {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    let mut pins = Vec::new();
    for ancestor in ancestors.into_iter().rev() {
        let pin = pin_delete_entry(ancestor, false)?;
        directory_identity(&pin)?;
        pins.push(pin);
    }
    let mut remaining = 10_000usize;
    delete_pinned_node(path, Some(expected), expected.0, 0, &mut remaining)
}
fn pin_delete_entry(path: &Path, delete: bool) -> io::Result<std::fs::File> {
    use std::os::windows::fs::OpenOptionsExt;
    std::fs::OpenOptions::new()
        // FILE_READ_ATTRIBUTES plus DELETE for a node, or FILE_LIST_DIRECTORY for an
        // ancestor pin. NTFS skips share checks for opens holding only attribute
        // access, so without one of those rights the share mode below denies nothing.
        .access_mode(0x80 | if delete { 0x0001_0000 } else { 0x1 })
        .share_mode(1 | 2) // deny renaming/deleting this component until its handle closes
        .custom_flags(0x0200_0000 | 0x0020_0000) // backup semantics, open reparse point itself
        .open(path)
}
fn delete_pinned_node(
    path: &Path,
    expected: Option<(u64, u64)>,
    volume: u64,
    depth: usize,
    remaining: &mut usize,
) -> io::Result<()> {
    use std::os::windows::io::AsRawHandle;
    if depth > 64 || *remaining == 0 {
        return Err(io::Error::other("managed subtree exceeds cleanup bounds"));
    }
    *remaining -= 1;
    let node = pin_delete_entry(path, true)?;
    let mut info = HandleInformation::default();
    // SAFETY: live owned handle, correctly laid out writable output structure.
    if unsafe { GetFileInformationByHandle(node.as_raw_handle(), &mut info) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let identity = (
        u64::from(info.volume),
        (u64::from(info.index_high) << 32) | u64::from(info.index_low),
    );
    if identity.0 != volume || expected.is_some_and(|expected| expected != identity) {
        return Err(io::Error::other("managed subtree changed"));
    }
    if info.attributes & 0x400 == 0 && info.attributes & 0x10 != 0 {
        for entry in std::fs::read_dir(path)? {
            delete_pinned_node(&entry?.path(), None, volume, depth + 1, remaining)?;
        }
    }
    delete_file_object(&node)
}

#[cfg(test)]
mod deletion_tests {
    use super::*;
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
    fn directory_pin_prevents_a_concurrent_path_replacement() {
        let path = scratch();
        let child = path.join("selected");
        std::fs::create_dir(&child).unwrap();
        let pinned = pin_delete_entry(&child, false).unwrap();
        assert!(std::fs::rename(&child, path.join("moved")).is_err());
        drop(pinned);
        std::fs::rename(&child, path.join("moved")).unwrap();
        std::fs::remove_dir_all(path).unwrap();
    }
    #[test]
    fn deletion_refuses_a_replacement_and_removes_only_the_selected_tree() {
        let path = scratch();
        let child = path.join("selected");
        std::fs::create_dir(&child).unwrap();
        std::fs::write(child.join("owned"), b"selected").unwrap();
        let identity = entry_identity(&child).unwrap();
        std::fs::rename(&child, path.join("saved")).unwrap();
        std::fs::create_dir(&child).unwrap();
        std::fs::write(child.join("sentinel"), b"unrelated").unwrap();
        assert!(delete_tree_checked(&child, identity).is_err());
        assert_eq!(std::fs::read(child.join("sentinel")).unwrap(), b"unrelated");
        delete_tree_checked(&path.join("saved"), identity).unwrap();
        assert!(!path.join("saved").exists());
        assert_eq!(std::fs::read(child.join("sentinel")).unwrap(), b"unrelated");
        std::fs::remove_dir_all(path).unwrap();
    }
}
