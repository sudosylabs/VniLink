//! Clipboard file selections. Metadata only: names, sizes and directory
//! markers cross the clipboard stream so a paste can be offered without
//! downloading a single payload byte. Pasting an offered selection moves
//! its bytes through Files jobs, never through this stream.

use crate::error::Error;

/// Largest file selection offer: entries and encoded bytes. A selection is
/// an offer to paste, not a transfer; payloads never ride this path.
pub const MAX_CLIPBOARD_FILES: usize = 256;
pub const MAX_CLIPBOARD_FILES_BYTES: usize = 64 * 1024;

/// One selected file or directory. `path` is relative to the selection root
/// and follows the transfer names' path rules; `size` is an estimate the receiver must
/// not trust for allocation; directories always report size zero.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClipboardFileEntry {
    pub path: String,
    pub size: u64,
    pub dir: bool,
}

/// An offered selection. Empty entries clear the peer's selection, mirroring
/// empty text. Selections are replaced, not queued.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FileSelection {
    pub entries: Vec<ClipboardFileEntry>,
}

/// A stored selection with its session-scoped revision. The source assigns
/// a fresh revision per offer, so a paste pinned to an older revision is
/// stale.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoredFileSelection {
    pub revision: u64,
    pub selection: FileSelection,
}

impl FileSelection {
    /// True when every requested path was offered. A paste serves only
    /// covered paths: the Transfer grant alone never authorizes a fetch of
    /// unoffered files.
    pub fn covers(&self, requested: &FileSelection) -> bool {
        requested
            .entries
            .iter()
            .all(|entry| self.entries.iter().any(|own| own.path == entry.path))
    }

    pub fn validate(&self) -> Result<(), Error> {
        if self.entries.len() > MAX_CLIPBOARD_FILES {
            return Err(Error::InvalidSetting(
                "clipboard file selection holds too many entries",
            ));
        }
        for entry in &self.entries {
            if entry.path.is_empty() {
                return Err(Error::InvalidSetting("clipboard file path is empty"));
            }
            crate::fs::check_rel(&entry.path)?;
            if entry.dir && entry.size != 0 {
                return Err(Error::InvalidSetting(
                    "clipboard directory selection must report size zero",
                ));
            }
        }
        if self.entries.len() > 1 {
            let mut sorted: Vec<&str> = self.entries.iter().map(|e| e.path.as_str()).collect();
            sorted.sort_unstable();
            if sorted.windows(2).any(|pair| pair[0] == pair[1]) {
                return Err(Error::InvalidSetting(
                    "clipboard file selection lists a path twice",
                ));
            }
        }
        if self.encoded_len() > MAX_CLIPBOARD_FILES_BYTES {
            return Err(Error::InvalidSetting(
                "clipboard file selection is larger than 64 KiB",
            ));
        }
        Ok(())
    }

    pub(crate) fn encoded_len(&self) -> usize {
        2 + self
            .entries
            .iter()
            .map(|e| 1 + 2 + e.path.len() + 8)
            .sum::<usize>()
    }

    pub(crate) fn encode(&self) -> Result<Vec<u8>, Error> {
        self.validate()?;
        let mut bytes = Vec::with_capacity(self.encoded_len());
        let count = u16::try_from(self.entries.len()).map_err(|_| Error::Malformed)?;
        bytes.extend_from_slice(&count.to_be_bytes());
        for entry in &self.entries {
            bytes.push(u8::from(entry.dir));
            let len = u16::try_from(entry.path.len()).map_err(|_| Error::Malformed)?;
            bytes.extend_from_slice(&len.to_be_bytes());
            bytes.extend_from_slice(entry.path.as_bytes());
            bytes.extend_from_slice(&entry.size.to_be_bytes());
        }
        Ok(bytes)
    }

    pub(crate) fn decode(bytes: &[u8]) -> Result<Self, Error> {
        if bytes.len() < 2 || bytes.len() > MAX_CLIPBOARD_FILES_BYTES {
            return Err(Error::Malformed);
        }
        let count = u16::from_be_bytes(bytes[0..2].try_into().expect("2 bytes")) as usize;
        if count > MAX_CLIPBOARD_FILES {
            return Err(Error::Malformed);
        }
        let mut entries = Vec::with_capacity(count);
        let mut rest = &bytes[2..];
        for _ in 0..count {
            if rest.len() < 11 {
                return Err(Error::Malformed);
            }
            let dir = match rest[0] {
                0 => false,
                1 => true,
                _ => return Err(Error::Malformed),
            };
            let len = u16::from_be_bytes(rest[1..3].try_into().expect("2 bytes")) as usize;
            rest = &rest[3..];
            if rest.len() < len + 8 {
                return Err(Error::Malformed);
            }
            let path = std::str::from_utf8(&rest[..len]).map_err(|_| Error::Malformed)?;
            let size = u64::from_be_bytes(rest[len..len + 8].try_into().expect("8 bytes"));
            rest = &rest[len + 8..];
            entries.push(ClipboardFileEntry {
                path: path.to_owned(),
                size,
                dir,
            });
        }
        if !rest.is_empty() {
            return Err(Error::Malformed);
        }
        let selection = Self { entries };
        selection.validate().map_err(|_| Error::Malformed)?;
        Ok(selection)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(path: &str, size: u64) -> ClipboardFileEntry {
        ClipboardFileEntry {
            path: path.into(),
            size,
            dir: false,
        }
    }

    fn selection(entries: Vec<ClipboardFileEntry>) -> FileSelection {
        FileSelection { entries }
    }

    #[test]
    fn file_selection_wire_codec_round_trips_and_rejects_garbage() {
        use crate::clipboard::{decode, encode, Frame};
        let offer = selection(vec![
            entry("a.txt", 3),
            entry("nested/b.bin", u64::MAX),
            ClipboardFileEntry {
                path: "empty".into(),
                size: 0,
                dir: true,
            },
        ]);
        let bytes = encode(&Frame::Files {
            revision: 7,
            selection: offer.clone(),
        })
        .unwrap();
        assert_eq!(
            decode(&bytes[4..]).unwrap(),
            Frame::Files {
                revision: 7,
                selection: offer,
            }
        );
        let empty = encode(&Frame::Files {
            revision: 3,
            selection: FileSelection::default(),
        })
        .unwrap();
        assert_eq!(
            decode(&empty[4..]).unwrap(),
            Frame::Files {
                revision: 3,
                selection: FileSelection::default(),
            }
        );
        // Tag, truncation, flag, trailing bytes and UTF-8 all fail closed.
        assert!(decode(&[]).is_err());
        assert!(decode(&[9]).is_err());
        assert!(decode(&[4, 0, 0, 0, 0, 0, 0, 0]).is_err());
        let mut truncated = bytes.clone();
        truncated.truncate(truncated.len() - 1);
        assert!(decode(&truncated[4..]).is_err());
        let mut flag = bytes.clone();
        let flag_at = 4 + 1 + 8 + 2;
        flag[flag_at] = 7;
        assert!(decode(&flag[4..]).is_err());
        let mut trailing = bytes.clone();
        trailing.push(0);
        assert!(decode(&trailing[4..]).is_err());
        let mut utf8 = encode(&Frame::Files {
            revision: 1,
            selection: selection(vec![entry("ok.txt", 1)]),
        })
        .unwrap();
        let name_at = utf8.len() - "ok.txt".len() - 8;
        utf8[name_at] = 0xFF;
        assert!(decode(&utf8[4..]).is_err());
        // A hostile peer cannot smuggle an unsafe path or a duplicate past decode.
        let evil = selection(vec![entry("../evil", 1)]);
        assert!(encode(&Frame::Files {
            revision: 1,
            selection: evil,
        })
        .is_err());
        let dupe = selection(vec![entry("d.txt", 1), entry("d.txt", 1)]);
        assert!(encode(&Frame::Files {
            revision: 1,
            selection: dupe,
        })
        .is_err());
        // Debug never prints selected paths.
        let rendered = format!(
            "{:?}",
            Frame::Files {
                revision: 1,
                selection: selection(vec![entry("do-not-log.txt", 1)]),
            }
        );
        assert!(!rendered.contains("do-not-log"));
    }

    #[test]
    fn offer_binding_covers_only_offered_paths() {
        let offered = selection(vec![entry("a.txt", 1), entry("nested/b.txt", 2)]);
        assert!(offered.covers(&selection(vec![entry("a.txt", 9)])));
        assert!(offered.covers(&offered.clone()));
        assert!(!offered.covers(&selection(vec![entry("evil.txt", 1)])));
        assert!(!offered.covers(&selection(vec![entry("a.txt", 1), entry("evil.txt", 1)])));
        assert!(!FileSelection::default().covers(&selection(vec![entry("a.txt", 1)])));
    }
}
