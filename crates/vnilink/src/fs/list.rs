//! Paged, sorted directory listings.
//!
//! The whole directory is read and sorted by name bytes so pages are stable
//! while the caller holds its navigation revision. Directories larger than
//! [`MAX_LISTED`] set `truncated` instead of growing the response without
//! bound. Names that are not valid UTF-8 are skipped and counted: the name
//! API is UTF-8, so such entries are unreachable until a byte-path policy
//! exists.

use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::error::Error;

#[cfg(not(unix))]
use super::entry::stat;
use super::entry::FsEntry;
use super::roots::RootSet;

/// Largest page a caller may ask for.
pub const MAX_PAGE: u16 = 500;
/// Largest directory prefix ever listed in one call. Each page reads the
/// directory again, so this bounds the work per page, not just the reply.
/// Tests exercise the same bound with a smaller budget.
#[cfg(not(test))]
pub const MAX_LISTED: usize = 20_000;
#[cfg(test)]
pub const MAX_LISTED: usize = 5000;

/// One page of a listing. `Debug` redacts names.
#[derive(Clone, PartialEq, Eq)]
pub struct DirPage {
    pub entries: Vec<FsEntry>,
    pub total: u64,
    pub next_offset: Option<u64>,
    pub truncated: bool,
    pub non_utf8_skipped: u64,
}

impl fmt::Debug for DirPage {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DirPage")
            .field("len", &self.entries.len())
            .field("total", &self.total)
            .field("next_offset", &self.next_offset)
            .field("truncated", &self.truncated)
            .finish()
    }
}

/// Per-entry test probes for detached listing. Passed by the caller
/// (unit tests) rather than installed globally, so concurrent suites never
/// observe them. Production passes [`ListHooks::none`].
#[derive(Clone, Copy, Default)]
pub(crate) struct ListHooks {
    /// Called with index 0 before any filesystem access, then once per
    /// stated page entry with 1-based indices. A probe that parks
    /// (controlled slow list) must also watch `cancelled` and return
    /// promptly once it is set.
    pub(crate) entry_probe: Option<fn(u64, &AtomicBool)>,
    /// Called once when the worker finishes; `true` reports cancellation.
    pub(crate) finish_probe: Option<fn(bool)>,
}

impl ListHooks {
    pub(crate) fn none() -> Self {
        Self::default()
    }
}

/// Dropping the list future cancels its detached blocking worker between
/// entries; never rely on dropping the JoinHandle alone.
struct ListGuard(std::sync::Arc<AtomicBool>);

impl Drop for ListGuard {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
    }
}

pub(crate) fn list_page(
    roots: &RootSet,
    id: u64,
    rel: &str,
    offset: u64,
    limit: u16,
) -> Result<DirPage, Error> {
    let never = AtomicBool::new(false);
    list_page_with_hooks(roots, id, rel, offset, limit, &never, ListHooks::none())
}

/// Cancellable listing over an already-snapshotted root set. Holds no
/// locks; callers snapshot under a short lock, then await off every lock.
pub(crate) fn list_page_with_hooks(
    roots: &RootSet,
    id: u64,
    rel: &str,
    offset: u64,
    limit: u16,
    cancelled: &AtomicBool,
    hooks: ListHooks,
) -> Result<DirPage, Error> {
    if limit == 0 || limit > MAX_PAGE {
        return Err(Error::InvalidSetting("the page limit is out of range"));
    }
    if list_probe(0, cancelled, hooks) {
        if let Some(finish) = hooks.finish_probe {
            finish(true);
        }
        return Err(Error::NoSession);
    }
    #[cfg(unix)]
    {
        list_page_anchored(roots, id, rel, offset, limit, cancelled, hooks)
    }
    #[cfg(not(unix))]
    {
        list_page_pathname(roots, id, rel, offset, limit, cancelled, hooks)
    }
}

/// Lists one page from a snapshotted root set on a bounded blocking
/// worker (the Tokio blocking pool), checking for cancellation between
/// entries. Yields the async worker while listing; dropping the future
/// aborts the detached worker promptly instead of orphaning it. Callers
/// must hold no locks across the await and must recheck owner/grant/
/// liveness before using the page.
pub(crate) async fn list_page_async(
    roots: RootSet,
    id: u64,
    rel: String,
    offset: u64,
    limit: u16,
    hooks: ListHooks,
) -> Result<DirPage, Error> {
    let cancelled = std::sync::Arc::new(AtomicBool::new(false));
    let _guard = ListGuard(cancelled.clone());
    let worker_cancelled = cancelled.clone();
    tokio::task::spawn_blocking(move || {
        list_page_with_hooks(&roots, id, &rel, offset, limit, &worker_cancelled, hooks)
    })
    .await
    .map_err(|_| Error::NoSession)?
}

/// Runs the entry probe for one unit of listing. Returns true when the
/// listing must stop.
fn list_probe(unit: u64, cancelled: &AtomicBool, hooks: ListHooks) -> bool {
    if cancelled.load(Ordering::Acquire) {
        return true;
    }
    if let Some(probe) = hooks.entry_probe {
        probe(unit, cancelled);
        // The probe may have waited out a cancellation.
        if cancelled.load(Ordering::Acquire) {
            return true;
        }
    }
    false
}

#[cfg(unix)]
fn list_page_anchored(
    roots: &RootSet,
    id: u64,
    rel: &str,
    offset: u64,
    limit: u16,
    cancelled: &AtomicBool,
    hooks: ListHooks,
) -> Result<DirPage, Error> {
    use std::os::unix::io::AsFd;
    let aborted = || -> Result<DirPage, Error> {
        if let Some(finish) = hooks.finish_probe {
            finish(true);
        }
        Err(Error::NoSession)
    };
    let dir_fd = roots.open_dir_fd(id, rel)?;
    // Single bounded enumeration: at most MAX_LISTED + 1 raw entries are
    // ever allocated, and the truncation state comes from the same pass
    // (a second scan of the handle would see an exhausted offset, not
    // the directory).
    let (raw, raw_truncated) =
        vnilink_rename::anchored::list_names_bounded(dir_fd.as_fd(), MAX_LISTED + 1)
            .map_err(|_| Error::InvalidSetting("the folder cannot be listed"))?;
    let mut names: Vec<String> = Vec::new();
    let mut non_utf8_skipped = 0u64;
    let mut utf8_overflow = false;
    for os in raw {
        match os.into_string() {
            Ok(name) => {
                if names.len() >= MAX_LISTED {
                    utf8_overflow = true;
                    break;
                }
                names.push(name);
            }
            Err(_) => non_utf8_skipped += 1,
        }
    }
    names.sort();
    let truncated = raw_truncated || utf8_overflow;
    let total = names.len() as u64;
    let start = (offset.min(total)) as usize;
    let end = start.saturating_add(limit as usize).min(names.len());
    let mut entries = Vec::with_capacity(end.saturating_sub(start));
    // Probe indices continue the listing sequence: 0 fired before any access.
    for (position, name) in names[start..end].iter().enumerate() {
        if list_probe(position as u64 + 1, cancelled, hooks) {
            return aborted();
        }
        entries.push(stat_anchored(&dir_fd, name));
    }
    let next_offset = if (end as u64) < total {
        Some(end as u64)
    } else {
        None
    };
    if let Some(finish) = hooks.finish_probe {
        finish(false);
    }
    Ok(DirPage {
        entries,
        total,
        next_offset,
        truncated,
        non_utf8_skipped,
    })
}

#[cfg(not(unix))]
fn list_page_pathname(
    roots: &RootSet,
    id: u64,
    rel: &str,
    offset: u64,
    limit: u16,
    cancelled: &AtomicBool,
    hooks: ListHooks,
) -> Result<DirPage, Error> {
    let aborted = || -> Result<DirPage, Error> {
        if let Some(finish) = hooks.finish_probe {
            finish(true);
        }
        Err(Error::NoSession)
    };
    let dir = roots.resolve_dir(id, rel)?;
    let read = std::fs::read_dir(&dir)
        .map_err(|_| Error::InvalidSetting("the folder cannot be listed"))?;
    let mut names: Vec<String> = Vec::new();
    let mut non_utf8_skipped = 0u64;
    for entry in read {
        let entry = entry.map_err(|_| Error::InvalidSetting("the folder cannot be listed"))?;
        match entry.file_name().into_string() {
            Ok(name) => names.push(name),
            Err(_) => non_utf8_skipped += 1,
        }
        if names.len() >= MAX_LISTED {
            break;
        }
    }
    names.sort();
    let truncated = names.len() >= MAX_LISTED && dir_has_more(&dir, MAX_LISTED);
    let total = names.len() as u64;
    let start = (offset.min(total)) as usize;
    let end = start.saturating_add(limit as usize).min(names.len());
    let mut entries = Vec::with_capacity(end.saturating_sub(start));
    // Probe indices continue the listing sequence: 0 fired before any access.
    for (position, name) in names[start..end].iter().enumerate() {
        if list_probe(position as u64 + 1, cancelled, hooks) {
            return aborted();
        }
        entries.push(stat(&dir.join(name), name.clone()));
    }
    let next_offset = if (end as u64) < total {
        Some(end as u64)
    } else {
        None
    };
    if let Some(finish) = hooks.finish_probe {
        finish(false);
    }
    Ok(DirPage {
        entries,
        total,
        next_offset,
        truncated,
        non_utf8_skipped,
    })
}

#[cfg(unix)]
fn stat_anchored(dir_fd: &std::os::unix::io::OwnedFd, name: &str) -> FsEntry {
    use std::os::unix::io::AsFd;
    match vnilink_rename::anchored::stat_at(dir_fd.as_fd(), name) {
        Ok(st) => {
            let kind = if st.is_symlink {
                super::entry::EntryKind::Symlink
            } else if st.is_dir {
                super::entry::EntryKind::Dir
            } else if st.is_file {
                super::entry::EntryKind::File
            } else {
                super::entry::EntryKind::Other
            };
            FsEntry {
                name: name.to_owned(),
                kind,
                size: if kind == super::entry::EntryKind::File {
                    st.size
                } else {
                    0
                },
                mtime_ms: st.mtime_ms,
                token: super::entry::EntryToken {
                    dev: Some(st.dev),
                    ino: Some(st.ino),
                },
            }
        }
        Err(_) => FsEntry {
            name: name.to_owned(),
            kind: super::entry::EntryKind::Other,
            size: 0,
            mtime_ms: 0,
            token: super::entry::EntryToken::NONE,
        },
    }
}

#[cfg(not(unix))]
fn dir_has_more(dir: &std::path::Path, at_least: usize) -> bool {
    match std::fs::read_dir(dir) {
        Ok(read) => read.count() > at_least,
        Err(_) => false,
    }
}
