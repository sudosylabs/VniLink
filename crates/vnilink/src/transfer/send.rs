// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! Sending one held source from a segment boundary, each segment followed
//! by its chaining value.
// trace: records every ignored failure

use std::io::{Read, Seek, SeekFrom};
use std::time::Duration;

use bytes::Bytes;
use iroh::endpoint::{RecvStream, SendStream, VarInt};

use super::codec::{self, BLOCK, MODE_RAW, MODE_ZSTD};
use super::hash::{segment_len, SegmentHasher, SEGMENT_LEN};
use super::wire::{
    FileRequest, REQUEST_LEN, RESET_CHANGED, STATUS_DENIED, STATUS_SENDING, VALUE_LEN,
};
use super::{TransferCounters, CHUNK};

/// Chunks read ahead of the network.
const READ_AHEAD: usize = 8;
/// Chunks between checks that the file may still be sent.
const RECHECK_EVERY: u64 = 16;
/// How long a request may take to arrive once its stream opens.
const REQUEST_WITHIN: Duration = Duration::from_secs(10);

/// A source held open for sending, with the size and modification time
/// the receiver expects of it.
pub(crate) struct Source {
    pub(crate) file: std::fs::File,
    pub(crate) size: u64,
    /// Zero when unknown.
    pub(crate) mtime_ms: u64,
}

/// The request a stream starts with, or `None` when it does not arrive
/// whole and in time.
pub(crate) async fn read_request(recv: &mut RecvStream) -> Option<FileRequest> {
    let mut bytes = [0u8; REQUEST_LEN];
    match tokio::time::timeout(REQUEST_WITHIN, recv.read_exact(&mut bytes)).await {
        Ok(Ok(())) => FileRequest::decode(&bytes).ok(),
        _ => None,
    }
}

/// Answers a request with a refusal status and ends the stream.
pub(crate) async fn refuse(send: &mut SendStream, status: u8) {
    if send.write_all(&[status]).await.is_ok() {
        crate::ignored!(
            crate::trace::FILES,
            "a refusal's stream that cannot finish is closed with its link",
            send.finish()
        );
    }
}

/// What the reader hands the network, in order.
enum Piece {
    /// How the bytes of the segment that follow are framed (`codec`).
    Mode(u8),
    /// Bytes of the stream, standing for `plain` bytes of the file.
    Bytes {
        wire: Bytes,
        plain: usize,
    },
    /// A segment's chaining value, after its bytes.
    Value([u8; VALUE_LEN]),
    /// Every segment was sent.
    End,
    Changed,
    Failed,
}

/// Sends `source` segment by segment from `offset`, a segment boundary,
/// leaving out the first `held` bytes, which the receiver holds: they are
/// read and hashed for the segment's value, not sent. The file is read on
/// a blocking thread a few chunks ahead of the network, and nothing before
/// `offset` is read. `may_send` is asked every
/// few megabytes; once it says no, the stream is reset, so a cancel or
/// revoke stops a file mid-way. `sent_to` hears how far into the
/// file the stream has taken bytes, and finally that the file was sent. A source that changed while it was read
/// is reset as changed instead of carrying on.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn send(
    send: &mut SendStream,
    source: Source,
    offset: u64,
    held: u64,
    counters: &TransferCounters,
    mut may_send: impl FnMut() -> bool,
    mut sent_to: impl FnMut(u64, bool),
) {
    if !holds_within_segment(offset, held, source.size) {
        refuse(send, STATUS_DENIED).await;
        return;
    }
    if send.write_all(&[STATUS_SENDING]).await.is_err() {
        return;
    }
    let started = std::time::Instant::now();
    let size = source.size;
    let (pieces, mut arriving) = tokio::sync::mpsc::channel(READ_AHEAD);
    let reader = tokio::task::spawn_blocking(move || read_source(source, offset, held, pieces));
    let mut sent: u64 = 0;
    let mut compressed: u64 = 0;
    let mut position = offset + held;
    let mut ended = "the reader stopped";
    while let Some(piece) = arriving.recv().await {
        match piece {
            Piece::Mode(mode) => {
                if send.write_all(&[mode]).await.is_err() {
                    ended = "the receiver left";
                    break;
                }
                compressed += u64::from(mode == MODE_ZSTD);
            }
            Piece::Bytes { wire, plain } => {
                // No deadline on a write: on a slow link a write waits while
                // the stream's window drains, for longer than any fixed time.
                // A receiver that stops reading stops the stream itself, and
                // a peer that is gone ends the connection.
                if send.write_chunk(wire).await.is_err() {
                    ended = "the receiver left";
                    break;
                }
                counters.sent(plain);
                position += plain as u64;
                sent_to(position, false);
                sent += 1;
                if sent.is_multiple_of(RECHECK_EVERY) && !may_send() {
                    crate::ignored!(
                        crate::trace::FILES,
                        "a stream that cannot be reset is closed with its link",
                        send.reset(VarInt::from_u32(0))
                    );
                    ended = "cancelled or no longer permitted";
                    break;
                }
            }
            Piece::Value(value) => {
                if send.write_all(&value).await.is_err() {
                    ended = "the receiver left";
                    break;
                }
            }
            Piece::End => {
                if send.finish().is_ok() {
                    counters.served();
                    sent_to(position, true);
                    ended = "sent";
                } else {
                    ended = "the receiver left before the end";
                }
                break;
            }
            Piece::Changed => {
                crate::ignored!(
                    crate::trace::FILES,
                    "a stream that cannot be reset is closed with its link",
                    send.reset(VarInt::from_u32(RESET_CHANGED))
                );
                ended = "the file changed while it was read";
                break;
            }
            Piece::Failed => {
                crate::ignored!(
                    crate::trace::FILES,
                    "a stream that cannot be reset is closed with its link",
                    send.reset(VarInt::from_u32(0))
                );
                ended = "the file could not be read";
                break;
            }
        }
    }
    // A dropped receiver stops the reader at its next chunk.
    drop(arriving);
    if reader.await.is_err() {
        tracing::warn!(target: crate::trace::FILES, "a file reader panicked");
    }
    tracing::debug!(
        target: crate::trace::FILES,
        size,
        from = offset + held,
        sent = position.saturating_sub(offset + held),
        compressed = (compressed > 0).then_some(compressed),
        took_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
        ended,
        "file served"
    );
}

/// Whether `offset` is a segment boundary of a `size`-byte file and `held`
/// bytes fit in the segment there.
fn holds_within_segment(offset: u64, held: u64, size: u64) -> bool {
    offset <= size
        && offset.is_multiple_of(SEGMENT_LEN)
        && held <= segment_len(offset, size, SEGMENT_LEN)
}

/// Reads the held source from `offset` to its end once, segment by
/// segment, handing on all but the first `held` bytes. Each segment's value
/// follows its bytes only while the source still has the expected size and
/// time, and has not changed since the read began, checked as each segment
/// ends.
fn read_source(
    mut source: Source,
    offset: u64,
    held: u64,
    pieces: tokio::sync::mpsc::Sender<Piece>,
) {
    let began = change_stamp(&source.file);
    if source.file.seek(SeekFrom::Start(offset)).is_err() {
        crate::ignored!(
            crate::trace::FILES,
            "a sender that left needs no piece",
            pieces.blocking_send(Piece::Failed)
        );
        return;
    }
    let unchanged = |file: &std::fs::File| {
        held_identity(file).is_some_and(|(size, mtime_ms)| {
            size == source.size && (source.mtime_ms == 0 || mtime_ms == source.mtime_ms)
        }) && change_stamp(file) == began
    };
    let sent_from = offset + held;
    let mut start = offset;
    while start < source.size {
        let end = start + segment_len(start, source.size, SEGMENT_LEN);
        let mut hasher = SegmentHasher::at(start);
        let mut at = start;
        // Chosen with the first block of the segment that is sent.
        let mut mode = None;
        while at < end {
            // A read never straddles the held bytes: they are only hashed.
            let held_block = at < sent_from;
            let limit = if held_block { sent_from.min(end) } else { end };
            let mut block = vec![0u8; BLOCK.min((limit - at) as usize)];
            if let Err(error) = source.file.read_exact(&mut block) {
                let piece = if error.kind() == std::io::ErrorKind::UnexpectedEof {
                    Piece::Changed
                } else {
                    Piece::Failed
                };
                crate::ignored!(
                    crate::trace::FILES,
                    "a sender that left needs no piece",
                    pieces.blocking_send(piece)
                );
                return;
            }
            hasher.update(&block);
            at += block.len() as u64;
            if held_block {
                continue;
            }
            if !send_block(&pieces, &mut mode, block) {
                return;
            }
        }
        if !unchanged(&source.file) {
            crate::ignored!(
                crate::trace::FILES,
                "a sender that left needs no piece",
                pieces.blocking_send(Piece::Changed)
            );
            return;
        }
        if pieces
            .blocking_send(Piece::Value(hasher.finalize()))
            .is_err()
        {
            return;
        }
        start = end;
    }
    let piece = if unchanged(&source.file) {
        Piece::End
    } else {
        Piece::Changed
    };
    crate::ignored!(
        crate::trace::FILES,
        "a sender that left needs no piece",
        pieces.blocking_send(piece)
    );
}

/// Hands on one block of the segment being read, as its mode asks; the
/// first block of a segment sets that mode. False when the sender left or
/// the block could not be framed.
fn send_block(
    pieces: &tokio::sync::mpsc::Sender<Piece>,
    mode: &mut Option<u8>,
    block: Vec<u8>,
) -> bool {
    let plain = block.len();
    let first = mode.is_none();
    let framed = match *mode {
        None => codec::probe(&block).inspect(|frame| {
            *mode = Some(if frame.is_some() { MODE_ZSTD } else { MODE_RAW });
        }),
        Some(MODE_ZSTD) => codec::encode(&block).map(Some),
        Some(_) => Ok(None),
    };
    let framed = match framed {
        Ok(framed) => framed,
        Err(error) => {
            tracing::warn!(target: crate::trace::FILES, %error, "a block could not be compressed");
            crate::ignored!(
                crate::trace::FILES,
                "a sender that left needs no piece",
                pieces.blocking_send(Piece::Failed)
            );
            return false;
        }
    };
    if let (true, Some(mode)) = (first, *mode) {
        if pieces.blocking_send(Piece::Mode(mode)).is_err() {
            return false;
        }
    }
    if let Some(frame) = framed {
        let wire = Bytes::from(frame);
        return pieces.blocking_send(Piece::Bytes { wire, plain }).is_ok();
    }
    let mut bytes = Bytes::from(block);
    while !bytes.is_empty() {
        let wire = bytes.split_to(CHUNK.min(bytes.len()));
        let plain = wire.len();
        if pieces.blocking_send(Piece::Bytes { wire, plain }).is_err() {
            return false;
        }
    }
    true
}

/// When an open file last changed, to the nanosecond: a write moves it, and
/// so does setting the modification time back, which a save within the same
/// millisecond as the scan, or a tool that keeps times, leaves unchanged.
/// Windows has none to offer and needs none: a file served there is opened
/// for reading only, and other writers are refused while it is read.
#[cfg(unix)]
fn change_stamp(file: &std::fs::File) -> Option<(i64, i64)> {
    use std::os::unix::fs::MetadataExt;
    let meta = file.metadata().ok()?;
    Some((meta.ctime(), meta.ctime_nsec()))
}

#[cfg(not(unix))]
fn change_stamp(_: &std::fs::File) -> Option<(i64, i64)> {
    None
}

/// Size and modification time of an open file, in manifest units.
fn held_identity(file: &std::fs::File) -> Option<(u64, u64)> {
    let meta = file.metadata().ok()?;
    let mtime_ms = meta
        .modified()
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since| since.as_millis() as u64)
        .unwrap_or(0);
    Some((meta.len(), mtime_ms))
}

#[cfg(all(test, not(windows)))]
mod tests {
    use super::super::codec::Blocks;
    use super::*;

    fn scratch(tag: &str, bytes: &[u8]) -> (std::path::PathBuf, std::fs::File) {
        let path = std::env::temp_dir().join(format!(
            "vnilink-send-{tag}-{}-{}",
            std::process::id(),
            bytes.len()
        ));
        std::fs::write(&path, bytes).unwrap();
        let file = std::fs::File::open(&path).unwrap();
        (path, file)
    }

    fn pieces_of(source: Source, offset: u64) -> Vec<Piece> {
        held_pieces_of(source, offset, 0)
    }

    fn held_pieces_of(source: Source, offset: u64, held: u64) -> Vec<Piece> {
        let (pieces, mut arriving) = tokio::sync::mpsc::channel(1024);
        read_source(source, offset, held, pieces);
        std::iter::from_fn(|| arriving.try_recv().ok()).collect()
    }

    /// What a receiver makes of a segment's stream bytes: its mode and the
    /// `plain_len` bytes of the file they carry.
    fn decoded(mode: u8, wire: &[u8], plain_len: usize) -> Vec<u8> {
        if mode == MODE_RAW {
            return wire.to_vec();
        }
        let mut blocks = Blocks::default();
        let (mut plain, mut rest) = (Vec::new(), wire);
        while plain.len() < plain_len {
            let (taken, block) = blocks
                .take(rest, BLOCK.min(plain_len - plain.len()))
                .unwrap();
            plain.extend_from_slice(block.expect("a whole frame"));
            rest = &rest[taken..];
        }
        assert!(rest.is_empty(), "the frames end with the segment");
        plain
    }

    /// The segments of `pieces` as (mode, wire bytes, plain bytes, value),
    /// and whether the stream ended.
    #[allow(clippy::type_complexity)]
    fn segments_of(pieces: Vec<Piece>) -> (Vec<(Option<u8>, Vec<u8>, usize, [u8; 32])>, bool) {
        let (mut segments, mut ended) = (Vec::new(), false);
        let (mut mode, mut wire, mut plain) = (None, Vec::new(), 0);
        for piece in pieces {
            match piece {
                Piece::Mode(chosen) => mode = Some(chosen),
                Piece::Bytes {
                    wire: bytes,
                    plain: len,
                } => {
                    wire.extend_from_slice(&bytes);
                    plain += len;
                }
                Piece::Value(value) => {
                    segments.push((mode.take(), std::mem::take(&mut wire), plain, value));
                    plain = 0;
                }
                Piece::End => ended = true,
                Piece::Changed | Piece::Failed => panic!("the source is unchanged"),
            }
        }
        (segments, ended)
    }

    /// Segments start at the offset, each followed by its own value, and
    /// their values compose into the BLAKE3 hash of the whole file.
    #[test]
    fn segments_start_at_the_offset_each_with_its_value() {
        let seg = SEGMENT_LEN as usize;
        let whole: Vec<u8> = (0..2 * seg + 9).map(|n| (n * 7) as u8).collect();
        let (path, file) = scratch("whole", &whole);
        let mtime_ms = held_identity(&file).unwrap().1;
        let mut values = Vec::new();
        for offset in [0, SEGMENT_LEN, 2 * SEGMENT_LEN] {
            let source = Source {
                file: file.try_clone().unwrap(),
                size: whole.len() as u64,
                mtime_ms,
            };
            let (segments, ended) = segments_of(pieces_of(source, offset));
            assert!(ended);
            let mut sent = Vec::new();
            for (mode, wire, plain, value) in segments {
                let start = offset as usize + sent.len();
                let segment = decoded(mode.expect("bytes have a mode"), &wire, plain);
                let mut hasher = SegmentHasher::at(start as u64);
                hasher.update(&segment);
                assert_eq!(hasher.finalize(), value);
                if offset == 0 {
                    values.push(value);
                }
                sent.extend_from_slice(&segment);
            }
            assert_eq!(sent, whole[offset as usize..]);
        }
        assert_eq!(
            super::super::hash::root(&values, whole.len() as u64, SEGMENT_LEN),
            blake3::hash(&whole)
        );
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn a_source_that_shrank_is_changed() {
        let (path, file) = scratch("shrank", &[1u8; 100]);
        let source = Source {
            file,
            size: 200,
            mtime_ms: 0,
        };
        assert!(matches!(pieces_of(source, 0).as_slice(), [Piece::Changed]));
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn a_source_with_another_time_is_changed() {
        let (path, file) = scratch("touched", &[1u8; 100]);
        let mtime_ms = held_identity(&file).unwrap().1;
        let source = Source {
            file,
            size: 100,
            mtime_ms: mtime_ms + 1,
        };
        assert!(matches!(
            pieces_of(source, 0).as_slice(),
            [Piece::Mode(_), Piece::Bytes { .. }, Piece::Changed]
        ));
        let _ = std::fs::remove_file(path);
    }

    /// Bytes rewritten while the file is read, its size kept and its time
    /// set back as it was, as a save within the same millisecond leaves it:
    /// what was sent is no longer the file, so it is changed, not ended.
    #[test]
    fn a_source_rewritten_while_read_with_its_time_kept_is_changed() {
        let bytes = vec![3u8; 3 * BLOCK];
        let (path, file) = scratch("rewritten", &bytes);
        let (_, mtime_ms) = held_identity(&file).unwrap();
        let modified = file.metadata().unwrap().modified().unwrap();
        let source = Source {
            file,
            size: bytes.len() as u64,
            mtime_ms,
        };
        let (pieces, mut arriving) = tokio::sync::mpsc::channel(1);
        let reader = std::thread::spawn(move || read_source(source, 0, 0, pieces));
        // The reader waits to hand on its next block: what it sent so far
        // was read before the rewrite.
        assert!(matches!(arriving.blocking_recv(), Some(Piece::Mode(_))));
        assert!(matches!(
            arriving.blocking_recv(),
            Some(Piece::Bytes { .. })
        ));
        {
            use std::io::Write;
            let mut writer = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
            writer.write_all(&[9u8; 16]).unwrap();
            writer
                .set_times(std::fs::FileTimes::new().set_modified(modified))
                .unwrap();
        }
        let rest: Vec<Piece> = std::iter::from_fn(|| arriving.blocking_recv()).collect();
        reader.join().unwrap();
        assert!(
            matches!(rest.last(), Some(Piece::Changed)),
            "a file rewritten while it was read is not sent as whole"
        );
        let _ = std::fs::remove_file(path);
    }

    /// Held bytes are read for the segment's value but not sent: the rest
    /// of the segment follows, then the value of the whole segment, then
    /// the next segment. A whole segment held sends its value alone, with
    /// no mode. The held bytes need not end on a block.
    #[test]
    fn held_bytes_count_toward_the_value_but_are_not_sent() {
        let seg = SEGMENT_LEN as usize;
        let whole: Vec<u8> = (0..seg + 9).map(|n| (n * 13) as u8).collect();
        let (path, file) = scratch("held", &whole);
        let mut first = SegmentHasher::at(0);
        first.update(&whole[..seg]);
        let first = first.finalize();
        for held in [1, CHUNK as u64 + 5, BLOCK as u64 + 3, SEGMENT_LEN] {
            let source = Source {
                file: file.try_clone().unwrap(),
                size: whole.len() as u64,
                mtime_ms: 0,
            };
            let (segments, _) = segments_of(held_pieces_of(source, 0, held));
            assert_eq!(segments.len(), 2);
            let (mode, wire, plain, value) = &segments[0];
            assert_eq!(*value, first, "held {held}");
            if held == SEGMENT_LEN {
                assert!(mode.is_none() && wire.is_empty(), "held {held}");
            } else {
                assert_eq!(
                    decoded(mode.unwrap(), wire, *plain),
                    whole[held as usize..seg],
                    "held {held}"
                );
            }
        }
        let _ = std::fs::remove_file(path);
    }

    /// Text goes as compressed blocks, much smaller than the file; noise
    /// goes raw, and a segment decides for itself.
    #[test]
    fn a_segment_goes_compressed_only_when_it_shrinks() {
        let seg = SEGMENT_LEN as usize;
        let mut whole = vec![0u8; seg];
        let mut state = 0x9e37_79b9_7f4a_7c15u64;
        whole.extend((0..seg).map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            (state >> 24) as u8
        }));
        let (path, file) = scratch("modes", &whole);
        let source = Source {
            file,
            size: whole.len() as u64,
            mtime_ms: 0,
        };
        let (segments, _) = segments_of(pieces_of(source, 0));
        let modes: Vec<_> = segments.iter().map(|segment| segment.0).collect();
        assert_eq!(modes, [Some(MODE_ZSTD), Some(MODE_RAW)]);
        assert!(segments[0].1.len() < seg / 100, "zeroes shrink");
        assert_eq!(segments[1].1.len(), seg, "noise is sent as it is");
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn held_bytes_stay_within_the_segment_at_the_offset() {
        let size = 2 * SEGMENT_LEN + 9;
        assert!(holds_within_segment(0, SEGMENT_LEN, size));
        assert!(holds_within_segment(2 * SEGMENT_LEN, 9, size));
        assert!(holds_within_segment(2 * SEGMENT_LEN, 0, 2 * SEGMENT_LEN));
        assert!(!holds_within_segment(2 * SEGMENT_LEN, 1, 2 * SEGMENT_LEN));
        assert!(!holds_within_segment(0, SEGMENT_LEN + 1, size));
        assert!(!holds_within_segment(2 * SEGMENT_LEN, 10, size));
        assert!(!holds_within_segment(5, 0, size));
        assert!(!holds_within_segment(3 * SEGMENT_LEN, 0, size));
    }

    #[test]
    fn a_small_file_is_one_segment() {
        let (path, file) = scratch("small", &[1u8; 100]);
        let source = Source {
            file,
            size: 100,
            mtime_ms: 0,
        };
        assert!(matches!(
            pieces_of(source, 0).as_slice(),
            [
                Piece::Mode(_),
                Piece::Bytes { .. },
                Piece::Value(_),
                Piece::End
            ]
        ));
        let _ = std::fs::remove_file(path);
    }
}
