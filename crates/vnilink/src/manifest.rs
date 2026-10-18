//! Transfer manifests: bounded file/folder descriptions for jobs.
//!
//! A manifest maps relative paths to content hashes, sizes and directory
//! entries. It is built by scanning the sender's approved roots, validated
//! before anything is written on the receiver, and published path by path
//! so folder merges preserve unrelated destination entries.
//!
//! Policy: symlinks are skipped and reported (never followed); empty
//! directories are carried; duplicate paths, traversal, excessive
//! nesting/counts and destination-case collisions are rejected. Preservation
//! beyond bytes is disclosed, not fabricated: only content and the hierarchy
//! move.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use serde::{Deserialize, Serialize};

use crate::error::Error;
use crate::fs::{self, RootSet};

/// Largest files in one manifest.
pub const MAX_MANIFEST_FILES: usize = 5000;
/// Largest total payload of one manifest, in bytes (1 GiB).
pub const MAX_MANIFEST_BYTES: u64 = 1024 * 1024 * 1024;
/// Largest single file in a manifest, in bytes (256 MiB).
pub const MAX_MANIFEST_FILE_BYTES: u64 = 256 * 1024 * 1024;
/// Largest depth of a manifest path, in components.
pub const MAX_MANIFEST_DEPTH: usize = 16;

pub(crate) const MANIFEST_EMPTY: &str = "the transfer is empty";
pub(crate) const MANIFEST_LIMIT: &str = "the transfer exceeds its limits";
pub(crate) const MANIFEST_UNSAFE: &str = "the transfer names an unsafe path";
pub(crate) const MANIFEST_COLLISION: &str = "the transfer collides on the destination";

/// One manifest line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManifestEntry {
    /// Relative path with `/` separators, UTF-8.
    pub path: String,
    /// True for directories (hash is then nil).
    pub is_dir: bool,
    /// Content hash bytes for files; `[0; 32]` for directories.
    pub hash: [u8; 32],
    /// File size in bytes; 0 for directories.
    pub size: u64,
    /// Source mtime in ms for change detection; 0 when unknown.
    pub source_mtime_ms: u64,
}

/// A validated transfer description.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    pub entries: Vec<ManifestEntry>,
}

impl Manifest {
    /// Total file payload in bytes.
    pub fn total_bytes(&self) -> u64 {
        self.entries.iter().map(|entry| entry.size).sum()
    }

    pub fn file_count(&self) -> usize {
        self.entries.iter().filter(|entry| !entry.is_dir).count()
    }
}

/// A scanned source tree before hashing: paths, sizes and mtimes.
/// `root_rel` is the root-relative path (e.g. `docs/sub/a.txt`) used to
/// re-open the file through directory handles at import time; `absolute`
/// is retained for compatible diagnostics but must never be reopened for
/// bytes (handles protect listing/stat, `root_rel` + `O_NOFOLLOW` protects
/// the read).
#[derive(Debug, Clone)]
pub struct ScannedFile {
    pub rel: String,
    pub absolute: PathBuf,
    pub root_rel: String,
    pub size: u64,
    pub mtime_ms: u64,
    pub dev: u64,
    pub ino: u64,
}

#[derive(Debug, Clone)]
pub struct Scan {
    pub files: Vec<ScannedFile>,
    pub dirs: Vec<String>,
    pub skipped_symlinks: u64,
}

/// Per-directory test probes for detached scanning. Passed by the caller
/// (unit tests) rather than installed globally, so concurrent suites never
/// observe them. Production passes [`ScanHooks::none`].
#[derive(Clone, Copy, Default)]
pub(crate) struct ScanHooks {
    /// Called with index 0 before any filesystem access, then once per
    /// listed directory with 1-based indices. A probe that parks
    /// (controlled slow scan) must also watch `cancelled` and return
    /// promptly once it is set.
    pub(crate) dir_probe: Option<fn(u64, &AtomicBool)>,
    /// Called once when the worker finishes; `true` reports cancellation.
    pub(crate) finish_probe: Option<fn(bool)>,
}

impl ScanHooks {
    pub(crate) fn none() -> Self {
        Self::default()
    }
}

/// Dropping the scan future cancels its detached blocking worker between
/// directories; never rely on dropping the JoinHandle alone.
struct ScanGuard(std::sync::Arc<AtomicBool>);

impl Drop for ScanGuard {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
    }
}

/// Walks `rel` under root `id` and returns the sorted scan. `rel` names
/// either a directory (its tree is scanned) or a single file (the scan
/// holds just it, so selections of individual files transfer too).
/// Symlinks are skipped and counted, never followed. Non-UTF-8 names are
/// skipped. Directory handles anchor every listing and stat so a parent
/// replaced by a symlink cannot escape the approved root.
/// Sync; only for callers outside the runtime (sync offer APIs). Async
/// callers use [`scan_roots_async`] so recursive traversal never occupies
/// a Tokio worker.
pub(crate) fn scan(roots: &RootSet, id: u64, rel: &str) -> Result<Scan, Error> {
    let never = AtomicBool::new(false);
    scan_with_hooks(roots, id, rel, &never, ScanHooks::none())
}

/// Cancellable scan over an already-snapshotted root set. Holds no locks;
/// callers snapshot under a short lock, then await off every lock.
pub(crate) fn scan_with_hooks(
    roots: &RootSet,
    id: u64,
    rel: &str,
    cancelled: &AtomicBool,
    hooks: ScanHooks,
) -> Result<Scan, Error> {
    let aborted = || -> Result<Scan, Error> {
        if let Some(finish) = hooks.finish_probe {
            finish(true);
        }
        Err(Error::NoSession)
    };
    if cancelled.load(Ordering::Acquire) {
        return aborted();
    }
    if let Some(probe) = hooks.dir_probe {
        probe(0, cancelled);
        // The probe may have waited out a cancellation.
        if cancelled.load(Ordering::Acquire) {
            return aborted();
        }
    }
    if let Ok(file) = scan_single_file(roots, id, rel) {
        if let Some(finish) = hooks.finish_probe {
            finish(false);
        }
        return Ok(file);
    }
    #[cfg(unix)]
    {
        scan_anchored(roots, id, rel, cancelled, hooks)
    }
    #[cfg(not(unix))]
    {
        scan_pathname(roots, id, rel, cancelled, hooks)
    }
}

/// Scans a snapshotted root set on a bounded blocking worker (the Tokio
/// blocking pool), checking for cancellation between directories. Yields
/// the async worker while scanning; dropping the future aborts the
/// detached worker promptly instead of orphaning it. Callers must hold no
/// locks across the await and must recheck owner/grant/liveness before
/// using the scan.
pub(crate) async fn scan_roots_async(
    roots: RootSet,
    id: u64,
    rel: String,
    hooks: ScanHooks,
) -> Result<Scan, Error> {
    let cancelled = std::sync::Arc::new(AtomicBool::new(false));
    let _guard = ScanGuard(cancelled.clone());
    let worker_cancelled = cancelled.clone();
    tokio::task::spawn_blocking(move || scan_with_hooks(&roots, id, &rel, &worker_cancelled, hooks))
        .await
        .map_err(|_| Error::NoSession)?
}

/// Runs the directory probe for one unit of traversal. Returns true when
/// the scan must stop.
fn scan_probe(unit: u64, cancelled: &AtomicBool, hooks: ScanHooks) -> bool {
    if cancelled.load(Ordering::Acquire) {
        return true;
    }
    if let Some(probe) = hooks.dir_probe {
        probe(unit, cancelled);
        // The probe may have waited out a cancellation.
        if cancelled.load(Ordering::Acquire) {
            return true;
        }
    }
    false
}

#[cfg(unix)]
fn scan_anchored(
    roots: &RootSet,
    id: u64,
    rel: &str,
    cancelled: &AtomicBool,
    hooks: ScanHooks,
) -> Result<Scan, Error> {
    use std::os::unix::io::AsFd;
    let aborted = || -> Result<Scan, Error> {
        if let Some(finish) = hooks.finish_probe {
            finish(true);
        }
        Err(Error::NoSession)
    };
    // Base is verified through handles; its absolute path is for blob imports
    // that still take paths (verified again at offer time).
    let base = roots.resolve_dir(id, rel)?;
    let mut files = Vec::new();
    let mut dirs = Vec::new();
    let mut skipped_symlinks = 0u64;
    // (absolute dir, root-relative rel, dir fd)
    let base_fd = roots.open_dir_fd(id, rel)?;
    let mut stack: Vec<(std::path::PathBuf, String, std::os::unix::io::OwnedFd)> =
        vec![(base, rel.to_owned(), base_fd)];
    // Probe indices continue the scan sequence: 0 fired before any access.
    let mut unit: u64 = 1;
    while let Some((dir, dir_rel, dir_fd)) = stack.pop() {
        if scan_probe(unit, cancelled, hooks) {
            return aborted();
        }
        unit += 1;
        // Per-directory budget from the global manifest cap: at most
        // `remaining + 1` raw entries are ever allocated, and overflow fails
        // here instead of after unbounded work.
        let remaining = MAX_MANIFEST_FILES.saturating_sub(files.len() + dirs.len());
        let (raw, truncated) =
            vnilink_rename::anchored::list_names_bounded(dir_fd.as_fd(), remaining + 1)
                .map_err(|_| Error::InvalidSetting("the source folder cannot be read"))?;
        if truncated {
            return Err(Error::InvalidSetting(MANIFEST_LIMIT));
        }
        let mut names: Vec<String> = Vec::new();
        for os in raw {
            let Some(name) = os.to_str().map(|s| s.to_owned()) else {
                continue;
            };
            names.push(name);
        }
        names.sort();
        for name in names {
            if files.len() + dirs.len() >= MAX_MANIFEST_FILES {
                return Err(Error::InvalidSetting(MANIFEST_LIMIT));
            }
            let child_rel = if dir_rel.is_empty() {
                name.clone()
            } else {
                format!("{dir_rel}/{name}")
            };
            let manifest_rel = strip_base(rel, &child_rel);
            if manifest_rel.split('/').count() > MAX_MANIFEST_DEPTH {
                return Err(Error::InvalidSetting(MANIFEST_LIMIT));
            }
            let st = vnilink_rename::anchored::stat_at(dir_fd.as_fd(), &name)
                .map_err(|_| Error::InvalidSetting("the source folder cannot be read"))?;
            if st.is_symlink {
                skipped_symlinks += 1;
                continue;
            }
            let full = dir.join(&name);
            if st.is_dir {
                // Re-resolve through the jail: a parent swapped by a symlink
                // after listing refuses here (handle-verified).
                let _ = roots.resolve_dir(id, &child_rel)?;
                let child_fd = vnilink_rename::anchored::open_child(dir_fd.as_fd(), &name)
                    .map_err(|_| Error::InvalidSetting("the source folder cannot be read"))?;
                dirs.push(manifest_rel.clone());
                stack.push((full, child_rel, child_fd));
            } else if st.is_file {
                if st.size > MAX_MANIFEST_FILE_BYTES {
                    return Err(Error::InvalidSetting(MANIFEST_LIMIT));
                }
                files.push(ScannedFile {
                    rel: manifest_rel,
                    absolute: full,
                    root_rel: child_rel,
                    size: st.size,
                    mtime_ms: st.mtime_ms,
                    dev: st.dev,
                    ino: st.ino,
                });
            }
        }
    }
    if files.is_empty() && dirs.is_empty() {
        return Err(Error::InvalidSetting(MANIFEST_EMPTY));
    }
    if let Some(finish) = hooks.finish_probe {
        finish(false);
    }
    Ok(Scan {
        files,
        dirs,
        skipped_symlinks,
    })
}

/// The scanned object's identity, so preparation can refuse a file replaced
/// after the scan: volume serial and file index on Windows, matching what
/// `vnilink_rename::windows::open_source_verified` reports for the same file.
#[cfg(not(unix))]
fn scan_identity(path: &Path, unreadable: &'static str) -> Result<(u64, u64), Error> {
    #[cfg(windows)]
    {
        vnilink_rename::windows::entry_identity(path).map_err(|_| Error::InvalidSetting(unreadable))
    }
    #[cfg(not(windows))]
    {
        let _ = (path, unreadable);
        Ok((0, 0))
    }
}

#[cfg(not(unix))]
fn scan_pathname(
    roots: &RootSet,
    id: u64,
    rel: &str,
    cancelled: &AtomicBool,
    hooks: ScanHooks,
) -> Result<Scan, Error> {
    let aborted = || -> Result<Scan, Error> {
        if let Some(finish) = hooks.finish_probe {
            finish(true);
        }
        Err(Error::NoSession)
    };
    let base = roots.resolve_dir(id, rel)?;
    let mut files = Vec::new();
    let mut dirs = Vec::new();
    let mut skipped_symlinks = 0u64;
    // (absolute dir, root-relative rel of that dir)
    let mut stack = vec![(base, rel.to_owned())];
    // Probe indices continue the scan sequence: 0 fired before any access.
    let mut unit: u64 = 1;
    while let Some((dir, dir_rel)) = stack.pop() {
        if scan_probe(unit, cancelled, hooks) {
            return aborted();
        }
        unit += 1;
        let read = std::fs::read_dir(&dir)
            .map_err(|_| Error::InvalidSetting("the source folder cannot be read"))?;
        // Same per-directory budget as the anchored scan: at most
        // `remaining + 1` raw entries are ever allocated.
        let remaining = MAX_MANIFEST_FILES.saturating_sub(files.len() + dirs.len());
        let mut names: Vec<std::ffi::OsString> = Vec::new();
        for entry in read {
            let entry =
                entry.map_err(|_| Error::InvalidSetting("the source folder cannot be read"))?;
            if names.len() > remaining {
                return Err(Error::InvalidSetting(MANIFEST_LIMIT));
            }
            names.push(entry.file_name());
        }
        names.sort();
        for name in names {
            let Some(name) = name.to_str() else {
                continue;
            };
            if files.len() + dirs.len() >= MAX_MANIFEST_FILES {
                return Err(Error::InvalidSetting(MANIFEST_LIMIT));
            }
            let child_rel = if dir_rel.is_empty() {
                name.to_owned()
            } else {
                format!("{dir_rel}/{name}")
            };
            // Manifest-relative path strips the scan base.
            let manifest_rel = strip_base(rel, &child_rel);
            if manifest_rel.split('/').count() > MAX_MANIFEST_DEPTH {
                return Err(Error::InvalidSetting(MANIFEST_LIMIT));
            }
            let full = dir.join(name);
            let meta = std::fs::symlink_metadata(&full)
                .map_err(|_| Error::InvalidSetting("the source folder cannot be read"))?;
            if meta.file_type().is_symlink() {
                skipped_symlinks += 1;
                continue;
            }
            if meta.is_dir() {
                // Re-resolve through the jail: a parent swapped by a
                // symlink after listing refuses here.
                let _ = roots.resolve_dir(id, &child_rel)?;
                dirs.push(manifest_rel.clone());
                stack.push((full, child_rel));
            } else if meta.is_file() {
                let size = meta.len();
                if size > MAX_MANIFEST_FILE_BYTES {
                    return Err(Error::InvalidSetting(MANIFEST_LIMIT));
                }
                #[cfg(not(unix))]
                let (dev, ino) = scan_identity(&full, "the source folder cannot be read")?;
                files.push(ScannedFile {
                    rel: manifest_rel,
                    absolute: full,
                    root_rel: child_rel,
                    size,
                    mtime_ms: mtime_ms(&meta),
                    #[cfg(unix)]
                    dev: {
                        use std::os::unix::fs::MetadataExt;
                        meta.dev()
                    },
                    #[cfg(unix)]
                    ino: {
                        use std::os::unix::fs::MetadataExt;
                        meta.ino()
                    },
                    #[cfg(not(unix))]
                    dev,
                    #[cfg(not(unix))]
                    ino,
                });
            }
        }
    }
    if files.is_empty() && dirs.is_empty() {
        return Err(Error::InvalidSetting(MANIFEST_EMPTY));
    }
    if let Some(finish) = hooks.finish_probe {
        finish(false);
    }
    Ok(Scan {
        files,
        dirs,
        skipped_symlinks,
    })
}

fn strip_base(base: &str, child: &str) -> String {
    if base.is_empty() {
        return child.to_owned();
    }
    child
        .strip_prefix(base)
        .and_then(|rest| rest.strip_prefix('/'))
        .unwrap_or(child)
        .to_owned()
}

/// A manifest source naming one file: the scan holds that file alone,
/// addressed by its basename so the receiver publishes one file.
/// Anchored: the parent is opened as a handle and the file is stated with
/// `fstatat(AT_SYMLINK_NOFOLLOW)` so a symlink is never followed.
fn scan_single_file(roots: &RootSet, id: u64, rel: &str) -> Result<Scan, Error> {
    if rel.is_empty() {
        return Err(Error::InvalidSetting(MANIFEST_EMPTY));
    }
    let (parent_rel, name) = match rel.rfind('/') {
        Some(slash) => (&rel[..slash], &rel[slash + 1..]),
        None => ("", rel),
    };
    crate::fs::check_name(name).map_err(|_| Error::InvalidSetting(MANIFEST_UNSAFE))?;
    #[cfg(unix)]
    {
        use std::os::unix::io::AsFd;
        let parent_fd = roots
            .open_dir_fd(id, parent_rel)
            .map_err(|_| Error::InvalidSetting(NOT_A_SCAN_DIR))?;
        let st = vnilink_rename::anchored::stat_at(parent_fd.as_fd(), name)
            .map_err(|_| Error::InvalidSetting(NOT_A_SCAN_DIR))?;
        if !st.is_file || st.is_symlink {
            return Err(Error::InvalidSetting(NOT_A_SCAN_DIR));
        }
        if st.size > MAX_MANIFEST_FILE_BYTES {
            return Err(Error::InvalidSetting(MANIFEST_LIMIT));
        }
        let parent = roots
            .resolve_dir(id, parent_rel)
            .map_err(|_| Error::InvalidSetting(NOT_A_SCAN_DIR))?;
        let full = parent.join(name);
        Ok(Scan {
            files: vec![ScannedFile {
                rel: name.to_owned(),
                absolute: full,
                root_rel: rel.to_owned(),
                size: st.size,
                mtime_ms: st.mtime_ms,
                dev: st.dev,
                ino: st.ino,
            }],
            dirs: Vec::new(),
            skipped_symlinks: 0,
        })
    }
    #[cfg(not(unix))]
    {
        let parent = roots
            .resolve_dir(id, parent_rel)
            .map_err(|_| Error::InvalidSetting(NOT_A_SCAN_DIR))?;
        let full = parent.join(name);
        let meta =
            std::fs::symlink_metadata(&full).map_err(|_| Error::InvalidSetting(NOT_A_SCAN_DIR))?;
        if !meta.is_file() || meta.file_type().is_symlink() {
            return Err(Error::InvalidSetting(NOT_A_SCAN_DIR));
        }
        let size = meta.len();
        if size > MAX_MANIFEST_FILE_BYTES {
            return Err(Error::InvalidSetting(MANIFEST_LIMIT));
        }
        let (dev, ino) = scan_identity(&full, NOT_A_SCAN_DIR)?;
        Ok(Scan {
            files: vec![ScannedFile {
                rel: name.to_owned(),
                absolute: full,
                root_rel: rel.to_owned(),
                size,
                mtime_ms: mtime_ms(&meta),
                dev,
                ino,
            }],
            dirs: Vec::new(),
            skipped_symlinks: 0,
        })
    }
}

const NOT_A_SCAN_DIR: &str = "the source folder cannot be read";

/// Prefixes every manifest path with the source directory's basename,
/// so copying a folder lands the folder itself, not just its contents.
/// Single files already carry basenames; the empty root stays flat.
pub(crate) fn prefix_source_dir(source_rel: &str, manifest: &mut Manifest) {
    let Some(base) = source_rel
        .rsplit('/')
        .next()
        .filter(|base| !base.is_empty())
    else {
        return;
    };
    for entry in &mut manifest.entries {
        entry.path = format!("{base}/{}", entry.path);
    }
}

/// Builds a manifest skeleton from a scan with nil hashes; the manager
/// fills hashes as files are imported.
pub(crate) fn skeleton(scan: &Scan) -> Manifest {
    let mut entries = Vec::with_capacity(scan.files.len() + scan.dirs.len());
    for dir in &scan.dirs {
        entries.push(ManifestEntry {
            path: dir.clone(),
            is_dir: true,
            hash: [0; 32],
            size: 0,
            source_mtime_ms: 0,
        });
    }
    for file in &scan.files {
        entries.push(ManifestEntry {
            path: file.rel.clone(),
            is_dir: false,
            hash: [0; 32],
            size: file.size,
            source_mtime_ms: file.mtime_ms,
        });
    }
    entries.sort_by(|a, b| a.path.cmp(&b.path));
    Manifest { entries }
}

/// Validates a manifest's shape and limits. Hash presence is checked by
/// the caller that fills them.
pub(crate) fn validate(manifest: &Manifest) -> Result<(), Error> {
    if manifest.entries.is_empty() || manifest.entries.len() > MAX_MANIFEST_FILES {
        return Err(Error::InvalidSetting(MANIFEST_LIMIT));
    }
    let mut seen = HashSet::with_capacity(manifest.entries.len());
    let mut folded = HashSet::with_capacity(manifest.entries.len());
    let mut total = 0u64;
    for entry in &manifest.entries {
        check_manifest_path(&entry.path)?;
        if !seen.insert(entry.path.clone()) {
            return Err(Error::InvalidSetting(MANIFEST_UNSAFE));
        }
        // Destination-case collision: two paths equal ignoring case would
        // land on one file on Windows/macOS destinations.
        if !folded.insert(entry.path.to_lowercase()) {
            return Err(Error::InvalidSetting(MANIFEST_COLLISION));
        }
        if entry.is_dir {
            if entry.size != 0 || entry.hash != [0; 32] {
                return Err(Error::InvalidSetting(MANIFEST_UNSAFE));
            }
        } else {
            if entry.size > MAX_MANIFEST_FILE_BYTES {
                return Err(Error::InvalidSetting(MANIFEST_LIMIT));
            }
            total = total.saturating_add(entry.size);
            if total > MAX_MANIFEST_BYTES {
                return Err(Error::InvalidSetting(MANIFEST_LIMIT));
            }
        }
    }
    if total == 0 && manifest.entries.iter().all(|entry| entry.is_dir) {
        // Directories alone still need no payload; allowed (empty folders
        // are real content of a folder transfer).
    }
    Ok(())
}

fn check_manifest_path(path: &str) -> Result<(), Error> {
    if path.is_empty()
        || path.len() > 1024
        || path.as_bytes().contains(&0)
        || path.contains('\\')
        || path.contains(':')
        || path.starts_with('/')
        || path.ends_with('/')
    {
        return Err(Error::InvalidSetting(MANIFEST_UNSAFE));
    }
    let mut count = 0usize;
    for component in path.split('/') {
        count += 1;
        if count > MAX_MANIFEST_DEPTH
            || component.is_empty()
            || component == "."
            || component == ".."
            || component.len() > 255
        {
            return Err(Error::InvalidSetting(MANIFEST_UNSAFE));
        }
    }
    Ok(())
}

/// Splits a `name (2).ext`-style free name: finds the first non-existing
/// sibling that keeps the extension. `exists` answers for one candidate.
pub fn keep_both_name(name: &str, mut exists: impl FnMut(&str) -> bool) -> String {
    if !exists(name) {
        return name.to_owned();
    }
    let (stem, extension) = match name.rfind('.') {
        Some(dot) if dot > 0 => (&name[..dot], &name[dot..]),
        _ => (name, ""),
    };
    for n in 2..100000u32 {
        let candidate = format!("{stem} ({n}){extension}");
        if !exists(&candidate) {
            return candidate;
        }
    }
    // Practically unreachable; the caller treats it as a conflict.
    name.to_owned()
}

/// Publishes one staged file into its destination: temp sibling plus an
/// overwrite-capable commit for Replace, atomic no-replace for KeepBoth.
/// Skip leaves the destination untouched. Only manifest paths are touched,
/// so folder merges preserve unrelated entries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PublishPolicy {
    Replace,
    KeepBoth,
}

#[allow(dead_code)]
pub(crate) fn publish_file(
    dest_dir: &Path,
    file_name: &str,
    staged: &Path,
    policy: PublishPolicy,
) -> Result<String, Error> {
    publish_file_with(dest_dir, file_name, staged, policy, &Commits::native())
}

/// How a staged file is moved into place; tests replace the renames.
pub(crate) struct Commits<'a> {
    pub(crate) replace: &'a dyn Fn(&Path, &Path) -> std::io::Result<()>,
    pub(crate) no_replace: &'a dyn Fn(&Path, &Path) -> std::io::Result<()>,
}

impl Commits<'static> {
    fn native() -> Self {
        Self {
            replace: &|from, to| std::fs::rename(from, to),
            no_replace: &|from, to| vnilink_rename::rename_noreplace(from, to),
        }
    }
}

pub(crate) fn publish_file_with(
    dest_dir: &Path,
    file_name: &str,
    staged: &Path,
    policy: PublishPolicy,
    commits: &Commits<'_>,
) -> Result<String, Error> {
    match policy {
        PublishPolicy::Replace => {
            let dest = dest_dir.join(file_name);
            // Preserve the existing destination until the replacement is
            // ready: the staged file is complete, so commit directly with
            // overwrite-capable rename. A concurrent reader may see old or
            // new, never partial.
            match commit_across_volumes(staged, &dest, commits.replace) {
                Ok(()) => Ok(file_name.to_owned()),
                Err(_) => Err(Error::InvalidSetting("the file could not be published")),
            }
        }
        PublishPolicy::KeepBoth => {
            let existing: HashSet<String> = std::fs::read_dir(dest_dir)
                .map_err(|_| Error::InvalidSetting("the destination cannot be read"))?
                .filter_map(|entry| entry.ok())
                .filter_map(|entry| entry.file_name().into_string().ok())
                .collect();
            let free = keep_both_name(file_name, |candidate| existing.contains(candidate));
            if free == file_name {
                // Raced or vanished: no-replace commit decides atomically.
                let dest = dest_dir.join(file_name);
                match commit_across_volumes(staged, &dest, commits.no_replace) {
                    Ok(()) => Ok(file_name.to_owned()),
                    Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                        Err(Error::FileExists)
                    }
                    Err(_) => Err(Error::InvalidSetting("the file could not be published")),
                }
            } else {
                let dest = dest_dir.join(&free);
                commit_across_volumes(staged, &dest, commits.replace)
                    .map_err(|_| Error::InvalidSetting("the file could not be published"))?;
                Ok(free)
            }
        }
    }
}

/// Moves `staged` to `dest` with `rename`. A rename cannot cross volumes (the
/// staging area and a destination root may sit on different drives), so the
/// bytes are first copied to a hidden sibling of `dest` and synced; the final
/// step is still a same-volume rename, and readers never see a partial file.
/// Unix publication pre-stages in the same way with anchored handles.
fn commit_across_volumes(
    staged: &Path,
    dest: &Path,
    rename: &dyn Fn(&Path, &Path) -> std::io::Result<()>,
) -> std::io::Result<()> {
    match rename(staged, dest) {
        Err(error) if error.kind() == std::io::ErrorKind::CrossesDevices => {
            static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let parent = dest.parent().ok_or(error)?;
            let sibling = parent.join(format!(
                ".vnlpub.{}.{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            let landed = std::fs::copy(staged, &sibling)
                // Windows flushes only through a handle with write access.
                .and_then(|_| {
                    std::fs::OpenOptions::new()
                        .write(true)
                        .open(&sibling)?
                        .sync_all()
                })
                .and_then(|()| rename(&sibling, dest));
            if landed.is_ok() {
                let _ = std::fs::remove_file(staged);
            } else {
                let _ = std::fs::remove_file(&sibling);
            }
            landed
        }
        other => other,
    }
}

/// Ensures the manifest's directory entries exist under `dest_root`,
/// creating missing levels. Unrelated entries are untouched.
pub(crate) fn ensure_manifest_dirs(
    roots: &RootSet,
    id: u64,
    rel: &str,
    manifest: &Manifest,
) -> Result<(), Error> {
    for entry in &manifest.entries {
        if !entry.is_dir {
            continue;
        }
        let full_rel = if rel.is_empty() {
            entry.path.clone()
        } else {
            format!("{rel}/{}", entry.path)
        };
        let dir = roots.resolve_dir(id, &full_rel).or_else(|_| {
            // Missing: create level by level through the jail.
            create_levels(roots, id, rel, &entry.path)
        })?;
        let _ = dir;
    }
    Ok(())
}

pub(crate) fn create_levels(
    roots: &RootSet,
    id: u64,
    rel: &str,
    dir_path: &str,
) -> Result<PathBuf, Error> {
    let mut current = rel.to_owned();
    for component in dir_path.split('/') {
        let next_name = component.to_owned();
        let created = fs::create_dir(roots, id, &current, &next_name);
        match created {
            Ok(()) => {}
            Err(Error::FileExists) => {}
            Err(error) => return Err(error),
        }
        current = if current.is_empty() {
            next_name
        } else {
            format!("{current}/{next_name}")
        };
    }
    roots.resolve_dir(id, &current)
}

#[allow(dead_code)]
fn mtime_ms(meta: &std::fs::Metadata) -> u64 {
    meta.modified()
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|age| age.as_millis().try_into().unwrap_or(u64::MAX))
        .unwrap_or(0)
}

/// Compares two trees exactly: same relative paths and types, equal sizes
/// and sha256 for regular files, empty directories present, no extras.
#[cfg(test)]
pub(crate) fn assert_trees_equal(source: &Path, dest: &Path) {
    use sha2::{Digest, Sha256};
    use std::collections::BTreeMap;

    fn walk(root: &Path) -> BTreeMap<String, Vec<u8>> {
        let mut out = BTreeMap::new();
        let mut stack = vec![(root.to_path_buf(), String::new())];
        while let Some((dir, prefix)) = stack.pop() {
            let mut names: Vec<std::ffi::OsString> = std::fs::read_dir(&dir)
                .unwrap()
                .map(|e| e.unwrap().file_name())
                .collect();
            names.sort();
            for name in names {
                let name = name.into_string().unwrap();
                let rel = if prefix.is_empty() {
                    name.clone()
                } else {
                    format!("{prefix}/{name}")
                };
                let full = dir.join(&name);
                let meta = std::fs::symlink_metadata(&full).unwrap();
                if meta.is_dir() {
                    out.insert(rel.clone() + "/", vec![]);
                    stack.push((full, rel));
                } else if meta.is_file() {
                    let bytes = std::fs::read(&full).unwrap();
                    let hash: Vec<u8> = Sha256::digest(&bytes).to_vec();
                    out.insert(rel, hash);
                }
            }
        }
        out
    }

    let left = walk(source);
    let right = walk(dest);
    assert_eq!(left, right, "trees differ");
}
