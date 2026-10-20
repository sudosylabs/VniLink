//! Segmented BLAKE3: a file is checked segment by segment.
//!
//! Segments are [`SEGMENT_LEN`] bytes, a power of two, so each one is a
//! whole subtree of the file's BLAKE3 tree and its chaining value is the
//! value BLAKE3 itself computes for those bytes at that offset. A segment
//! can therefore be checked the moment it arrives, and a resumed file
//! never rereads a segment already checked: its chaining value is kept.

use blake3::hazmat::{ChainingValue, HasherExt};

/// Bytes per segment: what a failed check refetches and what a resume can
/// lose at most.
pub(crate) const SEGMENT_LEN: u64 = 16 * 1024 * 1024;

/// Hashes the segment starting at `start`, a multiple of [`SEGMENT_LEN`].
pub(crate) struct SegmentHasher {
    hasher: blake3::Hasher,
}

impl SegmentHasher {
    pub(crate) fn at(start: u64) -> Self {
        debug_assert_eq!(start % SEGMENT_LEN, 0);
        let mut hasher = blake3::Hasher::new();
        hasher.set_input_offset(start);
        Self { hasher }
    }

    pub(crate) fn update(&mut self, bytes: &[u8]) {
        self.hasher.update(bytes);
    }

    /// The segment's chaining value.
    pub(crate) fn finalize(&self) -> ChainingValue {
        self.hasher.finalize_non_root()
    }
}

/// The length of the segment starting at `start` in a `size`-byte file.
pub(crate) fn segment_len(start: u64, size: u64) -> u64 {
    SEGMENT_LEN.min(size - start)
}

/// The root BLAKE3 hash of a `size`-byte file from its segments' chaining
/// values, in order. Proves the segmentation is BLAKE3's own tree.
#[cfg(test)]
pub(crate) fn root(segments: &[ChainingValue], size: u64) -> blake3::Hash {
    use blake3::hazmat::{left_subtree_len, merge_subtrees_non_root, merge_subtrees_root, Mode};

    fn subtree(segments: &[ChainingValue], start: u64, len: u64) -> ChainingValue {
        if len <= SEGMENT_LEN {
            return segments[(start / SEGMENT_LEN) as usize];
        }
        let left = left_subtree_len(len);
        merge_subtrees_non_root(
            &subtree(segments, start, left),
            &subtree(segments, start + left, len - left),
            Mode::Hash,
        )
    }

    assert!(size > SEGMENT_LEN, "a single segment is hashed whole");
    let left = left_subtree_len(size);
    merge_subtrees_root(
        &subtree(segments, 0, left),
        &subtree(segments, left, size - left),
        Mode::Hash,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn segments_of(bytes: &[u8]) -> Vec<ChainingValue> {
        let size = bytes.len() as u64;
        let mut start = 0;
        let mut values = Vec::new();
        while start < size {
            let end = start + segment_len(start, size);
            let mut hasher = SegmentHasher::at(start);
            hasher.update(&bytes[start as usize..end as usize]);
            values.push(hasher.finalize());
            start = end;
        }
        values
    }

    /// Segment values compose into the BLAKE3 hash of the whole file, for
    /// even, uneven and power-of-two segment counts.
    #[test]
    fn segments_are_subtrees_of_the_file_hash() {
        let seg = SEGMENT_LEN as usize;
        for len in [seg + 1, 2 * seg, 3 * seg - 5, 4 * seg + 1024, 5 * seg + 7] {
            let bytes: Vec<u8> = (0..len).map(|n| (n % 251) as u8).collect();
            assert_eq!(
                root(&segments_of(&bytes), len as u64),
                blake3::hash(&bytes),
                "{len} bytes"
            );
        }
    }

    /// A segment's value depends on where it lies, so a segment replayed at
    /// another offset never checks.
    #[test]
    fn a_segment_is_bound_to_its_offset() {
        let bytes = vec![9u8; 4096];
        let mut first = SegmentHasher::at(0);
        first.update(&bytes);
        let mut second = SegmentHasher::at(SEGMENT_LEN);
        second.update(&bytes);
        assert_ne!(first.finalize(), second.finalize());
    }
}
