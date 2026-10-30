// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! Where a file manager starts: the shared roots, this account's home
//! folder and the usual folders inside it, each named by root and relative
//! path so the other computer can open them without guessing its layout.

use std::path::{Path, PathBuf};

use super::roots::{drive_letter, RootSet};

/// A well-known folder of this account.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FolderKind {
    Home,
    Desktop,
    Documents,
    Downloads,
    Pictures,
    /// A drive mounted under the whole disk, such as a USB stick in
    /// `/Volumes` or `/media`; its name is its folder's.
    Volume,
}

impl FolderKind {
    pub(crate) fn code(self) -> u8 {
        match self {
            Self::Home => 0,
            Self::Desktop => 1,
            Self::Documents => 2,
            Self::Downloads => 3,
            Self::Pictures => 4,
            Self::Volume => 5,
        }
    }

    pub(crate) fn from_code(code: u8) -> Option<Self> {
        match code {
            0 => Some(Self::Home),
            1 => Some(Self::Desktop),
            2 => Some(Self::Documents),
            3 => Some(Self::Downloads),
            4 => Some(Self::Pictures),
            5 => Some(Self::Volume),
            _ => None,
        }
    }
}

/// One shared root: its id and its own name, `/` or a drive such as `C:`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlaceRoot {
    pub id: u64,
    pub name: String,
}

/// A well-known folder, under one root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Place {
    pub kind: FolderKind,
    pub root: u64,
    pub path: String,
}

/// The roots, then the folders that exist, home first, then the drives
/// mounted under the whole disk.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Places {
    pub roots: Vec<PlaceRoot>,
    pub folders: Vec<Place>,
}

/// The places on this computer under `roots`. A folder outside every root,
/// missing, or not a directory is left out; with no home folder there are
/// only roots.
pub(crate) fn places(roots: &RootSet) -> Places {
    let mut places = Places {
        roots: roots
            .snapshot()
            .into_iter()
            .map(|(id, path)| PlaceRoot {
                id,
                name: root_name(&path),
            })
            .collect(),
        folders: Vec::new(),
    };
    let volumes = mounted_volumes()
        .into_iter()
        .filter_map(|path| roots.locate(&path))
        .map(|(root, path)| Place {
            kind: FolderKind::Volume,
            root,
            path,
        });
    let Some(home) = home_dir() else {
        places.folders.extend(volumes);
        return places;
    };
    let candidates = [
        (FolderKind::Home, home.clone()),
        (FolderKind::Desktop, home.join("Desktop")),
        (FolderKind::Documents, home.join("Documents")),
        (FolderKind::Downloads, home.join("Downloads")),
        (FolderKind::Pictures, home.join("Pictures")),
    ];
    for (kind, path) in candidates {
        if !path.is_dir() {
            continue;
        }
        if let Some((root, path)) = roots.locate(&path) {
            places.folders.push(Place { kind, root, path });
        }
    }
    places.folders.extend(volumes);
    places
}

/// At most this many drives are offered; more are still reachable by path.
const MAX_VOLUMES: usize = 32;

/// Drives mounted where macOS and Linux put them, read afresh each time so
/// one plugged in later appears. A volume is a directory mounted over its
/// parent, so it sits on another device. As in Finder, the link macOS
/// keeps in `/Volumes` to the startup disk is left out, and so are system
/// volumes such as Recovery that are mounted not to be browsed.
#[cfg(unix)]
fn mounted_volumes() -> Vec<PathBuf> {
    use std::os::unix::fs::MetadataExt;
    let mut bases = vec![
        PathBuf::from("/Volumes"),
        PathBuf::from("/mnt"),
        PathBuf::from("/media"),
    ];
    if let Some(user) = nix::unistd::User::from_uid(nix::unistd::Uid::effective())
        .ok()
        .flatten()
    {
        bases.push(Path::new("/media").join(&user.name));
        bases.push(Path::new("/run/media").join(&user.name));
    }
    let mut volumes = Vec::new();
    for base in bases {
        let (Ok(parent), Ok(entries)) =
            (std::fs::symlink_metadata(&base), std::fs::read_dir(&base))
        else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(meta) = std::fs::symlink_metadata(&path) else {
                continue;
            };
            if meta.is_dir()
                && !meta.file_type().is_symlink()
                && meta.dev() != parent.dev()
                && vnilink_rename::anchored::browsable_volume(&path)
                && !volumes.contains(&path)
            {
                volumes.push(path);
            }
        }
    }
    volumes.sort_by_key(|path| {
        path.file_name()
            .map(|name| name.to_string_lossy().to_lowercase())
    });
    volumes.truncate(MAX_VOLUMES);
    volumes
}

/// Windows drives are roots of their own, read again with each Places.
#[cfg(not(unix))]
fn mounted_volumes() -> Vec<PathBuf> {
    Vec::new()
}

/// A drive's letter and colon, or the root's own path (`/`).
fn root_name(path: &Path) -> String {
    match drive_letter(path) {
        Some(letter) => format!("{}:", char::from(letter)),
        None => path.to_string_lossy().into_owned(),
    }
}

#[cfg(unix)]
fn home_dir() -> Option<PathBuf> {
    nix::unistd::User::from_uid(nix::unistd::Uid::effective())
        .ok()
        .flatten()
        .map(|user| user.dir)
        .or_else(|| std::env::var_os("HOME").map(PathBuf::from))
        .filter(|path| path.is_absolute())
}

#[cfg(windows)]
fn home_dir() -> Option<PathBuf> {
    std::env::var_os("USERPROFILE")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
}

#[cfg(not(any(unix, windows)))]
fn home_dir() -> Option<PathBuf> {
    None
}
