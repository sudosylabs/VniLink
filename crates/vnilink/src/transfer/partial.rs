//! The partial file a receiver appends to, and its segment journal.

use std::io::{Read, Seek, SeekFrom, Write};

use blake3::hazmat::ChainingValue;

use crate::error::Error;

use super::hash::{segment_len, SegmentHasher, SEGMENT_LEN};

/// Writing the partial failed for a reason other than space.
pub(crate) const PARTIAL_FAILED: &str = "the file could not be staged";
/// The destination's disk is full, or its quota is used up.
pub(crate) const NO_SPACE: &str = "there is not enough space for this file";
const VALUE_LEN: u64 = 32;
/// Bytes read at a time when held bytes are hashed again.
const HOLD_CHUNK: usize = 1024 * 1024;

/// A `size`-byte file arriving: the bytes written so far and, beside them,
/// a journal of the chaining value of every segment checked. A segment is
/// journaled only after its bytes are on disk, so after a crash the
/// journal never claims bytes the partial lost. A resume continues after
/// the last journaled segment, keeping the bytes of the next segment that
/// already arrived: they are read back and hashed again, and the request
/// names them so the sender sends only the rest. Bytes a crash tore fail
/// that segment's check, which costs the segment once.
pub(crate) struct Partial {
    file: std::fs::File,
    journal: std::fs::File,
    size: u64,
    /// Bytes per segment.
    segment: u64,
    /// Bytes in journaled segments.
    checked: u64,
    /// Bytes written, checked or not.
    written: u64,
    /// The segment being written; none once the file is whole.
    hasher: Option<SegmentHasher>,
}

impl Partial {
    /// Opens or creates the partial at `path` and its journal at
    /// `journal`. Bytes past the segment after the last journaled one are
    /// dropped.
    pub(crate) fn open(
        path: &std::path::Path,
        journal: &std::path::Path,
        size: u64,
    ) -> Result<Self, Error> {
        Self::open_in(path, journal, size, SEGMENT_LEN)
    }

    /// [`Partial::open`] for a file cut into segments of `segment`, which
    /// tests make small.
    pub(crate) fn open_in(
        path: &std::path::Path,
        journal: &std::path::Path,
        size: u64,
        segment: u64,
    ) -> Result<Self, Error> {
        let open = |path: &std::path::Path| {
            std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .truncate(false)
                .open(path)
                .map_err(failed)
        };
        let file = open(path)?;
        let journal = open(journal)?;
        let on_disk = file.metadata().map_err(failed)?.len();
        let values = journal.metadata().map_err(failed)?.len() / VALUE_LEN;
        let mut checked = 0;
        let mut segments = 0;
        while segments < values && checked < size {
            let end = checked + segment_len(checked, size, segment);
            if end > on_disk {
                break;
            }
            checked = end;
            segments += 1;
        }
        let mut partial = Self {
            file,
            journal,
            size,
            segment,
            checked,
            written: checked,
            hasher: next_segment(checked, size),
        };
        partial.truncate(segments)?;
        let unchecked = on_disk.saturating_sub(checked);
        partial.hold(unchecked.min(partial.segment_left()))?;
        Ok(partial)
    }

    /// Takes the first `bytes` past the checked segments back into the
    /// segment being written, reading them from disk for its hasher.
    fn hold(&mut self, bytes: u64) -> Result<(), Error> {
        if let Some(hasher) = self.hasher.as_mut() {
            let mut chunk = vec![0u8; HOLD_CHUNK.min(bytes as usize)];
            let mut left = bytes;
            while left > 0 {
                let take = left.min(HOLD_CHUNK as u64) as usize;
                self.file.read_exact(&mut chunk[..take]).map_err(failed)?;
                hasher.update(&chunk[..take]);
                left -= take as u64;
            }
        }
        self.written = self.checked + bytes;
        self.file.set_len(self.written).map_err(failed)
    }

    /// Bytes in checked segments: where the next request starts.
    pub(crate) fn checked(&self) -> u64 {
        self.checked
    }

    pub(crate) fn written(&self) -> u64 {
        self.written
    }

    /// Bytes written past the checked segments: the request's held bytes.
    pub(crate) fn held(&self) -> u64 {
        self.written - self.checked
    }

    /// Bytes still due in the segment being written.
    pub(crate) fn segment_left(&self) -> u64 {
        if self.checked == self.size {
            return 0;
        }
        self.checked + segment_len(self.checked, self.size, self.segment) - self.written
    }

    /// Writes bytes of the current segment; at most [`Self::segment_left`].
    pub(crate) fn append(&mut self, bytes: &[u8]) -> Result<(), Error> {
        debug_assert!(bytes.len() as u64 <= self.segment_left());
        let hasher = self.hasher.as_mut().ok_or(Error::Malformed)?;
        self.file.write_all(bytes).map_err(failed)?;
        hasher.update(bytes);
        self.written += bytes.len() as u64;
        Ok(())
    }

    /// Checks the written segment against the sender's `value`. A match
    /// before the last segment is synced and journaled; a mismatch drops
    /// the segment's bytes, so the next request starts at it again.
    pub(crate) fn seal(&mut self, value: &[u8]) -> Result<bool, Error> {
        debug_assert_eq!(self.segment_left(), 0);
        let expected: ChainingValue = self.hasher.as_ref().ok_or(Error::Malformed)?.finalize();
        if expected[..] != *value {
            self.drop_unchecked()?;
            return Ok(false);
        }
        // The last segment is published as soon as it checks, so only the
        // segments before it are worth a sync and a journal line: a small
        // file costs neither, and an interruption repeats one segment.
        if self.written < self.size {
            self.file.sync_data().map_err(failed)?;
            self.journal.write_all(&expected).map_err(failed)?;
        }
        self.checked = self.written;
        self.hasher = next_segment(self.checked, self.size);
        Ok(true)
    }

    /// Drops bytes written past the last checked segment.
    pub(crate) fn drop_unchecked(&mut self) -> Result<(), Error> {
        self.file.set_len(self.checked).map_err(failed)?;
        self.file
            .seek(SeekFrom::Start(self.checked))
            .map_err(failed)?;
        self.written = self.checked;
        self.hasher = next_segment(self.checked, self.size);
        Ok(())
    }

    /// Cuts the journal to its first `segments` values and positions the
    /// partial after the checked segments; the bytes past them stay until
    /// [`Self::hold`] keeps or cuts them.
    fn truncate(&mut self, segments: u64) -> Result<(), Error> {
        self.file
            .seek(SeekFrom::Start(self.checked))
            .map_err(failed)?;
        self.journal.set_len(segments * VALUE_LEN).map_err(failed)?;
        self.journal.seek(SeekFrom::End(0)).map_err(failed)?;
        Ok(())
    }
}

/// The hasher of the segment starting at `start`, unless the file ends there.
fn next_segment(start: u64, size: u64) -> Option<SegmentHasher> {
    (start < size).then(|| SegmentHasher::at(start))
}

fn failed(error: std::io::Error) -> Error {
    Error::InvalidSetting(if no_space(&error) {
        NO_SPACE
    } else {
        PARTIAL_FAILED
    })
}

/// Whether `error` says the disk or the user's quota is full.
fn no_space(error: &std::io::Error) -> bool {
    use std::io::ErrorKind;
    if matches!(
        error.kind(),
        ErrorKind::StorageFull | ErrorKind::QuotaExceeded
    ) {
        return true;
    }
    // ERROR_HANDLE_DISK_FULL and ERROR_DISK_FULL.
    cfg!(windows) && matches!(error.raw_os_error(), Some(39 | 112))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Small segments, so files of several stay cheap.
    const SEGMENT_LEN: u64 = 64 * 1024;

    struct Scratch(std::path::PathBuf);

    impl Scratch {
        fn new(tag: &str) -> Self {
            let dir =
                std::env::temp_dir().join(format!("vnilink-partial-{tag}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }

        fn open(&self, size: u64) -> Partial {
            Partial::open_in(
                &self.0.join("data"),
                &self.0.join("journal"),
                size,
                SEGMENT_LEN,
            )
            .unwrap()
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn value_of(bytes: &[u8], start: u64) -> ChainingValue {
        let mut hasher = SegmentHasher::at(start);
        hasher.update(bytes);
        hasher.finalize()
    }

    /// Writes what is left of the current segment from `whole` and seals
    /// it with its true value.
    fn land_segment(partial: &mut Partial, whole: &[u8]) {
        let start = partial.checked();
        let end = start + segment_len(start, whole.len() as u64, SEGMENT_LEN);
        partial
            .append(&whole[partial.written() as usize..end as usize])
            .unwrap();
        assert!(partial
            .seal(&value_of(&whole[start as usize..end as usize], start))
            .unwrap());
    }

    #[test]
    fn a_resume_continues_after_the_last_journaled_segment() {
        let scratch = Scratch::new("resume");
        let whole: Vec<u8> = (0..2 * SEGMENT_LEN + 99).map(|n| n as u8).collect();
        let size = whole.len() as u64;
        let mut partial = scratch.open(size);
        land_segment(&mut partial, &whole);
        // Half of the second segment arrives before the process dies.
        let half = (SEGMENT_LEN + SEGMENT_LEN / 2) as usize;
        partial.append(&whole[SEGMENT_LEN as usize..half]).unwrap();
        drop(partial);

        let mut partial = scratch.open(size);
        assert_eq!(partial.checked(), SEGMENT_LEN);
        assert_eq!(
            partial.held(),
            SEGMENT_LEN / 2,
            "the unchecked half is held"
        );
        land_segment(&mut partial, &whole);
        land_segment(&mut partial, &whole);
        assert_eq!(partial.checked(), size);
        assert_eq!(partial.segment_left(), 0);
        drop(partial);
        assert_eq!(std::fs::read(scratch.0.join("data")).unwrap(), whole);
    }

    /// A whole segment that arrived without its value is held whole, and
    /// only the value is still due; bytes past it are cut.
    #[test]
    fn a_resume_holds_at_most_the_next_segment() {
        let scratch = Scratch::new("hold");
        let whole: Vec<u8> = (0..2 * SEGMENT_LEN + 7).map(|n| (n * 3) as u8).collect();
        let size = whole.len() as u64;
        std::fs::write(scratch.0.join("data"), &whole[..(SEGMENT_LEN + 5) as usize]).unwrap();
        let mut partial = scratch.open(size);
        assert_eq!((partial.checked(), partial.held()), (0, SEGMENT_LEN));
        assert_eq!(partial.segment_left(), 0);
        assert!(partial
            .seal(&value_of(&whole[..SEGMENT_LEN as usize], 0))
            .unwrap());
        drop(partial);
        assert_eq!(
            std::fs::metadata(scratch.0.join("data")).unwrap().len(),
            SEGMENT_LEN
        );
    }

    /// Held bytes a crash tore fail the segment's check, which drops them.
    #[test]
    fn torn_held_bytes_fail_their_segment() {
        let scratch = Scratch::new("torn");
        let whole = vec![9u8; SEGMENT_LEN as usize];
        std::fs::write(scratch.0.join("data"), vec![0u8; 100]).unwrap();
        let mut partial = scratch.open(whole.len() as u64);
        assert_eq!(partial.held(), 100);
        partial.append(&whole[100..]).unwrap();
        assert!(!partial.seal(&value_of(&whole, 0)).unwrap());
        assert_eq!(partial.held(), 0);
        land_segment(&mut partial, &whole);
        drop(partial);
        assert_eq!(std::fs::read(scratch.0.join("data")).unwrap(), whole);
    }

    #[test]
    fn a_journal_ahead_of_its_bytes_is_cut_back() {
        let scratch = Scratch::new("ahead");
        let whole = vec![5u8; 2 * SEGMENT_LEN as usize];
        let mut partial = scratch.open(whole.len() as u64);
        land_segment(&mut partial, &whole);
        drop(partial);
        // The bytes of a journaled segment went missing.
        std::fs::OpenOptions::new()
            .write(true)
            .open(scratch.0.join("data"))
            .unwrap()
            .set_len(SEGMENT_LEN - 1)
            .unwrap();
        let partial = scratch.open(whole.len() as u64);
        assert_eq!(partial.checked(), 0);
        assert_eq!(
            std::fs::metadata(scratch.0.join("journal")).unwrap().len(),
            0
        );
    }

    #[test]
    fn a_segment_that_does_not_check_is_dropped() {
        let scratch = Scratch::new("mismatch");
        let whole = vec![3u8; SEGMENT_LEN as usize + 10];
        let mut partial = scratch.open(whole.len() as u64);
        land_segment(&mut partial, &whole);
        partial.append(&[4u8; 10]).unwrap();
        assert!(!partial
            .seal(&value_of(&whole[SEGMENT_LEN as usize..], SEGMENT_LEN))
            .unwrap());
        assert_eq!(partial.checked(), SEGMENT_LEN);
        assert_eq!(partial.written(), SEGMENT_LEN);
        land_segment(&mut partial, &whole);
        drop(partial);
        assert_eq!(std::fs::read(scratch.0.join("data")).unwrap(), whole);
    }

    #[test]
    fn a_full_disk_is_named() {
        let full = std::io::Error::from(std::io::ErrorKind::StorageFull);
        assert!(matches!(failed(full), Error::InvalidSetting(NO_SPACE)));
        let other = std::io::Error::from(std::io::ErrorKind::PermissionDenied);
        assert!(matches!(
            failed(other),
            Error::InvalidSetting(PARTIAL_FAILED)
        ));
    }

    #[test]
    fn an_empty_file_is_whole_at_once() {
        let scratch = Scratch::new("empty");
        let partial = scratch.open(0);
        assert_eq!(partial.checked(), 0);
        assert_eq!(partial.segment_left(), 0);
    }
}
