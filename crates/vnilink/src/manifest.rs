// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! Transfer manifests: bounded file/folder descriptions for jobs.
//!
//! A manifest maps relative paths to sizes, modification times and
//! directory entries. It is built by scanning the sender's shared disk, validated
//! before anything is written on the receiver, and published path by path
//! so folder merges preserve unrelated destination entries.
//!
//! Policy: symlinks are skipped and reported (never followed); empty
//! directories are carried; duplicate paths, traversal, excessive
//! nesting/counts and destination-case collisions are rejected. Preservation
//! beyond bytes is disclosed, not fabricated: only content and the hierarchy
//! move.
// trace: records every ignored failure

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use serde::{Deserialize, Serialize};

use crate::error::Error;
use crate::fs::{self, RootSet};

/// Largest number of files and folders in one manifest. Sizes have no cap
/// of their own: the receiver's free-space admission refuses what does not
/// fit. Tests exercise the same bound with a smaller budget.
#[cfg(not(test))]
pub const MAX_MANIFEST_FILES: usize = 50_000;
#[cfg(test)]
pub const MAX_MANIFEST_FILES: usize = 5000;
/// Largest depth of a manifest path, in components.
pub const MAX_MANIFEST_DEPTH: usize = 64;

pub(crate) const MANIFEST_EMPTY: &str = "the transfer is empty";
pub(crate) const MANIFEST_LIMIT: &str = "the transfer exceeds its limits";
pub(crate) const MANIFEST_UNSAFE: &str = "the transfer names an unsafe path";
pub(crate) const MANIFEST_COLLISION: &str = "the transfer collides on the destination";

/// One manifest line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManifestEntry {
    /// Relative path with `/` separators, UTF-8.
    pub path: String,
    /// True for directories.
    pub is_dir: bool,
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

/// A scanned source tree: paths, sizes and mtimes.
/// `root_rel` is the root-relative path (e.g. `docs/sub/a.txt`) used to
/// re-open the file through directory handles when it is sent; `ino` (the
/// Windows file index there) identifies it so a file renamed over it is
/// refused.
#[derive(Debug, Clone)]
pub struct ScannedFile {
    pub rel: String,
    pub root_rel: String,
    pub size: u64,
    pub mtime_ms: u64,
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
pub struct ScanHooks {
    /// Called with index 0 before any filesystem access, then once per
    /// listed directory with 1-based indices. A probe that parks
    /// (controlled slow scan) must also watch `cancelled` and return
    /// promptly once it is set.
    pub dir_probe: Option<fn(u64, &AtomicBool)>,
    /// Called once when the worker finishes; `true` reports cancellation.
    pub finish_probe: Option<fn(bool)>,
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
/// replaced by a symlink during the walk refuses it instead of redirecting
/// it.
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
    #[cfg(any(unix, windows))]
    {
        scan_anchored(roots, id, rel, cancelled, hooks)
    }
    #[cfg(not(any(unix, windows)))]
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

#[cfg(any(unix, windows))]
fn scan_anchored(
    roots: &RootSet,
    id: u64,
    rel: &str,
    cancelled: &AtomicBool,
    hooks: ScanHooks,
) -> Result<Scan, Error> {
    use vnilink_rename::anchored::AsDir;
    let aborted = || -> Result<Scan, Error> {
        if let Some(finish) = hooks.finish_probe {
            finish(true);
        }
        Err(Error::NoSession)
    };
    let mut files = Vec::new();
    let mut dirs = Vec::new();
    let mut skipped_symlinks = 0u64;
    // (root-relative rel, dir fd); the base is verified through handles.
    let base_fd = roots.open_dir_fd(id, rel)?;
    let mut stack: Vec<(String, vnilink_rename::anchored::Dir)> = vec![(rel.to_owned(), base_fd)];
    // Probe indices continue the scan sequence: 0 fired before any access.
    let mut unit: u64 = 1;
    while let Some((dir_rel, dir_fd)) = stack.pop() {
        if scan_probe(unit, cancelled, hooks) {
            return aborted();
        }
        unit += 1;
        // Per-directory budget from the global manifest cap: at most
        // `remaining + 1` raw entries are ever allocated, and overflow fails
        // here instead of after unbounded work.
        let remaining = MAX_MANIFEST_FILES.saturating_sub(files.len() + dirs.len());
        let (raw, truncated) =
            vnilink_rename::anchored::list_names_bounded(dir_fd.as_dir(), remaining + 1)
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
            let st = vnilink_rename::anchored::stat_at(dir_fd.as_dir(), &name)
                .map_err(|_| Error::InvalidSetting("the source folder cannot be read"))?;
            if st.is_symlink {
                skipped_symlinks += 1;
                continue;
            }
            if st.is_dir {
                // Still a folder on the shared disk. The resolution follows
                // a link, so what refuses a folder swapped for one after it
                // was listed is `open_child`, which never follows one.
                let _ = roots.resolve_dir(id, &child_rel)?;
                let child_fd = vnilink_rename::anchored::open_child(dir_fd.as_dir(), &name)
                    .map_err(|_| Error::InvalidSetting("the source folder cannot be read"))?;
                dirs.push(manifest_rel.clone());
                stack.push((child_rel, child_fd));
            } else if st.is_file {
                files.push(ScannedFile {
                    rel: manifest_rel,
                    root_rel: child_rel,
                    size: st.size,
                    mtime_ms: st.mtime_ms,
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

/// No platform identity here: sending compares size and time alone.
#[cfg(not(any(unix, windows)))]
fn scan_file_id(path: &Path, unreadable: &'static str) -> Result<u64, Error> {
    let _ = (path, unreadable);
    Ok(0)
}

#[cfg(not(any(unix, windows)))]
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
                // Still a folder on the shared disk. This path-based walk
                // follows a folder swapped for a link after listing; only
                // the handle-based walk above refuses one.
                let _ = roots.resolve_dir(id, &child_rel)?;
                dirs.push(manifest_rel.clone());
                stack.push((full, child_rel));
            } else if meta.is_file() {
                let size = meta.len();
                let ino = scan_file_id(&full, "the source folder cannot be read")?;
                files.push(ScannedFile {
                    rel: manifest_rel,
                    root_rel: child_rel,
                    size,
                    mtime_ms: mtime_ms(&meta),
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
    #[cfg(any(unix, windows))]
    {
        use vnilink_rename::anchored::AsDir;
        let parent_fd = roots
            .open_dir_fd(id, parent_rel)
            .map_err(|_| Error::InvalidSetting(NOT_A_SCAN_DIR))?;
        let st = vnilink_rename::anchored::stat_at(parent_fd.as_dir(), name)
            .map_err(|_| Error::InvalidSetting(NOT_A_SCAN_DIR))?;
        if !st.is_file || st.is_symlink {
            return Err(Error::InvalidSetting(NOT_A_SCAN_DIR));
        }
        Ok(Scan {
            files: vec![ScannedFile {
                rel: name.to_owned(),
                root_rel: rel.to_owned(),
                size: st.size,
                mtime_ms: st.mtime_ms,
                ino: st.ino,
            }],
            dirs: Vec::new(),
            skipped_symlinks: 0,
        })
    }
    #[cfg(not(any(unix, windows)))]
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
        let ino = scan_file_id(&full, NOT_A_SCAN_DIR)?;
        Ok(Scan {
            files: vec![ScannedFile {
                rel: name.to_owned(),
                root_rel: rel.to_owned(),
                size,
                mtime_ms: mtime_ms(&meta),
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

/// Builds a manifest from a scan.
pub(crate) fn skeleton(scan: &Scan) -> Manifest {
    let mut entries = Vec::with_capacity(scan.files.len() + scan.dirs.len());
    for dir in &scan.dirs {
        entries.push(ManifestEntry {
            path: dir.clone(),
            is_dir: true,
            size: 0,
            source_mtime_ms: 0,
        });
    }
    for file in &scan.files {
        entries.push(ManifestEntry {
            path: file.rel.clone(),
            is_dir: false,
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
    // The sizes are a peer's word: their sum must stay a byte count.
    let mut total = 0u64;
    let mut seen = HashSet::with_capacity(manifest.entries.len());
    let mut folded = HashSet::with_capacity(manifest.entries.len());
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
        // Folders carry no payload; a manifest of folders alone is allowed,
        // since empty folders are real content of a folder transfer.
        if entry.is_dir && entry.size != 0 {
            return Err(Error::InvalidSetting(MANIFEST_UNSAFE));
        }
        total = total
            .checked_add(entry.size)
            .ok_or(Error::InvalidSetting(MANIFEST_LIMIT))?;
    }
    Ok(())
}

fn check_manifest_path(path: &str) -> Result<(), Error> {
    if path.is_empty()
        || path.len() > crate::fs::MAX_REL_BYTES
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
                crate::ignored!(
                    crate::trace::FILES,
                    "a staged file left behind is swept with its job's staging",
                    std::fs::remove_file(staged)
                );
            } else {
                crate::ignored!(
                    crate::trace::FILES,
                    "a sibling copy left behind is swept with its job's staging",
                    std::fs::remove_file(&sibling)
                );
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
            // Missing: create level by level through handles.
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest;
    use crate::test_support::{FilesystemLocation, Scratch};
    use std::sync::atomic::AtomicU64;
    use std::time::Duration;

    /// A destination on another volume (a root on a second drive): the rename
    /// fails with CrossesDevices, so the bytes go through a synced sibling and a
    /// same-volume rename. Windows publishes this way; renames cannot cross drives.
    #[test]
    fn publication_across_volumes_copies_through_a_sibling() {
        use crate::manifest::{publish_file_with, Commits, PublishPolicy};
        use std::path::Path;
        let scratch = Scratch::tagged("cross-volume");
        let dest = scratch.join("dest");
        std::fs::create_dir_all(&dest).unwrap();
        let staged = scratch.join("staged.bin");
        let leftovers = || {
            std::fs::read_dir(&dest)
                .unwrap()
                .filter_map(|entry| entry.ok()?.file_name().into_string().ok())
                .filter(|name| name.starts_with(".vnlpub."))
                .count()
        };
        let crossing = |rename: fn(&Path, &Path) -> std::io::Result<()>| {
            let staged = staged.clone();
            move |from: &Path, to: &Path| {
                if from == staged {
                    Err(std::io::Error::from(std::io::ErrorKind::CrossesDevices))
                } else {
                    rename(from, to)
                }
            }
        };
        let replace = crossing(|from, to| std::fs::rename(from, to));
        let no_replace = crossing(|from, to| vnilink_rename::rename_noreplace(from, to));
        let commits = Commits {
            replace: &replace,
            no_replace: &no_replace,
        };

        std::fs::write(&staged, b"first").unwrap();
        let landed =
            publish_file_with(&dest, "a.bin", &staged, PublishPolicy::Replace, &commits).unwrap();
        assert_eq!(landed, "a.bin");
        assert_eq!(std::fs::read(dest.join("a.bin")).unwrap(), b"first");
        assert!(!staged.exists(), "the staged copy outlived publication");

        std::fs::write(&staged, b"second").unwrap();
        let kept =
            publish_file_with(&dest, "a.bin", &staged, PublishPolicy::KeepBoth, &commits).unwrap();
        assert_ne!(kept, "a.bin");
        assert_eq!(std::fs::read(dest.join(&kept)).unwrap(), b"second");
        assert_eq!(std::fs::read(dest.join("a.bin")).unwrap(), b"first");
        assert_eq!(leftovers(), 0, "a sibling was left behind");

        // A failed final rename removes the sibling and keeps the staged bytes.
        std::fs::write(&staged, b"third").unwrap();
        let failing = |from: &Path, _: &Path| -> std::io::Result<()> {
            Err(std::io::Error::from(if from == staged.as_path() {
                std::io::ErrorKind::CrossesDevices
            } else {
                std::io::ErrorKind::PermissionDenied
            }))
        };
        let broken = Commits {
            replace: &failing,
            no_replace: &failing,
        };
        assert!(
            publish_file_with(&dest, "a.bin", &staged, PublishPolicy::Replace, &broken).is_err()
        );
        assert_eq!(std::fs::read(&staged).unwrap(), b"third");
        assert_eq!(leftovers(), 0, "a failed sibling was left behind");
        assert_eq!(std::fs::read(dest.join("a.bin")).unwrap(), b"first");
    }

    #[test]
    fn manifest_validation_rejects_bad_shapes() {
        use crate::manifest::{Manifest, ManifestEntry};

        let file = |path: &str| ManifestEntry {
            path: path.to_owned(),
            is_dir: false,
            size: 10,
            source_mtime_ms: 0,
        };
        let dir = |path: &str| ManifestEntry {
            path: path.to_owned(),
            is_dir: true,
            size: 0,
            source_mtime_ms: 0,
        };
        let valid = Manifest {
            entries: vec![dir("sub"), file("a.txt"), file("sub/b.txt")],
        };
        manifest::validate(&valid).unwrap();

        for entries in [
            vec![],
            vec![file("a.txt"), file("a.txt")],
            vec![file("A.txt"), file("a.txt")],
            vec![file("../out.txt")],
            vec![file("/abs.txt")],
            vec![file("a\\b.txt")],
            vec![file("a/./b.txt")],
            vec![dir("d"), file("d")],
            vec![dir("bad")]
                .into_iter()
                .map(|mut entry| {
                    entry.size = 1;
                    entry
                })
                .collect(),
        ] {
            assert!(
                manifest::validate(&Manifest { entries }).is_err(),
                "bad manifest shape must be rejected"
            );
        }
    }

    /// Crashes the `job_message` fuzz target: sizes summing past `u64::MAX`
    /// validated, then overflowed the job's total.
    #[test]
    fn manifest_validation_rejects_sizes_past_a_byte_count() {
        use crate::manifest::{Manifest, ManifestEntry};

        let file = |path: &str, size| ManifestEntry {
            path: path.to_owned(),
            is_dir: false,
            size,
            source_mtime_ms: 0,
        };
        let largest = Manifest {
            entries: vec![file("a", u64::MAX - 1), file("b", 1)],
        };
        manifest::validate(&largest).unwrap();
        assert_eq!(largest.total_bytes(), u64::MAX);
        let past = Manifest {
            entries: vec![file("a", u64::MAX), file("b", 1)],
        };
        assert!(manifest::validate(&past).is_err());
    }

    #[test]
    fn scan_accepts_single_files_as_well_as_trees() {
        let scratch = Scratch::tagged("scan-file");
        let root = scratch.join("root");
        std::fs::create_dir_all(root.join("sub")).unwrap();
        std::fs::write(root.join("sub/only.txt"), b"solo-bytes").unwrap();
        let mut roots = crate::fs::RootSet::default();
        roots.set_filesystem().unwrap();
        let files = FilesystemLocation::in_roots(&roots, &root);

        // A whole tree still scans.
        let tree = crate::manifest::scan(&roots, files.root, &files.path("")).unwrap();
        assert!(tree.files.iter().any(|f| f.rel == "sub/only.txt"));

        // A single file scans to a one-file manifest addressed by basename,
        // so selections of individual files transfer too.
        let single =
            crate::manifest::scan(&roots, files.root, &files.path("sub/only.txt")).unwrap();
        assert_eq!(single.files.len(), 1);
        assert_eq!(single.files[0].rel, "only.txt");
        assert_eq!(single.files[0].size, 10);
        assert!(single.dirs.is_empty());

        // A symlink to a file is refused, never followed.
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(root.join("sub/only.txt"), root.join("sub/link.txt"))
                .unwrap();
            assert!(
                crate::manifest::scan(&roots, files.root, &files.path("sub/link.txt")).is_err()
            );
        }
    }

    /// STD05: manifest budgets apply while enumerating. A directory past the
    /// file cap fails instead of allocating every entry first, and an exactly
    /// full directory still scans.
    #[test]
    fn scan_refuses_directories_past_the_manifest_budget() {
        let scratch = Scratch::tagged("scan-budget");
        let root = scratch.join("root");
        let big = root.join("big");
        let exact = root.join("exact");
        std::fs::create_dir_all(&big).unwrap();
        std::fs::create_dir_all(&exact).unwrap();
        for i in 0..5001 {
            std::fs::write(big.join(format!("entry-{i:06}")), []).unwrap();
        }
        for i in 0..5000 {
            std::fs::write(exact.join(format!("entry-{i:06}")), []).unwrap();
        }
        let mut roots = crate::fs::RootSet::default();
        roots.set_filesystem().unwrap();
        let files = FilesystemLocation::in_roots(&roots, &root);
        assert!(crate::manifest::scan(&roots, files.root, &files.path("big")).is_err());
        let fit = crate::manifest::scan(&roots, files.root, &files.path("exact")).unwrap();
        assert_eq!(fit.files.len(), 5000);
        assert!(fit.dirs.is_empty());
    }

    static SCAN_PARKED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    static SCAN_MAX_UNIT: AtomicU64 = AtomicU64::new(0);
    static SCAN_FINISHED: std::sync::Mutex<Option<bool>> = std::sync::Mutex::new(None);
    static SCAN_FINISH_CV: std::sync::Condvar = std::sync::Condvar::new();

    fn scan_park_probe(unit: u64, cancelled: &std::sync::atomic::AtomicBool) {
        use std::sync::atomic::Ordering as Seq;
        SCAN_MAX_UNIT.fetch_max(unit, Seq::SeqCst);
        if SCAN_PARKED.swap(true, Seq::SeqCst) {
            return;
        }
        while !cancelled.load(Seq::Acquire) {
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn scan_finish_probe(cancelled: bool) {
        *SCAN_FINISHED.lock().unwrap() = Some(cancelled);
        SCAN_FINISH_CV.notify_all();
    }

    /// STD08: detached scanning yields the executor and holds no shared lock
    /// (a sibling scan completes while the first scan is parked), and
    /// dropping the future aborts the worker before anything is traversed.
    #[test]
    fn parked_scan_yields_sibling_and_cancels_before_traversal() {
        use std::sync::{Arc, Mutex};

        let scratch = Scratch::tagged("scan-park");
        let root = scratch.join("root");
        std::fs::create_dir_all(root.join("sub")).unwrap();
        std::fs::write(root.join("sub/a.txt"), b"parked-bytes").unwrap();
        let mut roots = crate::fs::RootSet::default();
        roots.set_filesystem().unwrap();
        let files = FilesystemLocation::in_roots(&roots, &root);
        // Stand-in for the shared file-roots lock: the parked scan must hold
        // no lock across its worker await, so the sibling locks, snapshots
        // and completes while the first scan stays parked.
        let shared = Arc::new(Mutex::new(roots));
        let first = shared.lock().unwrap().clone();

        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let hooks = crate::manifest::ScanHooks {
            dir_probe: Some(scan_park_probe),
            finish_probe: Some(scan_finish_probe),
        };
        let (handle_tx, handle_rx) = std::sync::mpsc::channel();
        let (sibling_tx, sibling_rx) = std::sync::mpsc::channel();
        let sibling_shared = shared.clone();
        // A current-thread runtime only drives tasks under `block_on`; the
        // driver thread owns it while the test observes from outside.
        let driver = std::thread::spawn(move || {
            runtime.block_on(async move {
                let sibling_files = files.clone();
                let scan = tokio::spawn(crate::manifest::scan_roots_async(
                    first,
                    files.root,
                    files.path(""),
                    hooks,
                ));
                handle_tx.send(scan).unwrap();
                let cap = std::time::Instant::now() + Duration::from_secs(30);
                while !SCAN_PARKED.load(Ordering::SeqCst) {
                    if std::time::Instant::now() > cap {
                        return;
                    }
                    tokio::task::yield_now().await;
                }
                // Spawned only after the park is observed: it must lock the
                // shared roots, snapshot and finish while parked.
                tokio::spawn(async move {
                    let snapshot = sibling_shared.lock().unwrap().clone();
                    let page = crate::manifest::scan_roots_async(
                        snapshot,
                        sibling_files.root,
                        sibling_files.path(""),
                        crate::manifest::ScanHooks::none(),
                    )
                    .await;
                    let _ = sibling_tx.send(page.map(|scan| scan.files.len()));
                });
                while SCAN_FINISHED.lock().unwrap().is_none() {
                    if std::time::Instant::now() > cap {
                        return;
                    }
                    tokio::task::yield_now().await;
                }
            });
        });
        // The worker must reach the parked probe; on the old inline path no
        // probe exists and this times out.
        let deadline = std::time::Instant::now() + Duration::from_secs(8);
        while !SCAN_PARKED.load(Ordering::SeqCst) {
            assert!(
                std::time::Instant::now() < deadline,
                "scan worker never parked"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        // Sibling progress while parked: the async worker is free and no
        // shared lock is held, so a full second scan completes.
        let sibling = sibling_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("sibling blocked during parked scan");
        assert_eq!(
            sibling.expect("sibling scan failed"),
            1,
            "sibling scan result changed while parked"
        );
        // Cancellation aborts the worker without traversing anything: only
        // the pre-access probe fired.
        let scan = handle_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        scan.abort();
        let finished = SCAN_FINISHED.lock().unwrap();
        let (finished, timeout) = SCAN_FINISH_CV
            .wait_timeout_while(finished, Duration::from_secs(5), |done| done.is_none())
            .unwrap();
        assert!(
            !timeout.timed_out(),
            "scan worker never finished after abort"
        );
        assert_eq!(
            *finished,
            Some(true),
            "scan worker did not report cancellation"
        );
        drop(finished);
        assert_eq!(
            SCAN_MAX_UNIT.load(Ordering::SeqCst),
            0,
            "parked scan traversed directories"
        );
        driver.join().expect("driver thread panicked");
    }
}
