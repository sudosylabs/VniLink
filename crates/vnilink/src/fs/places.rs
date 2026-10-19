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
}

impl FolderKind {
    pub(crate) fn code(self) -> u8 {
        match self {
            Self::Home => 0,
            Self::Desktop => 1,
            Self::Documents => 2,
            Self::Downloads => 3,
            Self::Pictures => 4,
        }
    }

    pub(crate) fn from_code(code: u8) -> Option<Self> {
        match code {
            0 => Some(Self::Home),
            1 => Some(Self::Desktop),
            2 => Some(Self::Documents),
            3 => Some(Self::Downloads),
            4 => Some(Self::Pictures),
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

/// The roots, then the folders that exist, home first.
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
    let Some(home) = home_dir() else {
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
    places
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
