//! Installation paths, namespace checks, and the single-owner lease.
//!
//! One production installation exists per OS user. Persistent engines share
//! it. The lease is `installation.lock` in the engine directory. It is taken
//! before the key or database is inspected and is never deleted.

use std::fs::{self, File};
use std::io::{self, ErrorKind};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use super::StoreFailure;

const DATABASE_NAME: &str = "installation.sqlite3";
const LOCK_NAME: &str = "installation.lock";
const SCREEN_GRANT_NAME: &str = "screen-grant";

pub(crate) struct InstallationPaths {
    pub(crate) engine_dir: PathBuf,
    pub(crate) database: PathBuf,
    pub(crate) journal: PathBuf,
    pub(crate) wal: PathBuf,
    pub(crate) shm: PathBuf,
    pub(crate) lock_file: PathBuf,
    /// The desktop portal's restore token for sharing this screen
    /// (`vnilink_linux::ScreenGrant`), private as the database is.
    pub(crate) screen_grant: PathBuf,
}

impl InstallationPaths {
    pub(crate) fn from_engine(engine_dir: PathBuf) -> Self {
        let database = engine_dir.join(DATABASE_NAME);
        Self {
            journal: sibling(&database, "-journal"),
            wal: sibling(&database, "-wal"),
            shm: sibling(&database, "-shm"),
            lock_file: engine_dir.join(LOCK_NAME),
            screen_grant: engine_dir.join(SCREEN_GRANT_NAME),
            database,
            engine_dir,
        }
    }
}

fn sibling(database: &Path, suffix: &str) -> PathBuf {
    let mut name = database.file_name().unwrap_or_default().to_os_string();
    name.push(suffix);
    database.with_file_name(name)
}

/// Linux data directory for the application, as a function of the inputs.
/// Relative or empty `XDG_DATA_HOME` is ignored. This does not read the environment.
#[cfg(any(target_os = "linux", test))]
pub(crate) fn linux_app_dir(
    home: Option<&Path>,
    xdg: Option<&std::ffi::OsStr>,
) -> Result<PathBuf, ()> {
    if let Some(xdg) = xdg {
        let candidate = Path::new(xdg);
        if !xdg.is_empty() && candidate.is_absolute() {
            return Ok(candidate.join("vnilink"));
        }
    }
    let home = home.filter(|path| !path.as_os_str().is_empty()).ok_or(())?;
    Ok(home.join(".local").join("share").join("vnilink"))
}

/// Longest matching mount's file system type. `None` when no mount contains `path`.
#[cfg(any(target_os = "linux", test))]
pub(crate) fn linux_mount_fstype(mountinfo: &str, path: &Path) -> Option<String> {
    let path_bytes = path.as_os_str().as_encoded_bytes();
    let mut best: Option<(usize, String)> = None;
    for line in mountinfo.lines() {
        let Some((mount, fstype)) = parse_mount_line(line) else {
            continue;
        };
        if !path_has_prefix(path_bytes, &mount) {
            continue;
        }
        if best.as_ref().is_none_or(|(len, _)| mount.len() > *len) {
            best = Some((mount.len(), fstype));
        }
    }
    best.map(|(_, fstype)| fstype)
}

#[cfg(any(target_os = "linux", test))]
pub(crate) fn linux_fstype_is_remote(fstype: &str) -> bool {
    matches!(
        fstype,
        "nfs" | "nfs4" | "cifs" | "smb" | "smb2" | "smb3" | "smbfs"
    )
}

/// Classifies `path` after resolving symlinks in the existing prefix.
/// The returned flag does not change the path used to create the store.
#[cfg(any(target_os = "linux", test))]
pub(crate) fn linux_path_is_remote(mountinfo: &str, path: &Path) -> Result<bool, StoreFailure> {
    let resolved = resolve_mount_path(path)?;
    let Some(fstype) = linux_mount_fstype(mountinfo, &resolved) else {
        return Err(StoreFailure::Unavailable);
    };
    Ok(linux_fstype_is_remote(&fstype))
}

#[cfg(any(target_os = "linux", test))]
const MAX_SYMLINKS: usize = 40;

/// Resolves the deepest existing prefix, then appends any missing suffix.
/// A symlink loop past [`MAX_SYMLINKS`] fails. This does not create directories
/// and is not a new store location.
#[cfg(any(target_os = "linux", test))]
pub(crate) fn resolve_mount_path(path: &Path) -> Result<PathBuf, StoreFailure> {
    let mut hops = 0usize;
    let mut current = PathBuf::new();
    let mut pending = owned_components(path);
    let mut index = 0;
    while index < pending.len() {
        let component = pending[index].clone();
        index += 1;
        match component {
            OwnedComponent::Prefix(prefix) => current.push(prefix),
            OwnedComponent::Root => current.push("/"),
            OwnedComponent::Cur => {}
            OwnedComponent::Parent => {
                current.pop();
            }
            OwnedComponent::Normal(name) => {
                let candidate = current.join(&name);
                match symlink_meta(&candidate) {
                    Err(error) if error.kind() == ErrorKind::NotFound => {
                        current.push(name);
                        for rest in pending.iter().skip(index) {
                            push_owned(&mut current, rest);
                        }
                        return Ok(current);
                    }
                    Err(_) => return Err(StoreFailure::Unavailable),
                    Ok(meta) if meta.file_type().is_symlink() => {
                        hops += 1;
                        if hops > MAX_SYMLINKS {
                            return Err(StoreFailure::Unavailable);
                        }
                        let target =
                            fs::read_link(&candidate).map_err(|_| StoreFailure::Unavailable)?;
                        let mut rest = owned_components(&target);
                        rest.extend(pending.iter().skip(index).cloned());
                        if target.is_absolute() {
                            current = PathBuf::new();
                        }
                        pending = rest;
                        index = 0;
                    }
                    Ok(_) => current.push(name),
                }
            }
        }
    }
    Ok(current)
}

#[cfg(any(target_os = "linux", test))]
#[derive(Clone)]
enum OwnedComponent {
    Prefix(std::ffi::OsString),
    Root,
    Cur,
    Parent,
    Normal(std::ffi::OsString),
}

#[cfg(any(target_os = "linux", test))]
fn owned_components(path: &Path) -> Vec<OwnedComponent> {
    use std::path::Component;
    path.components()
        .map(|component| match component {
            Component::Prefix(prefix) => OwnedComponent::Prefix(prefix.as_os_str().to_os_string()),
            Component::RootDir => OwnedComponent::Root,
            Component::CurDir => OwnedComponent::Cur,
            Component::ParentDir => OwnedComponent::Parent,
            Component::Normal(name) => OwnedComponent::Normal(name.to_os_string()),
        })
        .collect()
}

#[cfg(any(target_os = "linux", test))]
fn push_owned(path: &mut PathBuf, component: &OwnedComponent) {
    match component {
        OwnedComponent::Prefix(prefix) => path.push(prefix),
        OwnedComponent::Root => path.push("/"),
        OwnedComponent::Cur => {}
        OwnedComponent::Parent => {
            path.pop();
        }
        OwnedComponent::Normal(name) => path.push(name),
    }
}

/// Mountinfo octal escapes are raw bytes, not Unicode scalar values.
/// `\303\251` is the two UTF-8 bytes of `é`, so the mount point stays bytes
/// until it is compared with the path.
#[cfg(any(target_os = "linux", test))]
fn parse_mount_line(line: &str) -> Option<(Vec<u8>, String)> {
    let mut fields = Vec::new();
    let mut current = Vec::new();
    let bytes = line.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b' ' || bytes[index] == b'\t' {
            if !current.is_empty() {
                fields.push(std::mem::take(&mut current));
            }
            index += 1;
            continue;
        }
        if bytes[index] == b'\\' && index + 3 < bytes.len() {
            let octal = &bytes[index + 1..index + 4];
            if octal.iter().all(|byte| byte.is_ascii_digit()) {
                if let Ok(text) = std::str::from_utf8(octal) {
                    if let Ok(value) = u8::from_str_radix(text, 8) {
                        current.push(value);
                        index += 4;
                        continue;
                    }
                }
            }
        }
        current.push(bytes[index]);
        index += 1;
    }
    if !current.is_empty() {
        fields.push(current);
    }
    let split = fields.iter().position(|field| field.as_slice() == b"-")?;
    let mount = fields.get(4)?.clone();
    let fstype = std::str::from_utf8(fields.get(split + 1)?).ok()?;
    Some((mount, fstype.to_string()))
}

#[cfg(any(target_os = "linux", test))]
fn path_has_prefix(path: &[u8], mount: &[u8]) -> bool {
    if mount == b"/" {
        return true;
    }
    if path == mount {
        return true;
    }
    path.starts_with(mount) && path.get(mount.len()) == Some(&b'/')
}

pub(crate) fn production_paths() -> Result<InstallationPaths, StoreFailure> {
    prepare_application(&application_dir()?)
}

/// Establishes the engine namespace beneath the platform application directory.
/// Tests exercise the same preparation without resolving a production path.
pub(crate) fn prepare_application(app: &Path) -> Result<InstallationPaths, StoreFailure> {
    ensure_application_parent(app)?;
    let engine = app.join("engine");
    ensure_private_dir(&engine)?;
    if location_is_remote(&engine)? {
        return Err(StoreFailure::Unavailable);
    }
    Ok(InstallationPaths::from_engine(engine))
}

/// macOS shares this parent with Electron's userData. Only the engine child
/// contains installation secrets, so an existing non-writable-by-others parent
/// may retain Electron's permissions. Never repair existing permissions here.
fn ensure_application_parent(path: &Path) -> Result<(), StoreFailure> {
    #[cfg(target_os = "macos")]
    {
        use std::os::unix::fs::MetadataExt;

        let meta = match symlink_meta(path) {
            Err(error) if error.kind() == ErrorKind::NotFound => {
                return ensure_private_dir(path);
            }
            Ok(meta) => meta,
            Err(_) => return Err(StoreFailure::Unavailable),
        };
        // Safety: geteuid has no arguments and returns this process's effective UID.
        let current_user = unsafe { libc::geteuid() };
        if meta.file_type().is_symlink()
            || !meta.is_dir()
            || meta.uid() != current_user
            || meta.mode() & 0o700 != 0o700
            || meta.mode() & 0o022 != 0
        {
            return Err(StoreFailure::Unavailable);
        }
        reject_broader_acl(path)?;
        Ok(())
    }
    #[cfg(not(target_os = "macos"))]
    {
        ensure_private_dir(path)
    }
}

fn application_dir() -> Result<PathBuf, StoreFailure> {
    #[cfg(target_os = "linux")]
    {
        let home = std::env::var_os("HOME").map(PathBuf::from);
        let xdg = std::env::var_os("XDG_DATA_HOME");
        let app = linux_app_dir(home.as_deref(), xdg.as_deref())
            .map_err(|_| StoreFailure::Unavailable)?;
        let base = app.parent().ok_or(StoreFailure::Unavailable)?;
        ensure_platform_dir(base)?;
        if location_is_remote(base)? {
            return Err(StoreFailure::Unavailable);
        }
        return Ok(app);
    }
    #[cfg(not(target_os = "linux"))]
    {
        let base = dirs::data_local_dir().ok_or(StoreFailure::Unavailable)?;
        if !base.is_dir() {
            return Err(StoreFailure::Unavailable);
        }
        if location_is_remote(&base)? {
            return Err(StoreFailure::Unavailable);
        }
        Ok(base.join("VniLink"))
    }
}

/// Creates a missing platform data directory. The application directory below
/// it has the stricter owner-only checks. Existing aliases above that
/// directory stay as the operating system presented them.
#[cfg(target_os = "linux")]
fn ensure_platform_dir(path: &Path) -> Result<(), StoreFailure> {
    if path.as_os_str().is_empty() {
        return Err(StoreFailure::Unavailable);
    }
    if path.is_dir() {
        return Ok(());
    }
    match symlink_meta(path) {
        Ok(_) => Err(StoreFailure::Unavailable),
        Err(error) if error.kind() == ErrorKind::NotFound => {
            if let Some(parent) = path.parent() {
                if parent != path {
                    ensure_platform_dir(parent)?;
                }
            }
            fs::create_dir(path).map_err(|_| StoreFailure::Unavailable)
        }
        Err(_) => Err(StoreFailure::Unavailable),
    }
}

pub(crate) fn ensure_private_dir(path: &Path) -> Result<(), StoreFailure> {
    match symlink_meta(path) {
        Err(error) if error.kind() == ErrorKind::NotFound => create_private_dir(path)?,
        Ok(meta) if meta.file_type().is_symlink() || !meta.is_dir() => {
            return Err(StoreFailure::Unavailable);
        }
        Ok(_) => {}
        Err(_) => return Err(StoreFailure::Unavailable),
    }
    validate_dir(path)
}

fn create_private_dir(path: &Path) -> Result<(), StoreFailure> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        let mut builder = fs::DirBuilder::new();
        builder.mode(0o700);
        builder
            .create(path)
            .map_err(|_| StoreFailure::Unavailable)?;
    }
    #[cfg(windows)]
    {
        windows_support::create_private_dir(path)?;
    }
    Ok(())
}

pub(crate) fn create_private_file(path: &Path) -> Result<(), StoreFailure> {
    #[cfg(unix)]
    {
        let fd = open_unix(
            path,
            libc::O_RDWR | libc::O_CREAT | libc::O_EXCL | libc::O_CLOEXEC | libc::O_NOFOLLOW,
            0o600,
        )?;
        // The descriptor owns the new inode. Closing it leaves the directory entry.
        drop(fd);
    }
    #[cfg(windows)]
    {
        windows_support::create_private_file(path)?;
    }
    validate_file(path)
}

pub(crate) fn validate_dir(path: &Path) -> Result<(), StoreFailure> {
    reject_reparse(path)?;
    let meta = symlink_meta(path).map_err(|_| StoreFailure::Unavailable)?;
    if meta.file_type().is_symlink() || !meta.is_dir() {
        return Err(StoreFailure::Unavailable);
    }
    validate_owner_mode(&meta, 0o700)?;
    reject_broader_acl(path)?;
    Ok(())
}

pub(crate) fn validate_file(path: &Path) -> Result<(), StoreFailure> {
    reject_reparse(path)?;
    let meta = symlink_meta(path).map_err(|_| StoreFailure::Unavailable)?;
    if meta.file_type().is_symlink() || !meta.is_file() {
        return Err(StoreFailure::Unavailable);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if meta.nlink() != 1 {
            return Err(StoreFailure::Unavailable);
        }
    }
    validate_owner_mode(&meta, 0o600)?;
    reject_broader_acl(path)?;
    Ok(())
}

fn validate_owner_mode(meta: &fs::Metadata, expected: u32) -> Result<(), StoreFailure> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        // Safety: `geteuid` takes no pointer and does not dereference memory.
        // It returns this process's effective user id.
        if meta.uid() != unsafe { libc::geteuid() } {
            return Err(StoreFailure::Unavailable);
        }
        // `mode` includes the file-type bits. The permission mask is the low nine.
        if meta.mode() & 0o777 != expected {
            return Err(StoreFailure::Unavailable);
        }
    }
    #[cfg(windows)]
    {
        let _ = (meta, expected);
    }
    Ok(())
}

pub(crate) fn file_len(path: &Path) -> Result<Option<u64>, StoreFailure> {
    match symlink_meta(path) {
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
        Ok(meta) if meta.file_type().is_symlink() => Err(StoreFailure::Corrupt),
        Ok(meta) if meta.is_file() => Ok(Some(meta.len())),
        Ok(_) => Err(StoreFailure::Corrupt),
        Err(_) => Err(StoreFailure::Unavailable),
    }
}

pub(crate) fn sync_file(path: &Path) -> Result<(), StoreFailure> {
    let file = open_sync_file(path)?;
    file.sync_all().map_err(|_| StoreFailure::Unavailable)
}

pub(crate) fn sync_dir(path: &Path) -> Result<(), StoreFailure> {
    #[cfg(unix)]
    {
        // A directory fsync on Unix accepts the read-only descriptor from open.
        let file = File::open(path).map_err(|_| StoreFailure::Unavailable)?;
        file.sync_all().map_err(|_| StoreFailure::Unavailable)?;
        Ok(())
    }
    #[cfg(windows)]
    {
        windows_support::sync_dir(path)
    }
}

fn open_sync_file(path: &Path) -> Result<File, StoreFailure> {
    #[cfg(windows)]
    {
        // FlushFileBuffers fails when the handle was opened without write access.
        fs::OpenOptions::new()
            .write(true)
            .open(path)
            .map_err(|_| StoreFailure::Unavailable)
    }
    #[cfg(not(windows))]
    {
        File::open(path).map_err(|_| StoreFailure::Unavailable)
    }
}

fn symlink_meta(path: &Path) -> io::Result<fs::Metadata> {
    fs::symlink_metadata(path)
}

fn reject_reparse(path: &Path) -> Result<(), StoreFailure> {
    #[cfg(windows)]
    {
        if windows_support::is_reparse(path)? {
            return Err(StoreFailure::Unavailable);
        }
    }
    #[cfg(not(windows))]
    {
        let _ = path;
    }
    Ok(())
}

fn reject_broader_acl(path: &Path) -> Result<(), StoreFailure> {
    #[cfg(target_os = "macos")]
    {
        if macos_acl_allows(path)? {
            return Err(StoreFailure::Unavailable);
        }
    }
    #[cfg(target_os = "linux")]
    {
        if linux_posix_acl_present(path)? {
            return Err(StoreFailure::Unavailable);
        }
    }
    #[cfg(windows)]
    {
        windows_support::reject_broader_dacl(path)?;
    }
    Ok(())
}

#[cfg(unix)]
fn reject_lease_acl(file: &File) -> Result<(), StoreFailure> {
    #[cfg(target_os = "macos")]
    {
        if macos_acl_allows_fd(file)? {
            return Err(StoreFailure::Unavailable);
        }
    }
    #[cfg(target_os = "linux")]
    {
        if linux_posix_acl_present_fd(file)? {
            return Err(StoreFailure::Unavailable);
        }
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        let _ = file;
    }
    Ok(())
}

pub(crate) fn location_is_remote(path: &Path) -> Result<bool, StoreFailure> {
    #[cfg(target_os = "linux")]
    {
        let info =
            fs::read_to_string("/proc/self/mountinfo").map_err(|_| StoreFailure::Unavailable)?;
        return linux_path_is_remote(&info, path);
    }
    #[cfg(target_os = "macos")]
    {
        return macos_is_remote(path);
    }
    #[cfg(windows)]
    {
        return windows_support::is_remote(path);
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
    {
        let _ = path;
        Ok(false)
    }
}

static RESERVED: Mutex<std::collections::BTreeSet<PathBuf>> =
    Mutex::new(std::collections::BTreeSet::new());

pub(crate) struct InstallationLease {
    engine_dir: PathBuf,
    file: File,
}

impl InstallationLease {
    pub(crate) fn acquire(paths: &InstallationPaths) -> Result<Self, StoreFailure> {
        reserve(&paths.engine_dir)?;
        match open_and_lock(&paths.lock_file) {
            Ok(file) => Ok(Self {
                engine_dir: paths.engine_dir.clone(),
                file,
            }),
            Err(error) => {
                unreserve(&paths.engine_dir);
                Err(error)
            }
        }
    }
}

impl Drop for InstallationLease {
    fn drop(&mut self) {
        let _ = self.file.unlock();
        unreserve(&self.engine_dir);
    }
}

fn reserve(path: &Path) -> Result<(), StoreFailure> {
    let mut reserved = RESERVED.lock().unwrap_or_else(|poison| poison.into_inner());
    if reserved.insert(path.to_path_buf()) {
        Ok(())
    } else {
        Err(StoreFailure::Unavailable)
    }
}

fn unreserve(path: &Path) {
    let mut reserved = RESERVED.lock().unwrap_or_else(|poison| poison.into_inner());
    reserved.remove(path);
}

fn open_and_lock(path: &Path) -> Result<File, StoreFailure> {
    let file = open_lock(path)?;
    // `try_lock` does not wait. A held lease is an immediate unavailable store.
    match file.try_lock() {
        Ok(()) => Ok(file),
        Err(fs::TryLockError::WouldBlock) => Err(StoreFailure::Unavailable),
        Err(fs::TryLockError::Error(_)) => Err(StoreFailure::Unavailable),
    }
}

fn open_lock(path: &Path) -> Result<File, StoreFailure> {
    #[cfg(unix)]
    {
        let file = open_unix(
            path,
            libc::O_RDWR | libc::O_CREAT | libc::O_CLOEXEC | libc::O_NOFOLLOW,
            0o600,
        )?;
        validate_file_handle(&file)?;
        // The descriptor is the inode opened with `O_NOFOLLOW`. The ACL is
        // read from that descriptor so a later path swap cannot redirect it.
        reject_lease_acl(&file)?;
        Ok(file)
    }
    #[cfg(windows)]
    {
        let file = windows_support::open_lock(path)?;
        validate_file(path)?;
        Ok(file)
    }
}

#[cfg(unix)]
fn open_unix(path: &Path, flags: i32, mode: i32) -> Result<File, StoreFailure> {
    use std::os::fd::FromRawFd;
    let text = std::ffi::CString::new(path.as_os_str().as_encoded_bytes())
        .map_err(|_| StoreFailure::Unavailable)?;
    // Safety: `path` is a nul-terminated byte string. On success the descriptor
    // is owned here and transferred to `File`. `O_CLOEXEC` keeps it out of
    // children. `O_NOFOLLOW` refuses a symlink at the final component.
    let fd = unsafe { libc::open(text.as_ptr(), flags, mode) };
    if fd < 0 {
        return Err(StoreFailure::Unavailable);
    }
    Ok(unsafe { File::from_raw_fd(fd) })
}

#[cfg(unix)]
fn validate_file_handle(file: &File) -> Result<(), StoreFailure> {
    use std::os::unix::fs::MetadataExt;
    let meta = file.metadata().map_err(|_| StoreFailure::Unavailable)?;
    // Safety: `geteuid` takes no pointer and does not dereference memory.
    // It returns this process's effective user id.
    if !meta.is_file() || meta.nlink() != 1 || meta.uid() != unsafe { libc::geteuid() } {
        return Err(StoreFailure::Unavailable);
    }
    if meta.mode() & 0o777 != 0o600 {
        return Err(StoreFailure::Unavailable);
    }
    Ok(())
}

#[cfg(target_os = "macos")]
mod macos_acl {
    use std::ffi::{c_char, c_int, c_void};

    pub(super) type Acl = *mut c_void;
    pub(super) type AclEntry = *mut c_void;
    pub(super) type AclTag = c_int;
    pub(super) const ACL_TYPE_EXTENDED: c_int = 0x100;
    pub(super) const ACL_FIRST_ENTRY: c_int = 0;
    pub(super) const ACL_NEXT_ENTRY: c_int = -1;
    pub(super) const ACL_EXTENDED_ALLOW: AclTag = 1;

    // libc 0.2 does not bind the macOS ACL calls. The signatures match acl.h.
    unsafe extern "C" {
        pub(super) fn acl_get_link_np(path: *const c_char, type_: c_int) -> Acl;
        pub(super) fn acl_get_fd_np(fd: c_int, type_: c_int) -> Acl;
        pub(super) fn acl_get_entry(acl: Acl, entry_id: c_int, entry_p: *mut AclEntry) -> c_int;
        pub(super) fn acl_get_tag_type(entry: AclEntry, tag: *mut AclTag) -> c_int;
        pub(super) fn acl_free(obj: *mut c_void) -> c_int;
    }
}

#[cfg(target_os = "macos")]
fn macos_acl_allows(path: &Path) -> Result<bool, StoreFailure> {
    let text = std::ffi::CString::new(path.as_os_str().as_encoded_bytes())
        .map_err(|_| StoreFailure::Unavailable)?;
    // Safety: `acl_get_link_np` does not follow the final symlink. A non-null
    // result is an ACL object `macos_acl_result` frees exactly once.
    let acl = unsafe { macos_acl::acl_get_link_np(text.as_ptr(), macos_acl::ACL_TYPE_EXTENDED) };
    macos_acl_result(acl)
}

#[cfg(target_os = "macos")]
fn macos_acl_allows_fd(file: &File) -> Result<bool, StoreFailure> {
    use std::os::unix::io::AsRawFd;
    // Safety: `file` owns the descriptor. `acl_get_fd_np` does not retain it.
    // A non-null result is an ACL object `macos_acl_result` frees exactly once.
    let acl = unsafe { macos_acl::acl_get_fd_np(file.as_raw_fd(), macos_acl::ACL_TYPE_EXTENDED) };
    macos_acl_result(acl)
}

#[cfg(target_os = "macos")]
fn macos_acl_result(acl: macos_acl::Acl) -> Result<bool, StoreFailure> {
    if acl.is_null() {
        // No ACL is ENOENT. Any other failure leaves the object untouched.
        let missing = std::io::Error::last_os_error().raw_os_error() == Some(libc::ENOENT);
        return if missing {
            Ok(false)
        } else {
            Err(StoreFailure::Unavailable)
        };
    }
    let mut allows = false;
    let mut entry: macos_acl::AclEntry = std::ptr::null_mut();
    let mut entry_id = macos_acl::ACL_FIRST_ENTRY;
    loop {
        // Safety: `acl` is still live. `entry` is valid until `acl` is freed.
        let read = unsafe { macos_acl::acl_get_entry(acl, entry_id, &mut entry) };
        if read != 0 {
            break;
        }
        entry_id = macos_acl::ACL_NEXT_ENTRY;
        let mut tag: macos_acl::AclTag = 0;
        let tag_read = unsafe { macos_acl::acl_get_tag_type(entry, &mut tag) };
        if tag_read != 0 || tag == macos_acl::ACL_EXTENDED_ALLOW {
            allows = true;
            break;
        }
    }
    unsafe { macos_acl::acl_free(acl) };
    Ok(allows)
}

#[cfg(target_os = "macos")]
fn macos_is_remote(path: &Path) -> Result<bool, StoreFailure> {
    let text = std::ffi::CString::new(path.as_os_str().as_encoded_bytes())
        .map_err(|_| StoreFailure::Unavailable)?;
    let mut info: libc::statfs = unsafe { std::mem::zeroed() };
    // Safety: `info` is a zeroed `statfs` owned by this frame. `statfs` writes
    // one struct and does not retain the path.
    let read = unsafe { libc::statfs(text.as_ptr(), &mut info) };
    if read != 0 {
        return Err(StoreFailure::Unavailable);
    }
    const MNT_LOCAL: u32 = 0x0000_1000;
    if info.f_flags & MNT_LOCAL == 0 {
        return Ok(true);
    }
    let name = unsafe { std::ffi::CStr::from_ptr(info.f_fstypename.as_ptr()) };
    let name = name.to_string_lossy();
    Ok(matches!(
        name.as_ref(),
        "nfs" | "nfs4" | "smbfs" | "cifs" | "afpfs" | "webdav"
    ))
}

#[cfg(target_os = "linux")]
fn linux_posix_acl_present(path: &Path) -> Result<bool, StoreFailure> {
    let text = std::ffi::CString::new(path.as_os_str().as_encoded_bytes())
        .map_err(|_| StoreFailure::Unavailable)?;
    for name in [c"system.posix_acl_access", c"system.posix_acl_default"] {
        // Safety: both pointers are nul-terminated and not retained. A null
        // buffer asks for the attribute size; it does not write.
        let size = unsafe { libc::getxattr(text.as_ptr(), name.as_ptr(), std::ptr::null_mut(), 0) };
        if size >= 0 {
            return Ok(true);
        }
        let error = io::Error::last_os_error();
        match error.raw_os_error() {
            // `ENOTSUP` is the same value as `EOPNOTSUPP` on Linux.
            Some(libc::ENODATA) | Some(libc::EOPNOTSUPP) => {}
            Some(libc::ERANGE) => return Ok(true),
            _ => return Err(StoreFailure::Unavailable),
        }
    }
    Ok(false)
}

#[cfg(target_os = "linux")]
fn linux_posix_acl_present_fd(file: &File) -> Result<bool, StoreFailure> {
    use std::os::unix::io::AsRawFd;
    let fd = file.as_raw_fd();
    for name in [c"system.posix_acl_access", c"system.posix_acl_default"] {
        // Safety: `fd` is owned by `file`. `name` is a NUL-terminated literal
        // that is not retained. A null buffer asks for the attribute size.
        let size = unsafe { libc::fgetxattr(fd, name.as_ptr(), std::ptr::null_mut(), 0) };
        if size >= 0 {
            return Ok(true);
        }
        let error = io::Error::last_os_error();
        match error.raw_os_error() {
            Some(libc::ENODATA) | Some(libc::EOPNOTSUPP) => {}
            Some(libc::ERANGE) => return Ok(true),
            _ => return Err(StoreFailure::Unavailable),
        }
    }
    Ok(false)
}

#[cfg(windows)]
mod windows_support {
    use std::fs::File;
    use std::os::windows::ffi::OsStrExt;
    use std::path::Path;

    use super::StoreFailure;

    pub(super) fn is_remote(path: &Path) -> Result<bool, StoreFailure> {
        let text = path.as_os_str().to_string_lossy();
        if text.starts_with("\\\\") || text.starts_with("//") {
            return Ok(true);
        }
        let root = drive_root(path).ok_or(StoreFailure::Unavailable)?;
        let wide = wide(&root);
        // Safety: `wide` is a nul-terminated path. `GetDriveTypeW` does not retain it.
        let kind = unsafe {
            windows::Win32::Storage::FileSystem::GetDriveTypeW(windows::core::PCWSTR(wide.as_ptr()))
        };
        // Win32 `DRIVE_REMOTE`. windows 0.61 keeps the constant in WindowsProgramming.
        const DRIVE_REMOTE: u32 = 4;
        Ok(kind == DRIVE_REMOTE)
    }

    pub(super) fn is_reparse(path: &Path) -> Result<bool, StoreFailure> {
        use windows::Win32::Storage::FileSystem::{
            GetFileAttributesW, FILE_ATTRIBUTE_REPARSE_POINT, INVALID_FILE_ATTRIBUTES,
        };
        let wide = wide_path(path)?;
        // Safety: `wide` is nul-terminated and not retained.
        let attributes = unsafe { GetFileAttributesW(windows::core::PCWSTR(wide.as_ptr())) };
        if attributes == INVALID_FILE_ATTRIBUTES {
            return Err(StoreFailure::Unavailable);
        }
        Ok(attributes & FILE_ATTRIBUTE_REPARSE_POINT.0 != 0)
    }

    pub(super) fn create_private_dir(path: &Path) -> Result<(), StoreFailure> {
        use windows::Win32::Storage::FileSystem::CreateDirectoryW;
        let wide = wide_path(path)?;
        let descriptor = security_descriptor()?;
        let mut attributes = windows::Win32::Security::SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<windows::Win32::Security::SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: descriptor.0 .0,
            bInheritHandle: false.into(),
        };
        // Safety: `wide` and `attributes` live for the call. The descriptor is
        // freed when `descriptor` drops.
        unsafe { CreateDirectoryW(windows::core::PCWSTR(wide.as_ptr()), Some(&mut attributes)) }
            .map_err(|_| StoreFailure::Unavailable)?;
        Ok(())
    }

    pub(super) fn create_private_file(path: &Path) -> Result<(), StoreFailure> {
        let _file = create_file(path, true)?;
        Ok(())
    }

    pub(super) fn open_lock(path: &Path) -> Result<File, StoreFailure> {
        create_file(path, false)
    }

    pub(super) fn sync_dir(path: &Path) -> Result<(), StoreFailure> {
        use std::os::windows::io::FromRawHandle;
        use windows::Win32::Foundation::CloseHandle;
        use windows::Win32::Storage::FileSystem::{
            CreateFileW, FlushFileBuffers, FILE_FLAG_BACKUP_SEMANTICS, FILE_GENERIC_READ,
            FILE_GENERIC_WRITE, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
            OPEN_EXISTING,
        };
        let wide = wide_path(path)?;
        // Safety: backup semantics are required to open a directory. Write
        // access is required because FlushFileBuffers rejects a read-only
        // handle. The handle is owned and closed through `File`.
        let handle = unsafe {
            CreateFileW(
                windows::core::PCWSTR(wide.as_ptr()),
                FILE_GENERIC_READ.0 | FILE_GENERIC_WRITE.0,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                None,
                OPEN_EXISTING,
                FILE_FLAG_BACKUP_SEMANTICS,
                None,
            )
        }
        .map_err(|_| StoreFailure::Unavailable)?;
        let file = unsafe { File::from_raw_handle(handle.0) };
        let flushed = unsafe {
            FlushFileBuffers(windows::Win32::Foundation::HANDLE(
                file.as_raw_handle_copy(),
            ))
        };
        // `File` closes the handle. Don't also CloseHandle.
        let _ = CloseHandle;
        flushed.map_err(|_| StoreFailure::Unavailable)?;
        Ok(())
    }

    pub(super) fn reject_broader_dacl(path: &Path) -> Result<(), StoreFailure> {
        use windows::Win32::Security::Authorization::SE_FILE_OBJECT;
        use windows::Win32::Security::Authorization::{GetNamedSecurityInfoW, GetSecurityInfo};
        use windows::Win32::Security::{
            GetAce, GetAclInformation, GetSecurityDescriptorDacl, ACCESS_ALLOWED_ACE, ACE_HEADER,
            ACL, ACL_SIZE_INFORMATION, DACL_SECURITY_INFORMATION,
        };
        let _ = (GetSecurityInfo, GetSecurityDescriptorDacl);
        let wide = wide_path(path)?;
        let mut dacl: *mut ACL = std::ptr::null_mut();
        let mut descriptor = windows::Win32::Security::PSECURITY_DESCRIPTOR::default();
        // Safety: out pointers are local. `descriptor` is freed with LocalFree
        // below. `dacl` points into that allocation.
        let status = unsafe {
            GetNamedSecurityInfoW(
                windows::core::PCWSTR(wide.as_ptr()),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION,
                None,
                None,
                Some(&mut dacl),
                None,
                &mut descriptor,
            )
        };
        if status.is_err() || dacl.is_null() {
            free_descriptor(descriptor);
            return Err(StoreFailure::Unavailable);
        }
        let mut info = ACL_SIZE_INFORMATION::default();
        let info_ok = unsafe {
            GetAclInformation(
                dacl,
                &mut info as *mut _ as *mut _,
                std::mem::size_of::<ACL_SIZE_INFORMATION>() as u32,
                windows::Win32::Security::AclSizeInformation,
            )
        };
        if info_ok.is_err() {
            free_descriptor(descriptor);
            return Err(StoreFailure::Unavailable);
        }
        let user = current_user_sid()?;
        let mut broader = false;
        for index in 0..info.AceCount {
            let mut ace = std::ptr::null_mut();
            let ace_ok = unsafe { GetAce(dacl, index, &mut ace) };
            if ace_ok.is_err() || ace.is_null() {
                broader = true;
                break;
            }
            // Safety: `GetAce` returned an ACE inside `dacl` for this index.
            let header = unsafe { &*(ace as *const ACE_HEADER) };
            const ACCESS_ALLOWED_ACE_TYPE: u8 = 0;
            const ACCESS_DENIED_ACE_TYPE: u8 = 1;
            if header.AceType == ACCESS_DENIED_ACE_TYPE {
                continue;
            }
            if header.AceType != ACCESS_ALLOWED_ACE_TYPE {
                broader = true;
                break;
            }
            let allowed = unsafe { &*(ace as *const ACCESS_ALLOWED_ACE) };
            let sid = windows::Win32::Security::PSID(
                (&allowed.SidStart as *const u32).cast_mut() as *mut _
            );
            let sid_text = sid_string(sid).unwrap_or_default();
            if sid_text != user && sid_text != "S-1-5-18" {
                broader = true;
                break;
            }
        }
        free_descriptor(descriptor);
        if broader {
            Err(StoreFailure::Unavailable)
        } else {
            Ok(())
        }
    }

    struct Descriptor(windows::Win32::Security::PSECURITY_DESCRIPTOR);

    impl Drop for Descriptor {
        fn drop(&mut self) {
            free_descriptor(self.0);
            self.0 = windows::Win32::Security::PSECURITY_DESCRIPTOR::default();
        }
    }

    fn free_descriptor(descriptor: windows::Win32::Security::PSECURITY_DESCRIPTOR) {
        if !descriptor.0.is_null() {
            unsafe {
                windows::Win32::Foundation::LocalFree(Some(windows::Win32::Foundation::HLOCAL(
                    descriptor.0,
                )));
            }
        }
    }

    fn security_descriptor() -> Result<Descriptor, StoreFailure> {
        use windows::Win32::Security::Authorization::ConvertStringSecurityDescriptorToSecurityDescriptorW;
        let user = current_user_sid()?;
        let text = format!("D:P(A;OICI;FA;;;SY)(A;OICI;FA;;;{user})");
        let wide = wide(&text);
        let mut descriptor = windows::Win32::Security::PSECURITY_DESCRIPTOR::default();
        // Safety: `wide` is nul-terminated SDDL. On success `descriptor` owns
        // a LocalAlloc block freed by `Descriptor`.
        unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                windows::core::PCWSTR(wide.as_ptr()),
                1,
                &mut descriptor,
                None,
            )
        }
        .map_err(|_| StoreFailure::Unavailable)?;
        Ok(Descriptor(descriptor))
    }

    fn current_user_sid() -> Result<String, StoreFailure> {
        use windows::Win32::Security::{GetTokenInformation, TokenUser, TOKEN_QUERY, TOKEN_USER};
        use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
        // Safety: the process handle is a pseudo-handle and is not closed.
        // The token handle is closed at the end of this function.
        let mut token = windows::Win32::Foundation::HANDLE::default();
        unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) }
            .map_err(|_| StoreFailure::Unavailable)?;
        let mut length = 0u32;
        let _ = unsafe { GetTokenInformation(token, TokenUser, None, 0, &mut length) };
        let mut buffer = vec![0u8; length as usize];
        unsafe {
            GetTokenInformation(
                token,
                TokenUser,
                Some(buffer.as_mut_ptr() as *mut _),
                length,
                &mut length,
            )
        }
        .map_err(|_| {
            let _ = unsafe { windows::Win32::Foundation::CloseHandle(token) };
            StoreFailure::Unavailable
        })?;
        let user = unsafe { &*(buffer.as_ptr() as *const TOKEN_USER) };
        let text = sid_string(user.User.Sid);
        let _ = unsafe { windows::Win32::Foundation::CloseHandle(token) };
        text
    }

    fn sid_string(sid: windows::Win32::Security::PSID) -> Result<String, StoreFailure> {
        use windows::Win32::Security::Authorization::ConvertSidToStringSidW;
        let mut raw = windows::core::PWSTR::null();
        // Safety: `sid` points at a SID inside a still-live token or ACE buffer.
        // The returned string is freed with LocalFree.
        unsafe { ConvertSidToStringSidW(sid, &mut raw) }.map_err(|_| StoreFailure::Unavailable)?;
        if raw.is_null() {
            return Err(StoreFailure::Unavailable);
        }
        let mut len = 0usize;
        unsafe {
            while *raw.0.add(len) != 0 {
                len += 1;
            }
        }
        let text = String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(raw.0, len) });
        unsafe {
            windows::Win32::Foundation::LocalFree(Some(windows::Win32::Foundation::HLOCAL(
                raw.0 as *mut _,
            )));
        }
        Ok(text)
    }

    fn create_file(path: &Path, exclusive: bool) -> Result<File, StoreFailure> {
        use std::os::windows::io::FromRawHandle;
        use windows::Win32::Storage::FileSystem::{
            CreateFileW, CREATE_NEW, FILE_ATTRIBUTE_NORMAL, FILE_GENERIC_READ, FILE_GENERIC_WRITE,
            FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_ALWAYS,
        };
        let wide = wide_path(path)?;
        let descriptor = security_descriptor()?;
        let mut attributes = windows::Win32::Security::SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<windows::Win32::Security::SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: descriptor.0 .0,
            bInheritHandle: false.into(),
        };
        let disposition = if exclusive { CREATE_NEW } else { OPEN_ALWAYS };
        // Safety: the path and security attributes outlive the call. The handle
        // is transferred to `File`, which closes it.
        let handle = unsafe {
            CreateFileW(
                windows::core::PCWSTR(wide.as_ptr()),
                FILE_GENERIC_READ.0 | FILE_GENERIC_WRITE.0,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                Some(&mut attributes),
                disposition,
                FILE_ATTRIBUTE_NORMAL,
                None,
            )
        }
        .map_err(|_| StoreFailure::Unavailable)?;
        Ok(unsafe { File::from_raw_handle(handle.0) })
    }

    fn drive_root(path: &Path) -> Option<String> {
        use std::path::Component;
        for component in path.components() {
            if let Component::Prefix(prefix) = component {
                let mut text = prefix.as_os_str().to_string_lossy().into_owned();
                if !text.ends_with('\\') {
                    text.push('\\');
                }
                return Some(text);
            }
        }
        None
    }

    fn wide_path(path: &Path) -> Result<Vec<u16>, StoreFailure> {
        Ok(wide(&path.as_os_str().to_string_lossy()))
    }

    fn wide(text: &str) -> Vec<u16> {
        use std::ffi::OsStr;
        OsStr::new(text)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect()
    }

    trait AsRawHandleCopy {
        fn as_raw_handle_copy(&self) -> *mut std::ffi::c_void;
    }

    impl AsRawHandleCopy for File {
        fn as_raw_handle_copy(&self) -> *mut std::ffi::c_void {
            use std::os::windows::io::AsRawHandle;
            self.as_raw_handle()
        }
    }
}

/// The mutex wait used around the installation connection. SQLite's own busy
/// timeout is separate and is also one second.
pub(crate) fn lock_budget() -> Duration {
    Duration::from_millis(1000)
}
