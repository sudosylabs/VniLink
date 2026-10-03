//! File names and the directory they are allowed to land in.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::error::Error;

use super::{
    DIRECTORY_FAILED, MAX_COMPONENTS, MAX_COMPONENT_BYTES, MAX_FILE_BYTES, MAX_NAME_BYTES,
    SAVE_FAILED, TOO_BIG, UNSAFE_NAME,
};

pub(crate) fn check_name(name: &str) -> Result<(), Error> {
    if name.is_empty()
        || name.len() > MAX_NAME_BYTES
        || name.as_bytes().contains(&0)
        || name.contains('\\')
        || name.contains(':')
        || name.starts_with('/')
        || name.ends_with('/')
    {
        return Err(Error::InvalidSetting(UNSAFE_NAME));
    }
    let mut count = 0usize;
    for component in name.split('/') {
        count += 1;
        if count > MAX_COMPONENTS
            || component.is_empty()
            || component == "."
            || component == ".."
            || component.len() > MAX_COMPONENT_BYTES
        {
            return Err(Error::InvalidSetting(UNSAFE_NAME));
        }
    }
    Ok(())
}

pub(crate) fn checked_size(size: u64) -> Result<usize, Error> {
    let size = usize::try_from(size).map_err(|_| Error::InvalidSetting(TOO_BIG))?;
    if size > MAX_FILE_BYTES {
        Err(Error::InvalidSetting(TOO_BIG))
    } else {
        Ok(size)
    }
}

pub(crate) fn check_submission(name: &str, bytes: &[u8]) -> Result<(), Error> {
    check_name(name)?;
    if bytes.len() > MAX_FILE_BYTES {
        Err(Error::InvalidSetting(TOO_BIG))
    } else {
        Ok(())
    }
}

/// Creates the directory when it is missing and stores its canonical path.
///
/// Canonicalize follows a symlink only for this chosen root. Later writes do
/// not canonicalize names inside it.
pub(crate) fn prepare_directory(path: &Path) -> Result<PathBuf, Error> {
    std::fs::create_dir_all(path).map_err(|_| Error::InvalidSetting(DIRECTORY_FAILED))?;
    let canonical =
        std::fs::canonicalize(path).map_err(|_| Error::InvalidSetting(DIRECTORY_FAILED))?;
    let meta = std::fs::symlink_metadata(&canonical)
        .map_err(|_| Error::InvalidSetting(DIRECTORY_FAILED))?;
    if !meta.is_dir() {
        return Err(Error::InvalidSetting(DIRECTORY_FAILED));
    }
    Ok(canonical)
}

/// Writes `bytes` at `name` under `root`.
///
/// `name` is a relative path. A symlink is not a destination, and `..` never
/// leaves `root`. `replace` is required to change an existing regular file.
/// The bytes land in one rename, so a failed call leaves no partial file.
pub(crate) fn write_file(
    root: &Path,
    name: &str,
    bytes: &[u8],
    replace: bool,
) -> Result<(), Error> {
    check_submission(name, bytes)?;
    let parts: Vec<&str> = name.split('/').collect();
    let file_name = parts[parts.len() - 1];
    let mut parent = root.to_path_buf();
    for component in &parts[..parts.len() - 1] {
        parent.push(component);
        if !under_root(root, &parent) {
            return Err(Error::InvalidSetting(UNSAFE_NAME));
        }
        ensure_real_dir(&parent)?;
    }
    let dest = parent.join(file_name);
    if !under_root(root, &dest) {
        return Err(Error::InvalidSetting(UNSAFE_NAME));
    }
    match classify(&dest)? {
        Dest::Reject => return Err(Error::InvalidSetting(UNSAFE_NAME)),
        Dest::File if !replace => return Err(Error::FileExists),
        Dest::File | Dest::Missing => {}
    }
    let temp = write_unique_temp(root, &parent, bytes)?;
    let result = commit(&temp, &dest, replace);
    if result.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    result
}

fn under_root(root: &Path, path: &Path) -> bool {
    path.starts_with(root) && path != root
}

fn ensure_real_dir(path: &Path) -> Result<(), Error> {
    for _ in 0..2 {
        match std::fs::symlink_metadata(path) {
            Ok(meta) if meta.file_type().is_symlink() => {
                return Err(Error::InvalidSetting(UNSAFE_NAME))
            }
            Ok(meta) if meta.is_dir() => return Ok(()),
            Ok(_) => return Err(Error::InvalidSetting(UNSAFE_NAME)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                match std::fs::create_dir(path) {
                    Ok(()) => return Ok(()),
                    Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                    Err(_) => return Err(Error::InvalidSetting(SAVE_FAILED)),
                }
            }
            Err(_) => return Err(Error::InvalidSetting(SAVE_FAILED)),
        }
    }
    match std::fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_symlink() => Err(Error::InvalidSetting(UNSAFE_NAME)),
        Ok(meta) if meta.is_dir() => Ok(()),
        Ok(_) => Err(Error::InvalidSetting(UNSAFE_NAME)),
        Err(_) => Err(Error::InvalidSetting(SAVE_FAILED)),
    }
}

enum Dest {
    Missing,
    File,
    Reject,
}

fn classify(path: &Path) -> Result<Dest, Error> {
    match std::fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_symlink() => Ok(Dest::Reject),
        Ok(meta) if meta.is_dir() => Ok(Dest::Reject),
        Ok(meta) if meta.is_file() => Ok(Dest::File),
        Ok(_) => Ok(Dest::Reject),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Dest::Missing),
        Err(_) => Err(Error::InvalidSetting(SAVE_FAILED)),
    }
}

fn write_unique_temp(root: &Path, parent: &Path, bytes: &[u8]) -> Result<PathBuf, Error> {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    for _ in 0..8 {
        let id = NEXT.fetch_add(1, Ordering::Relaxed);
        let path = parent.join(format!(".{id}.vnlpart"));
        if !under_root(root, &path) {
            continue;
        }
        if write_temp(&path, bytes).is_ok() {
            return Ok(path);
        }
    }
    Err(Error::InvalidSetting(SAVE_FAILED))
}

fn write_temp(path: &Path, bytes: &[u8]) -> Result<(), Error> {
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|_| Error::InvalidSetting(SAVE_FAILED))?;
    if file.write_all(bytes).and_then(|_| file.sync_all()).is_err() {
        drop(file);
        let _ = std::fs::remove_file(path);
        return Err(Error::InvalidSetting(SAVE_FAILED));
    }
    Ok(())
}

fn commit(temp: &Path, dest: &Path, replace: bool) -> Result<(), Error> {
    match classify(dest)? {
        Dest::Reject => Err(Error::InvalidSetting(UNSAFE_NAME)),
        Dest::File if !replace => Err(Error::FileExists),
        Dest::Missing => {
            std::fs::rename(temp, dest).map_err(|_| Error::InvalidSetting(SAVE_FAILED))
        }
        Dest::File => rename_over(temp, dest),
    }
}

/// Unix rename replaces a directory entry and does not follow a symlink.
/// Windows rename fails when the destination exists, so remove a regular
/// file and rename again. A symlink is never removed.
fn rename_over(temp: &Path, dest: &Path) -> Result<(), Error> {
    if std::fs::rename(temp, dest).is_ok() {
        return Ok(());
    }
    match classify(dest)? {
        Dest::File => {
            std::fs::remove_file(dest).map_err(|_| Error::InvalidSetting(SAVE_FAILED))?;
            std::fs::rename(temp, dest).map_err(|_| Error::InvalidSetting(SAVE_FAILED))
        }
        Dest::Missing => {
            std::fs::rename(temp, dest).map_err(|_| Error::InvalidSetting(SAVE_FAILED))
        }
        Dest::Reject => Err(Error::InvalidSetting(UNSAFE_NAME)),
    }
}
