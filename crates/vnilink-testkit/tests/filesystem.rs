//! The file system of one engine, as its own windows reach it: the whole
//! disk it shares, listings, and changes made in place —
//! folders made, entries renamed, deleted or moved to the Trash, and
//! folders watched. Every case asserts what is on disk after it: bytes,
//! identities, exact listings and originals kept.

#[path = "filesystem/changes.rs"]
mod changes;
#[path = "filesystem/listing.rs"]
mod listing;
#[path = "filesystem/positional.rs"]
mod positional;
#[path = "filesystem/roots.rs"]
mod roots;
#[path = "filesystem/watching.rs"]
mod watching;

use std::path::{Path, PathBuf};

use vnilink::Error;
use vnilink_testkit::{wait_for, DEFAULT_WAIT};

/// What a rename or delete says of an entry replaced since it was listed.
const STALE_ENTRY: &str = "the entry changed since it was listed";
/// What a delete says when the entry could not be deleted.
const DELETE_FAILED: &str = "the entry could not be deleted";
/// What a path the engine will not walk is refused as.
const UNSAFE_PATH: &str = "the path is not a safe relative path";

fn is_message(error: &Error, expected: &str) -> bool {
    matches!(error, Error::InvalidSetting(message) if *message == expected)
}

fn is_stale(error: &Error) -> bool {
    is_message(error, STALE_ENTRY)
}

fn read(path: &Path) -> Vec<u8> {
    std::fs::read(path).unwrap_or_else(|error| panic!("read {}: {error}", path.display()))
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

/// A tree of folders `levels` deep below `top`, each named `level-<n>`.
fn deep_folder(top: &Path, levels: usize) -> PathBuf {
    let mut deep = top.to_path_buf();
    for level in 0..levels {
        deep.push(format!("level-{level}"));
    }
    std::fs::create_dir_all(&deep).unwrap();
    deep
}
