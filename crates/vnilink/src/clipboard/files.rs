//! Clipboard file selections. Metadata only: names, sizes and directory
//! markers cross the clipboard stream so a paste can be offered without
//! downloading a single payload byte. Materialization revalidates the
//! selection revision and fetches bytes through the Files engine.

use crate::error::Error;

/// Largest file selection offer: entries and encoded bytes. A selection is
/// an offer to paste, not a transfer; payloads never ride this path.
pub const MAX_CLIPBOARD_FILES: usize = 256;
pub const MAX_CLIPBOARD_FILES_BYTES: usize = 64 * 1024;

/// One selected file or directory. `path` is relative to the selection root
/// and jailed like transfer names; `size` is an estimate the receiver must
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
/// a fresh revision per offer, so a paste prepared against an older
/// revision is stale.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoredFileSelection {
    pub revision: u64,
    pub selection: FileSelection,
}

/// One materialized paste entry. `hash` authenticates the fetched bytes;
/// `size` is the source-measured length the fetch must reproduce exactly.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PreparedFile {
    pub path: String,
    pub hash: [u8; 32],
    pub size: u64,
}

impl PreparedFile {
    pub fn blob_hash(&self) -> iroh_blobs::Hash {
        iroh_blobs::Hash::from_bytes(self.hash)
    }
}

/// A preparation result, aligned with the requested selection by position.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PreparedPaste {
    pub files: Vec<PreparedFile>,
}

/// A preparation answer. Every prepare gets exactly one: hashes on success,
/// or a refusal reason. Refusals ride the ready frame because the dial side
/// of the stream writes no statuses, so a status alone could never carry a
/// source-side denial back to a host-originated prepare.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum PasteReadyOutcome {
    Ready(Vec<([u8; 32], u64)>),
    TransferDenied,
    Unauthorized,
    Unavailable,
}

/// Ready payload: per-entry content hash and source-measured size, in
/// prepare-request order, or a one-byte refusal.
pub(crate) fn encode_ready(outcome: &PasteReadyOutcome) -> Result<Vec<u8>, Error> {
    match outcome {
        PasteReadyOutcome::Ready(files) => {
            if files.len() > MAX_CLIPBOARD_FILES {
                return Err(Error::Malformed);
            }
            let mut bytes = Vec::with_capacity(3 + files.len() * 40);
            bytes.push(0);
            let count = u16::try_from(files.len()).map_err(|_| Error::Malformed)?;
            bytes.extend_from_slice(&count.to_be_bytes());
            for (hash, size) in files {
                bytes.extend_from_slice(hash);
                bytes.extend_from_slice(&size.to_be_bytes());
            }
            Ok(bytes)
        }
        PasteReadyOutcome::TransferDenied => Ok(vec![1]),
        PasteReadyOutcome::Unauthorized => Ok(vec![2]),
        PasteReadyOutcome::Unavailable => Ok(vec![3]),
    }
}

pub(crate) fn decode_ready(bytes: &[u8]) -> Result<PasteReadyOutcome, Error> {
    let Some((&outcome, rest)) = bytes.split_first() else {
        return Err(Error::Malformed);
    };
    match outcome {
        1 if rest.is_empty() => Ok(PasteReadyOutcome::TransferDenied),
        2 if rest.is_empty() => Ok(PasteReadyOutcome::Unauthorized),
        3 if rest.is_empty() => Ok(PasteReadyOutcome::Unavailable),
        0 => {
            if rest.len() < 2 || rest.len() > 2 + MAX_CLIPBOARD_FILES * 40 {
                return Err(Error::Malformed);
            }
            let count = u16::from_be_bytes(rest[0..2].try_into().expect("2 bytes")) as usize;
            if rest.len() != 2 + count * 40 {
                return Err(Error::Malformed);
            }
            let mut files = Vec::with_capacity(count);
            for n in 0..count {
                let at = 2 + n * 40;
                let hash: [u8; 32] = rest[at..at + 32].try_into().expect("32 bytes");
                let size = u64::from_be_bytes(rest[at + 32..at + 40].try_into().expect("8 bytes"));
                files.push((hash, size));
            }
            Ok(PasteReadyOutcome::Ready(files))
        }
        _ => Err(Error::Malformed),
    }
}

impl FileSelection {
    /// True when every requested path was offered. Paste preparation serves
    /// only covered paths: the Transfer grant alone never authorizes a fetch
    /// of unoffered files.
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
