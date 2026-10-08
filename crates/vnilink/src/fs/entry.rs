//! Directory entries with filesystem identity.
//!
//! The token carries the object's device and inode on Unix so a rename can
//! prove it still names the listed object instead of a replacement. Names
//! never appear in `Debug`: listings are sensitive the same way transfer
//! names are.

use std::fmt;

/// What the entry is. Links are reported, never followed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryKind {
    File,
    Dir,
    Symlink,
    Other,
}

/// Filesystem identity for stale-mutation protection. `None` fields mean
/// the platform could not report an identity; the caller must treat the
/// token as weak (see the architecture note).
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct EntryToken {
    pub dev: Option<u64>,
    pub ino: Option<u64>,
}

impl EntryToken {
    /// A token that matches nothing: forces the mutation to re-check.
    pub const NONE: Self = Self {
        dev: None,
        ino: None,
    };

    /// True when both sides carry a real identity and agree.
    pub fn same_object(self, other: Self) -> bool {
        match (self.dev, self.ino, other.dev, other.ino) {
            (Some(a_dev), Some(a_ino), Some(b_dev), Some(b_ino)) => {
                a_dev == b_dev && a_ino == b_ino
            }
            _ => false,
        }
    }

    /// True when this token can prove identity at all.
    pub fn is_strong(self) -> bool {
        self.dev.is_some() && self.ino.is_some()
    }
}

impl fmt::Debug for EntryToken {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("EntryToken")
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
                token: identity(&meta),
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

#[cfg(unix)]
fn identity(meta: &std::fs::Metadata) -> EntryToken {
    use std::os::unix::fs::MetadataExt;
    EntryToken {
        dev: Some(meta.dev()),
        ino: Some(meta.ino()),
    }
}

#[cfg(not(unix))]
fn identity(_meta: &std::fs::Metadata) -> EntryToken {
    EntryToken::NONE
}
