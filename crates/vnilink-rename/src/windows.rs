//! Small Windows filesystem primitives; callers still own path confinement.
use std::{io, os::windows::ffi::OsStrExt, path::Path};
#[link(name = "kernel32")]
unsafe extern "system" {
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
        let open = std::fs::File::open(dir.join("folder/child")).unwrap();
        rename_noreplace(&dir.join("folder"), &dir.join("renamed")).unwrap();
        assert!(!dir.join("folder").exists());
        assert_eq!(std::fs::read(dir.join("renamed/child")).unwrap(), b"child");
        assert_eq!(open.metadata().unwrap().len(), 5);
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
