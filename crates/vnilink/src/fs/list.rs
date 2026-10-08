//! Paged, sorted directory listings.
//!
//! The whole directory is read and sorted by name bytes so pages are stable
//! while the caller holds its navigation revision. Directories larger than
//! [`MAX_LISTED`] set `truncated` instead of growing the response without
//! bound. Names that are not valid UTF-8 are skipped and counted: the name
//! API is UTF-8, so such entries are unreachable until a byte-path policy
//! exists.

use std::fmt;

use crate::error::Error;

#[cfg(not(unix))]
use super::entry::stat;
use super::entry::FsEntry;
use super::roots::RootSet;

/// Largest page a caller may ask for.
pub const MAX_PAGE: u16 = 500;
/// Largest directory prefix ever listed in one call.
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

pub(crate) fn list_page(
    roots: &RootSet,
    id: u64,
    rel: &str,
    offset: u64,
    limit: u16,
) -> Result<DirPage, Error> {
    if limit == 0 || limit > MAX_PAGE {
        return Err(Error::InvalidSetting("the page limit is out of range"));
    }
    #[cfg(unix)]
    {
        use std::os::unix::io::AsFd;
        let dir_fd = roots.open_dir_fd(id, rel)?;
        let raw = vnilink_rename::anchored::list_names(dir_fd.as_fd())
            .map_err(|_| Error::InvalidSetting("the folder cannot be listed"))?;
        let mut names: Vec<String> = Vec::new();
        let mut non_utf8_skipped = 0u64;
        for os in raw {
            match os.into_string() {
                Ok(name) => names.push(name),
                Err(_) => non_utf8_skipped += 1,
            }
            if names.len() >= MAX_LISTED {
                break;
            }
        }
        names.sort();
        // Truncation is detected by a second anchored read when the prefix
        // is exactly full: more entries than MAX_LISTED means truncated.
        let truncated = names.len() >= MAX_LISTED && {
            match vnilink_rename::anchored::list_names(dir_fd.as_fd()) {
                Ok(all) => all.len() > MAX_LISTED,
                Err(_) => false,
            }
        };
        let total = names.len() as u64;
        let start = (offset.min(total)) as usize;
        let end = start.saturating_add(limit as usize).min(names.len());
        let mut entries = Vec::with_capacity(end.saturating_sub(start));
        for name in &names[start..end] {
            entries.push(stat_anchored(&dir_fd, name));
        }
        let next_offset = if (end as u64) < total {
            Some(end as u64)
        } else {
            None
        };
        Ok(DirPage {
            entries,
            total,
            next_offset,
            truncated,
            non_utf8_skipped,
        })
    }
    #[cfg(not(unix))]
    {
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
        for name in &names[start..end] {
            entries.push(stat(&dir.join(name), name.clone()));
        }
        let next_offset = if (end as u64) < total {
            Some(end as u64)
        } else {
            None
        };
        Ok(DirPage {
            entries,
            total,
            next_offset,
            truncated,
            non_utf8_skipped,
        })
    }
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
