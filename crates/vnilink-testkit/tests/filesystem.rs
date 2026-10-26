//! The file system of one engine, as its own windows reach it: the folders
//! it shares or the whole disk, listings, and changes made in place —
//! folders made, entries renamed, deleted or moved to the Trash, and
//! folders watched. Every case asserts what is on disk after it: bytes,
//! identities, exact listings and originals kept.

#[path = "filesystem/changes.rs"]
mod changes;
#[path = "filesystem/listing.rs"]
mod listing;
#[path = "filesystem/roots.rs"]
mod roots;
#[path = "filesystem/watching.rs"]
mod watching;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use vnilink::{
    Core, CoreEvent, EntryToken, Error, EventSubscription, MemoryStore, RelaySettings, MAX_PAGE,
};
use vnilink_testkit::{holds, wait_for, DEFAULT_WAIT};

/// What a rename or delete says of an entry replaced since it was listed.
const STALE_ENTRY: &str = "the entry changed since it was listed";
/// What a delete says when the entry could not be deleted.
const DELETE_FAILED: &str = "the entry could not be deleted";
/// What a path the engine will not walk is refused as.
const UNSAFE_PATH: &str = "the path is not a safe relative path";

/// An engine as it first opens, before anything is shared: unlike
/// [`vnilink_testkit::Engine`], it has no file root.
struct Unshared {
    core: Core,
}

impl Unshared {
    fn start(name: &str) -> Self {
        vnilink::trace::name_next_engine(name);
        let core = Core::open_direct(Arc::new(MemoryStore::empty()), RelaySettings::public_n0())
            .expect("a loopback engine");
        core.set_trace_name(name);
        Self { core }
    }
}

impl Drop for Unshared {
    fn drop(&mut self) {
        self.core.shutdown();
    }
}

/// A folder of the test's own, addressed as an engine with its whole file
/// system shared names it: a root and the path below it. Fixtures are
/// ordinary folders; only paths below the fixture are written.
#[derive(Clone)]
struct Location {
    root: u64,
    relative: String,
}

impl Location {
    /// Where `fixture` is among `core`'s roots.
    fn of(core: &Core, fixture: &Path) -> Self {
        let fixture = std::fs::canonicalize(fixture).expect("the fixture exists");
        let (root, relative) = core
            .file_roots()
            .expect("file roots")
            .into_iter()
            .find_map(|(id, path)| {
                fixture.strip_prefix(path).ok().map(|relative| {
                    let relative = relative
                        .components()
                        .map(|part| part.as_os_str().to_str().expect("a UTF-8 fixture path"))
                        .collect::<Vec<_>>()
                        .join("/");
                    (id, relative)
                })
            })
            .expect("the fixture is under one of the engine's roots");
        assert!(
            !relative.is_empty(),
            "writes stay inside a temporary fixture"
        );
        Self { root, relative }
    }

    /// `child` below the fixture, or the fixture itself for `""`.
    fn path(&self, child: &str) -> String {
        if child.is_empty() {
            self.relative.clone()
        } else {
            format!("{}/{child}", self.relative)
        }
    }
}

/// The id of `core`'s one file root.
fn only_root(core: &Core) -> u64 {
    let roots = core.file_roots().expect("file roots");
    assert_eq!(roots.len(), 1, "the engine shares one folder");
    roots[0].0
}

/// The identity `name` in `path` is listed with, as a pane holds it.
fn token_of(core: &Core, root: u64, path: &str, name: &str) -> EntryToken {
    let token = core
        .list_dir(root, path, 0, MAX_PAGE)
        .expect("a listing")
        .entries
        .into_iter()
        .find(|entry| entry.name == name)
        .unwrap_or_else(|| panic!("{name} is listed"))
        .token;
    assert!(
        token.is_strong(),
        "fixture entries must carry native identity"
    );
    token
}

fn is_message(error: &Error, expected: &str) -> bool {
    matches!(error, Error::InvalidSetting(message) if *message == expected)
}

fn is_stale(error: &Error) -> bool {
    is_message(error, STALE_ENTRY)
}

#[cfg(unix)]
fn ino_of(path: &Path) -> Option<u64> {
    use std::os::unix::fs::MetadataExt;
    std::fs::symlink_metadata(path).ok().map(|meta| meta.ino())
}

#[cfg(not(unix))]
fn ino_of(_path: &Path) -> Option<u64> {
    None
}

fn read(path: &Path) -> Vec<u8> {
    std::fs::read(path).unwrap_or_else(|error| panic!("read {}: {error}", path.display()))
}

/// The names in `dir` a delete sets aside while it works.
fn aside_names(dir: &Path) -> Vec<String> {
    std::fs::read_dir(dir)
        .unwrap()
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name.starts_with(".vnldelete."))
        .collect()
}

/// Every name in `dir`.
fn names_in(dir: &Path) -> Vec<String> {
    std::fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect()
}

/// Waits until the file system's clock has passed the tick `original` was
/// made in, so a file made again now cannot carry its times. ext4 and
/// overlayfs give a freed inode number to the next file at once; its
/// creation time is what tells the two apart, and on a kernel with coarse
/// inode timestamps a file made in the same tick would carry it too. A
/// probe made in `probes`, on the same file system, shows the tick.
fn past_the_tick_of(original: &Path, probes: &Path) {
    let made = std::fs::metadata(original)
        .and_then(|meta| meta.modified())
        .expect("the original's time");
    let probe = probes.join("clock-probe");
    wait_for(
        "the file system's clock to pass the original's tick",
        DEFAULT_WAIT,
        || {
            std::fs::write(&probe, b"tick").expect("a probe");
            let now = std::fs::metadata(&probe)
                .and_then(|meta| meta.modified())
                .expect("the probe's time");
            (now > made).then_some(())
        },
    );
    std::fs::remove_file(&probe).expect("the probe removed");
}

/// A watch may report its own start: waits until `events` has been quiet
/// for a while, and drops what came before.
fn settle(events: &EventSubscription) {
    wait_for("the watch to settle", DEFAULT_WAIT, || {
        events
            .recv_timeout(Duration::from_millis(300))
            .is_none()
            .then_some(())
    });
}

/// Whether `events` holds a change to `watch`, taking what it holds.
fn saw_change(events: &EventSubscription, watch: u64) -> bool {
    let mut seen = false;
    while let Some(event) = events.try_recv() {
        seen |= matches!(event, CoreEvent::FolderChanged { watch: changed } if changed == watch);
    }
    seen
}

/// Waits up to `within` for a change to `watch`, failing as `what`.
fn wait_for_change(events: &EventSubscription, watch: u64, what: &str, within: Duration) {
    wait_for(what, within, || saw_change(events, watch).then_some(()));
}

/// No change to `watch` arrives throughout `period`.
fn no_change(events: &EventSubscription, watch: u64, what: &str, period: Duration) {
    holds(what, period, || !saw_change(events, watch));
}

/// A tree of folders `levels` deep below `top`, each named `level-<n>`.
fn deep_folder(top: &Path, levels: usize) -> PathBuf {
    let mut deep = top.to_path_buf();
    for level in 0..levels {
        deep.push(format!("level-{level}"));
    }
    std::fs::create_dir_all(&deep).unwrap();
    deep
}
