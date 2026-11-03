// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

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

/// Every name in `dir`; a name that is not text shows as it can.
fn names_in(dir: &Path) -> Vec<String> {
    std::fs::read_dir(dir)
        .unwrap_or_else(|error| panic!("list {}: {error}", dir.display()))
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect()
}

/// Waits until the file system's clock has passed the tick `original` was
/// made in, so a file made again now cannot carry its times. ext4 and
/// overlayfs give a freed inode number to the next file at once; its
/// creation time is what tells the two apart, and on a kernel with coarse
/// inode timestamps a file made in the same tick would carry it too. A
/// probe made in `probes`, on the same file system, shows the tick.
fn past_the_tick_of(original: &Path, probes: &Path) -> std::io::Result<()> {
    past_the_tick(original, probes, std::fs::Metadata::modified)
}

/// As [`past_the_tick_of`], for the creation time: a volume that keeps it
/// coarser than it keeps the modification time (FAT's even second) gives a
/// file made again in that tick the original's.
fn past_the_creation_tick_of(original: &Path, probes: &Path) -> std::io::Result<()> {
    past_the_tick(original, probes, std::fs::Metadata::created)
}

/// Waits until a probe made in `probes` has a later `time` than `original`.
fn past_the_tick(
    original: &Path,
    probes: &Path,
    time: fn(&std::fs::Metadata) -> std::io::Result<std::time::SystemTime>,
) -> std::io::Result<()> {
    let made = time(&std::fs::metadata(original)?)?;
    let probe = probes.join("clock-probe");
    wait_for(
        "the file system's clock to pass the original's tick",
        DEFAULT_WAIT,
        || {
            // Made anew each time, so it has a creation time of its own.
            let _ = std::fs::remove_file(&probe);
            std::fs::write(&probe, b"tick").ok()?;
            let now = time(&std::fs::metadata(&probe).ok()?).ok()?;
            (now > made).then_some(())
        },
    );
    std::fs::remove_file(&probe)
}

/// A tree of folders `levels` deep below `top`, each named `level-<n>`.
fn deep_folder(top: &Path, levels: usize) -> std::io::Result<PathBuf> {
    let mut deep = top.to_path_buf();
    for level in 0..levels {
        deep.push(format!("level-{level}"));
    }
    std::fs::create_dir_all(&deep)?;
    Ok(deep)
}
