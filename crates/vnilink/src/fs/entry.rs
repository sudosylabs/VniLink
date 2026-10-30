// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! Directory entries with the token a change names them by.
//!
//! A token lets a rename, delete or move to the Trash prove it still acts
//! on the listed entry instead of a replacement. On most volumes it is the
//! object's identity: device, inode and creation time. A deleted file's
//! inode number is given to the next file made, at once on ext4 and
//! overlayfs; its creation time tells the two apart. On a volume whose ids
//! are entry positions (FAT, exFAT) an identity proves nothing, so the
//! token is a fingerprint of what the listing saw instead. Names never
//! appear in `Debug`: listings are sensitive the same way transfer names
//! are.

use std::fmt;

/// What the entry is. Links are reported, never followed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryKind {
    File,
    Dir,
    Symlink,
    Other,
}

/// What a token proves an entry by.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenKind {
    /// The object's identity: `dev`, `ino` and `born`.
    Identity,
    /// On a volume whose ids are entry positions: `dev` names the volume
    /// and `fingerprint` digests the entry's name, kind, size, times at the
    /// volume's granularity and attributes ([`fingerprint`]).
    Fingerprint,
}

/// What a change names a listed entry by. An identity token with no `dev`
/// or `ino` means the platform could not report one: the token is weak and
/// proves nothing. `born` is the creation time in nanoseconds since 1970,
/// `None` where the filesystem keeps none: there a recycled inode number is
/// not detected.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct EntryToken {
    pub kind: TokenKind,
    pub dev: Option<u64>,
    pub ino: Option<u64>,
    pub born: Option<u64>,
    pub fingerprint: Option<u64>,
}

impl EntryToken {
    /// A token that matches nothing: forces the mutation to re-check.
    pub const NONE: Self = Self {
        kind: TokenKind::Identity,
        dev: None,
        ino: None,
        born: None,
        fingerprint: None,
    };

    /// An identity token.
    pub const fn identity(dev: u64, ino: u64, born: Option<u64>) -> Self {
        Self {
            kind: TokenKind::Identity,
            dev: Some(dev),
            ino: Some(ino),
            born,
            fingerprint: None,
        }
    }

    /// A fingerprint token for an entry on the volume `volume`.
    pub const fn fingerprinted(volume: u64, fingerprint: u64) -> Self {
        Self {
            kind: TokenKind::Fingerprint,
            dev: Some(volume),
            ino: None,
            born: None,
            fingerprint: Some(fingerprint),
        }
    }

    /// True when both tokens are strong, of one kind, and agree: the same
    /// object, creation time included, or the same fingerprint on the same
    /// volume.
    pub fn same_object(self, other: Self) -> bool {
        if !self.is_strong() || !other.is_strong() || self.kind != other.kind {
            return false;
        }
        match self.kind {
            TokenKind::Identity => {
                self.dev == other.dev && self.ino == other.ino && self.born == other.born
            }
            TokenKind::Fingerprint => {
                self.dev == other.dev && self.fingerprint == other.fingerprint
            }
        }
    }

    /// True when this token can prove anything at all.
    pub fn is_strong(self) -> bool {
        match self.kind {
            TokenKind::Identity => self.dev.is_some() && self.ino.is_some(),
            TokenKind::Fingerprint => self.dev.is_some() && self.fingerprint.is_some(),
        }
    }
}

/// What a fingerprint covers of an entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Seen<'a> {
    pub(crate) name: &'a str,
    pub(crate) dir: bool,
    /// Zero for a folder, whose size follows what it holds.
    pub(crate) size: u64,
    pub(crate) mtime_ms: u64,
    /// Nanoseconds since 1970.
    pub(crate) born: Option<u64>,
    pub(crate) attributes: u32,
    pub(crate) volume: u64,
}

/// The digest a fingerprint token carries: FNV-1a over what was seen,
/// times cut to the volume's granularity (`mtime_ms`, `born_ms`), so two
/// readings of one unchanged entry agree and a change the volume records
/// does not.
pub(crate) fn fingerprint(seen: Seen<'_>, mtime_ms: u64, born_ms: u64) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    let mut eat = |bytes: &[u8]| {
        for byte in bytes {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
    };
    eat(&(seen.name.len() as u64).to_be_bytes());
    eat(seen.name.as_bytes());
    eat(&[u8::from(seen.dir)]);
    eat(&seen.size.to_be_bytes());
    eat(&(seen.mtime_ms / mtime_ms.max(1)).to_be_bytes());
    match seen.born {
        Some(born) => {
            eat(&[1]);
            eat(&(born / (born_ms.max(1) * 1_000_000)).to_be_bytes());
        }
        None => eat(&[0]),
    }
    eat(&seen.attributes.to_be_bytes());
    eat(&seen.volume.to_be_bytes());
    hash
}

/// The token a listing gives the entry `name` stated as `st` on a volume
/// that names its entries as `ids` does, and a change compares with.
#[cfg(any(unix, windows))]
pub(crate) fn token_of(
    name: &str,
    st: &vnilink_rename::anchored::EntryStat,
    ids: vnilink_rename::VolumeIds,
) -> EntryToken {
    match ids {
        vnilink_rename::VolumeIds::Stable => EntryToken::identity(st.dev, st.ino, st.born),
        vnilink_rename::VolumeIds::Positional { mtime_ms, born_ms } => {
            let seen = Seen {
                name,
                dir: st.is_dir,
                size: if st.is_dir { 0 } else { st.size },
                mtime_ms: st.mtime_ms,
                born: st.born,
                attributes: st.attributes,
                volume: st.dev,
            };
            EntryToken::fingerprinted(st.dev, fingerprint(seen, mtime_ms, born_ms))
        }
    }
}

impl fmt::Debug for EntryToken {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("EntryToken")
            .field("kind", &self.kind)
            .field("strong", &self.is_strong())
            .finish()
    }
}

/// One listed entry. `Debug` redacts the name.
#[derive(Clone, PartialEq, Eq)]
pub struct FsEntry {
    pub name: String,
    pub kind: EntryKind,
    pub size: u64,
    pub mtime_ms: u64,
    pub token: EntryToken,
}

impl fmt::Debug for FsEntry {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("FsEntry")
            .field("kind", &self.kind)
            .field("size", &self.size)
            .field("token", &self.token)
            .finish()
    }
}

#[allow(dead_code)]
pub(crate) fn stat(path: &std::path::Path, name: String) -> FsEntry {
    let meta = std::fs::symlink_metadata(path);
    match meta {
        Ok(meta) => {
            let file_type = meta.file_type();
            let kind = if file_type.is_symlink() {
                EntryKind::Symlink
            } else if file_type.is_dir() {
                EntryKind::Dir
            } else if file_type.is_file() {
                EntryKind::File
            } else {
                EntryKind::Other
            };
            FsEntry {
                name,
                kind,
                size: if kind == EntryKind::File {
                    meta.len()
                } else {
                    0
                },
                mtime_ms: mtime_ms(&meta),
                token: identity(path, &meta),
            }
        }
        Err(_) => FsEntry {
            name,
            kind: EntryKind::Other,
            size: 0,
            mtime_ms: 0,
            token: EntryToken::NONE,
        },
    }
}

fn mtime_ms(meta: &std::fs::Metadata) -> u64 {
    meta.modified()
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|age| age.as_millis().try_into().unwrap_or(u64::MAX))
        .unwrap_or(0)
}

/// When `meta`'s object was made, in nanoseconds since 1970: statx's
/// birth time on Linux, `st_birthtime` on macOS, the creation time on
/// Windows. `None` where the filesystem keeps none.
fn born(meta: &std::fs::Metadata) -> Option<u64> {
    let age = meta
        .created()
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?;
    age.as_nanos().try_into().ok()
}

#[cfg(unix)]
fn identity(_path: &std::path::Path, meta: &std::fs::Metadata) -> EntryToken {
    use std::os::unix::fs::MetadataExt;
    EntryToken::identity(meta.dev(), meta.ino(), born(meta))
}

#[cfg(windows)]
fn identity(path: &std::path::Path, meta: &std::fs::Metadata) -> EntryToken {
    match vnilink_rename::windows::entry_identity(path) {
        Ok((dev, ino)) => EntryToken::identity(dev, ino, born(meta)),
        Err(_) => EntryToken::NONE,
    }
}

#[cfg(not(any(unix, windows)))]
fn identity(_path: &std::path::Path, _meta: &std::fs::Metadata) -> EntryToken {
    EntryToken::NONE
}

#[cfg(test)]
mod fingerprint_tests {
    use super::*;

    const LISTED: Seen<'static> = Seen {
        name: "photo.jpg",
        dir: false,
        size: 4096,
        mtime_ms: 1_700_000_000_000,
        born: Some(1_699_000_000_120_000_000),
        attributes: 0x20,
        volume: 7,
    };

    /// FAT's granularities: modification to 2 s, creation to 10 ms.
    fn fat(seen: Seen<'_>) -> EntryToken {
        EntryToken::fingerprinted(seen.volume, fingerprint(seen, 2_000, 10))
    }

    #[test]
    fn an_unchanged_entry_matches_its_listing() {
        assert!(fat(LISTED).same_object(fat(LISTED)));
    }

    #[test]
    fn a_time_inside_the_volume_s_granularity_is_the_same_time() {
        let read_again = Seen {
            mtime_ms: LISTED.mtime_ms + 1_999,
            born: LISTED.born.map(|born| born + 9_000_000),
            ..LISTED
        };
        assert!(fat(LISTED).same_object(fat(read_again)));
    }

    #[test]
    fn a_change_the_volume_records_is_refused() {
        for changed in [
            Seen {
                size: 4097,
                ..LISTED
            },
            Seen {
                mtime_ms: LISTED.mtime_ms + 2_000,
                ..LISTED
            },
            Seen {
                attributes: 0x21,
                ..LISTED
            },
            Seen {
                name: "photo2.jpg",
                ..LISTED
            },
            Seen {
                volume: 8,
                ..LISTED
            },
        ] {
            assert!(!fat(LISTED).same_object(fat(changed)), "{changed:?}");
        }
    }

    #[test]
    fn another_file_made_under_the_name_is_refused() {
        let recreated = Seen {
            size: 2048,
            mtime_ms: LISTED.mtime_ms + 60_000,
            born: Some(1_699_000_060_000_000_000),
            ..LISTED
        };
        assert!(!fat(LISTED).same_object(fat(recreated)));
    }

    #[test]
    fn a_fingerprint_never_matches_an_identity() {
        let identity = EntryToken::identity(7, 4096, LISTED.born);
        assert!(!fat(LISTED).same_object(identity));
        assert!(!identity.same_object(fat(LISTED)));
    }
}
