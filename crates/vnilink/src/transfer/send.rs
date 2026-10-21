//! Sending one held source from a segment boundary, each segment followed
//! by its chaining value.

use std::io::{Read, Seek, SeekFrom};
use std::time::Duration;

use bytes::Bytes;
use iroh::endpoint::{RecvStream, SendStream, VarInt};

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
    /// Pins the path to the opened object while it is read.
    #[cfg(windows)]
    pub(crate) _hold: vnilink_rename::windows::VerifiedSource,
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
        let _ = send.finish();
    }
}

/// What the reader hands the network, in order.
enum Piece {
    Bytes(Bytes),
    /// A segment's chaining value, after its bytes.
    Value([u8; VALUE_LEN]),
    /// Every segment was sent.
    End,
    Changed,
    Failed,
}

/// Sends `source` segment by segment from `offset`, a segment boundary.
/// The file is read on a blocking thread a few chunks ahead of the
/// network, and nothing before `offset` is read. `may_send` is asked every
/// few megabytes; once it says no, the stream is reset, so a cancel or
/// revoke stops a file mid-way. `sent_to` hears how far into the
/// file the stream has taken bytes, and finally that the file was sent. A source that changed while it was read
/// is reset as changed instead of carrying on.
pub(crate) async fn send(
    send: &mut SendStream,
    source: Source,
    offset: u64,
    counters: &TransferCounters,
    mut may_send: impl FnMut() -> bool,
    mut sent_to: impl FnMut(u64, bool),
) {
    if offset > source.size || offset % SEGMENT_LEN != 0 {
        refuse(send, STATUS_DENIED).await;
        return;
    }
    if send.write_all(&[STATUS_SENDING]).await.is_err() {
        return;
    }
    let (pieces, mut arriving) = tokio::sync::mpsc::channel(READ_AHEAD);
    let reader = tokio::task::spawn_blocking(move || read_source(source, offset, pieces));
    let mut sent: u64 = 0;
    let mut position = offset;
    while let Some(piece) = arriving.recv().await {
        match piece {
            Piece::Bytes(bytes) => {
                let len = bytes.len();
                // No deadline on a write: on a slow link a write waits while
                // the stream's window drains, for longer than any fixed time.
                // A receiver that stops reading stops the stream itself, and
                // a peer that is gone ends the connection.
                if send.write_chunk(bytes).await.is_err() {
                    break;
                }
                counters.sent(len);
                position += len as u64;
                sent_to(position, false);
                sent += 1;
                if sent % RECHECK_EVERY == 0 && !may_send() {
                    let _ = send.reset(VarInt::from_u32(0));
                    break;
                }
            }
            Piece::Value(value) => {
                if send.write_all(&value).await.is_err() {
                    break;
                }
            }
            Piece::End => {
                if send.finish().is_ok() {
                    counters.served();
                    sent_to(position, true);
                }
                break;
            }
            Piece::Changed => {
                let _ = send.reset(VarInt::from_u32(RESET_CHANGED));
                break;
            }
            Piece::Failed => {
                let _ = send.reset(VarInt::from_u32(0));
                break;
            }
        }
    }
    // A dropped receiver stops the reader at its next chunk.
    drop(arriving);
    let _ = reader.await;
}

/// Reads the held source from `offset` to its end once, segment by
/// segment. Each segment's value follows its bytes only while the source
/// still has the expected size and time, checked as each segment ends.
fn read_source(mut source: Source, offset: u64, pieces: tokio::sync::mpsc::Sender<Piece>) {
    if source.file.seek(SeekFrom::Start(offset)).is_err() {
        let _ = pieces.blocking_send(Piece::Failed);
        return;
    }
    let unchanged = |file: &std::fs::File| {
        held_identity(file).is_some_and(|(size, mtime_ms)| {
            size == source.size && (source.mtime_ms == 0 || mtime_ms == source.mtime_ms)
        })
    };
    let mut start = offset;
    while start < source.size {
        let end = start + segment_len(start, source.size, SEGMENT_LEN);
        let mut hasher = SegmentHasher::at(start);
        let mut at = start;
        while at < end {
            let mut chunk = vec![0u8; CHUNK.min((end - at) as usize)];
            if let Err(error) = source.file.read_exact(&mut chunk) {
                let piece = if error.kind() == std::io::ErrorKind::UnexpectedEof {
                    Piece::Changed
                } else {
                    Piece::Failed
                };
                let _ = pieces.blocking_send(piece);
                return;
            }
            hasher.update(&chunk);
            at += chunk.len() as u64;
            if pieces
                .blocking_send(Piece::Bytes(Bytes::from(chunk)))
                .is_err()
            {
                return;
            }
        }
        if !unchanged(&source.file) {
            let _ = pieces.blocking_send(Piece::Changed);
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
    let _ = pieces.blocking_send(piece);
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
        let (pieces, mut arriving) = tokio::sync::mpsc::channel(1024);
        read_source(source, offset, pieces);
        std::iter::from_fn(|| arriving.try_recv().ok()).collect()
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
            let mut sent = Vec::new();
            let mut segment = Vec::new();
            let mut ended = false;
            for piece in pieces_of(source, offset) {
                match piece {
                    Piece::Bytes(bytes) => segment.extend_from_slice(&bytes),
                    Piece::Value(value) => {
                        let start = offset as usize + sent.len();
                        let mut hasher = SegmentHasher::at(start as u64);
                        hasher.update(&segment);
                        assert_eq!(hasher.finalize(), value);
                        if offset == 0 {
                            values.push(value);
                        }
                        sent.append(&mut segment);
                    }
                    Piece::End => ended = true,
                    Piece::Changed | Piece::Failed => panic!("the source is unchanged"),
                }
            }
            assert!(ended);
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
            [Piece::Bytes(_), Piece::Changed]
        ));
        let _ = std::fs::remove_file(path);
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
            [Piece::Bytes(_), Piece::Value(_), Piece::End]
        ));
        let _ = std::fs::remove_file(path);
    }
}
