//! Watching a folder for changes made outside VniLink, such as a file
//! deleted in Finder or Explorer, so the pane showing it lists it again.
//!
//! This computer's folders are watched for the window, which hears of a
//! change as a [`CoreEvent::FolderChanged`]. The other computer's folder is
//! waited on: the controller asks, on a stream of its own, to be answered
//! once the folder's fingerprint differs from the last one it saw. Each
//! wait compares with the last answer, so no change between waits is lost.

use super::*;
use notify::{EventKind, RecursiveMode, Watcher};

/// Changes arriving together, such as a copy of many files, settle into one.
const SETTLE: Duration = Duration::from_millis(200);
/// The longest a wait is held before it is answered unchanged.
pub(super) const WAIT: Duration = Duration::from_secs(25);
/// At most this many local folders are watched at once. A window that
/// reloads never says it stopped watching, so past this the oldest goes.
const MAX_WATCHES: usize = 16;
const NO_WATCH: &str = "the folder can't be watched";

/// Watches `folder` itself, not its subfolders. `changed` runs on the
/// watcher's own thread for each change to one of its entries.
fn watch_folder(
    folder: &std::path::Path,
    changed: impl Fn() + Send + 'static,
) -> Result<notify::RecommendedWatcher, Error> {
    let mut watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
        if event.is_ok_and(|event| !matches!(event.kind, EventKind::Access(_))) {
            changed();
        }
    })
    .map_err(|_| Error::Unsupported(NO_WATCH))?;
    watcher
        .watch(folder, RecursiveMode::NonRecursive)
        .map_err(|_| Error::Unsupported(NO_WATCH))?;
    Ok(watcher)
}

/// A digest of what a pane shows of a folder: each entry's name, kind,
/// size and modified time. It changes when an entry comes, goes, is
/// renamed or is written to.
pub(super) fn fingerprint(roots: &crate::fs::RootSet, root: u64, rel: &str) -> Result<u64, Error> {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    let mut offset = 0;
    loop {
        let page = crate::fs::list_page(roots, root, rel, offset, crate::fs::MAX_PAGE)?;
        for entry in &page.entries {
            (&entry.name, entry.kind as u8, entry.size, entry.mtime_ms).hash(&mut hasher);
        }
        match page.next_offset {
            Some(next) => offset = next,
            None => break,
        }
    }
    Ok(hasher.finish())
}

impl Core {
    /// Watches one folder on this computer; each change to its entries is
    /// published, settled, as [`CoreEvent::FolderChanged`] with the id this
    /// returns, until [`Core::unwatch_folder`].
    pub fn watch_folder(&self, root: u64, path: &str) -> Result<u64, Error> {
        self.ensure_alive()?;
        let folder = lock(&self.shared.file_roots).resolve_dir(root, path)?;
        let watch = self.shared.watch_next.fetch_add(1, Ordering::Relaxed);
        let shared = Arc::downgrade(&self.shared);
        let runtime = self.runtime.handle().clone();
        let pending = Arc::new(AtomicBool::new(false));
        let watcher = watch_folder(&folder, move || {
            if pending.swap(true, Ordering::AcqRel) {
                return;
            }
            let shared = shared.clone();
            let pending = pending.clone();
            runtime.spawn(async move {
                tokio::time::sleep(SETTLE).await;
                pending.store(false, Ordering::Release);
                if let Some(shared) = shared.upgrade() {
                    shared.events.publish(CoreEvent::FolderChanged { watch });
                }
            });
        })?;
        let mut watches = lock(&self.shared.folder_watches);
        while watches.len() >= MAX_WATCHES {
            let Some(oldest) = watches.keys().min().copied() else {
                break;
            };
            watches.remove(&oldest);
        }
        watches.insert(watch, watcher);
        Ok(watch)
    }

    /// Stops a watch from [`Core::watch_folder`]; an unknown id is ignored.
    pub fn unwatch_folder(&self, watch: u64) {
        lock(&self.shared.folder_watches).remove(&watch);
    }
}

/// The host's side of a wait: answers at once when the folder already
/// differs from `since`, else when it changes or after [`WAIT`], with the
/// fingerprint it has then. The watch starts before the first look, so a
/// change between the two is not missed.
pub(super) async fn answer_wait(
    roots: crate::fs::RootSet,
    root: u64,
    path: String,
    since: u64,
) -> Result<u64, Error> {
    let folder = roots.resolve_dir(root, &path)?;
    let changed = Arc::new(tokio::sync::Notify::new());
    let _watcher = {
        let changed = changed.clone();
        watch_folder(&folder, move || changed.notify_one())?
    };
    let look = |roots: crate::fs::RootSet, path: String| {
        tokio::task::spawn_blocking(move || fingerprint(&roots, root, &path))
    };
    let now = look(roots.clone(), path.clone())
        .await
        .map_err(|_| Error::Unsupported(NO_WATCH))??;
    if now != since {
        return Ok(now);
    }
    // A closed connection ends the stream, and with it this wait.
    tokio::select! {
        _ = changed.notified() => tokio::time::sleep(SETTLE).await,
        _ = tokio::time::sleep(WAIT) => {}
    }
    look(roots, path)
        .await
        .map_err(|_| Error::Unsupported(NO_WATCH))?
}
