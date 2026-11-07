// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! How a segment's bytes are framed on the wire.
//!
//! The sent bytes of a segment start with a mode byte. In [`MODE_RAW`] the
//! bytes follow as they are. In [`MODE_ZSTD`] they follow as blocks, each a
//! 4-byte big-endian length and one zstd frame of up to [`BLOCK`] bytes of
//! the file. Blocks are compressed independently, so a receiver decodes
//! each as it arrives, and a resume that starts mid-segment starts a new
//! sequence of blocks at the first byte it lacks. The receiver knows how many
//! bytes each block holds (the block size, or what the segment has left), so
//! a frame that decodes to any other size is malformed.
//!
//! The sender chooses a segment's mode from its first block: zstd when that
//! block shrinks enough to be worth decoding, raw otherwise. Data that does
//! not compress (archives, media, encrypted files) therefore costs one block
//! of compression per segment and nothing more.

use std::cell::RefCell;

use crate::error::Error;

/// Bytes of a file in one compressed block.
pub(crate) const BLOCK: usize = 1024 * 1024;

/// The segment's bytes follow as they are.
pub(crate) const MODE_RAW: u8 = 0;
/// The segment's bytes follow as compressed blocks.
pub(crate) const MODE_ZSTD: u8 = 1;

/// Size of a block's length prefix.
const HEADER_LEN: usize = 4;

/// zstd's fastest level: about as fast as a disk reads, and most of what
/// compressible data gives.
const LEVEL: i32 = 1;

/// A first block must shrink to this share (7/8) of its size, or its
/// segment travels raw.
const WORTH: (usize, usize) = (7, 8);

thread_local! {
    /// A compressor per reader thread: creating one costs more than
    /// compressing a small file.
    static ENCODER: RefCell<Option<zstd::bulk::Compressor<'static>>> = const { RefCell::new(None) };
}

/// The most a block's frame may hold, header excluded.
fn frame_limit() -> usize {
    zstd::zstd_safe::compress_bound(BLOCK)
}

/// `block` as a length-prefixed zstd frame.
pub(crate) fn encode(block: &[u8]) -> std::io::Result<Vec<u8>> {
    ENCODER.with(|slot| {
        let mut slot = slot.borrow_mut();
        let encoder = match slot.as_mut() {
            Some(encoder) => encoder,
            None => slot.insert(zstd::bulk::Compressor::new(LEVEL)?),
        };
        let mut frame = vec![0u8; HEADER_LEN + zstd::zstd_safe::compress_bound(block.len())];
        let body = encoder.compress_to_buffer(block, &mut frame[HEADER_LEN..])?;
        frame.truncate(HEADER_LEN + body);
        frame[..HEADER_LEN].copy_from_slice(&(body as u32).to_be_bytes());
        Ok(frame)
    })
}

/// The frame of `block` when it shrinks enough to be sent compressed.
pub(crate) fn probe(block: &[u8]) -> std::io::Result<Option<Vec<u8>>> {
    let frame = encode(block)?;
    Ok((frame.len() - HEADER_LEN <= block.len() / WORTH.1 * WORTH.0).then_some(frame))
}

/// Collects the compressed blocks of a stream, which arrive in any pieces,
/// and decodes each once whole.
#[derive(Default)]
pub(crate) struct Blocks {
    frame: Vec<u8>,
    decoder: Option<zstd::bulk::Decompressor<'static>>,
    plain: Vec<u8>,
}

impl Blocks {
    /// Takes bytes of the frame of a block that holds `plain_len` bytes of
    /// the file. Returns how many of `bytes` it took and, once the frame is
    /// whole, the block it decodes to. A frame that is empty, too long for a
    /// block or does not decode to exactly `plain_len` bytes is malformed.
    pub(crate) fn take(
        &mut self,
        bytes: &[u8],
        plain_len: usize,
    ) -> Result<(usize, Option<&[u8]>), Error> {
        let mut taken = 0;
        if self.frame.len() < HEADER_LEN {
            taken = bytes.len().min(HEADER_LEN - self.frame.len());
            self.frame.extend_from_slice(&bytes[..taken]);
            if self.frame.len() < HEADER_LEN {
                return Ok((taken, None));
            }
            let body = self.body_len();
            if body == 0 || body > frame_limit() {
                return Err(Error::Malformed);
            }
        }
        let whole = HEADER_LEN + self.body_len();
        let more = (whole - self.frame.len()).min(bytes.len() - taken);
        self.frame.extend_from_slice(&bytes[taken..taken + more]);
        taken += more;
        if self.frame.len() < whole {
            return Ok((taken, None));
        }
        if self.decoder.is_none() {
            self.decoder = Some(zstd::bulk::Decompressor::new().map_err(|_| Error::Malformed)?);
        }
        let decoder = self.decoder.as_mut().expect("created above");
        self.plain.resize(BLOCK, 0);
        let decoded = decoder
            .decompress_to_buffer(&self.frame[HEADER_LEN..], &mut self.plain[..plain_len])
            .map_err(|_| Error::Malformed)?;
        self.frame.clear();
        if decoded != plain_len {
            return Err(Error::Malformed);
        }
        Ok((taken, Some(&self.plain[..plain_len])))
    }

    fn body_len(&self) -> usize {
        u32::from_be_bytes(self.frame[..HEADER_LEN].try_into().expect("4 bytes")) as usize
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(len: usize) -> Vec<u8> {
        b"the quick brown fox jumps over the lazy dog\n"
            .iter()
            .copied()
            .cycle()
            .take(len)
            .collect()
    }

    fn noise(len: usize) -> Vec<u8> {
        let mut state = 0x9e37_79b9_7f4a_7c15u64;
        (0..len)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                (state >> 24) as u8
            })
            .collect()
    }

    /// The blocks of `frame`, fed in pieces of `piece` bytes, decode to the
    /// block.
    fn decode(frame: &[u8], plain_len: usize, piece: usize) -> Result<Vec<u8>, Error> {
        let mut blocks = Blocks::default();
        for part in frame.chunks(piece) {
            let mut part = part;
            while !part.is_empty() {
                let (taken, block) = blocks.take(part, plain_len)?;
                if let Some(block) = block {
                    assert_eq!(taken, part.len(), "a frame ends with its last byte");
                    return Ok(block.to_vec());
                }
                part = &part[taken..];
            }
        }
        panic!("the frame never completed");
    }

    #[test]
    fn a_block_round_trips_however_its_frame_is_cut() {
        let block = text(BLOCK);
        let frame = encode(&block).unwrap();
        assert!(frame.len() < block.len() / 10, "text shrinks");
        for piece in [1, 3, HEADER_LEN, 4096, frame.len()] {
            assert_eq!(decode(&frame, block.len(), piece).unwrap(), block);
        }
    }

    #[test]
    fn text_is_worth_compressing_and_noise_is_not() {
        assert!(probe(&text(BLOCK)).unwrap().is_some());
        assert!(probe(&noise(BLOCK)).unwrap().is_none());
    }

    #[test]
    fn a_frame_that_decodes_to_another_size_is_malformed() {
        let frame = encode(&text(1000)).unwrap();
        for plain_len in [999, 1001, 2000] {
            assert!(matches!(
                decode(&frame, plain_len, frame.len()),
                Err(Error::Malformed)
            ));
        }
    }

    #[test]
    fn an_empty_or_oversized_frame_is_malformed() {
        for body in [0u32, frame_limit() as u32 + 1, u32::MAX] {
            let mut blocks = Blocks::default();
            assert!(matches!(
                blocks.take(&body.to_be_bytes(), BLOCK),
                Err(Error::Malformed)
            ));
        }
    }

    #[test]
    fn bytes_that_are_not_a_frame_are_malformed() {
        let mut frame = (64u32).to_be_bytes().to_vec();
        frame.extend_from_slice(&noise(64));
        assert!(matches!(
            decode(&frame, 100, frame.len()),
            Err(Error::Malformed)
        ));
    }

    /// A frame takes only its own bytes, so the next block's follow.
    #[test]
    fn a_frame_leaves_the_bytes_after_it() {
        let block = text(5000);
        let mut stream = encode(&block).unwrap();
        let frame_len = stream.len();
        stream.extend_from_slice(b"next");
        let mut blocks = Blocks::default();
        let (taken, decoded) = blocks.take(&stream, block.len()).unwrap();
        assert_eq!(taken, frame_len);
        assert_eq!(decoded.unwrap(), block);
    }
}
