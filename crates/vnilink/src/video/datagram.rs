//! Loss-tolerant video fragments over QUIC datagrams.
//!
//! Each encoded body (codec header plus payload) is split into fragments that
//! fit one datagram. Fragments may arrive out of order or not at all: the
//! reassembler completes a frame only when every fragment arrived recently.
//! Incomplete frames expire, and the existing keyframe-request path recovers
//! the decoder, exactly as it does for queue drops. Wire bytes are screen
//! content: do not log them.

use std::collections::{BTreeMap, VecDeque};
use std::time::{Duration, Instant};

/// Fragment header: frame sequence, fragment index, fragment count.
/// Keyframe and timestamp travel inside the reassembled body, which the
/// existing video-body codec validates.
pub(crate) const FRAG_HEADER: usize = 8;

/// Largest fragment count accepted. A 1 MiB frame over ~240-byte payloads
/// needs about four thousand fragments; anything larger is malformed.
const MAX_FRAGMENTS: u16 = 8192;

/// Incomplete frames older than this never complete: the decoder has already
/// moved on and a keyframe request is in flight.
const FRAG_TTL: Duration = Duration::from_millis(500);

/// At most this many incomplete frames are tracked; older sequences are
/// evicted first. Bounds memory to a few megabytes even under attack.
const MAX_PARTIAL_FRAMES: usize = 8;

/// Recently completed sequences. Straggler fragments for these are ignored
/// instead of reopening a frame the decoder already consumed.
const COMPLETED_MEMORY: usize = 16;

/// Split `body` into datagram payloads of at most `max_datagram` bytes.
/// `seq` identifies the frame; the caller increments it per frame.
pub(crate) fn fragment(body: &[u8], seq: u32, max_datagram: usize) -> Vec<Vec<u8>> {
    let per = max_datagram.saturating_sub(FRAG_HEADER).max(64);
    let count = body
        .len()
        .div_ceil(per)
        .max(1)
        .min(usize::from(MAX_FRAGMENTS));
    let mut out = Vec::with_capacity(count);
    for (index, chunk) in body.chunks(per).enumerate() {
        let mut frag = Vec::with_capacity(FRAG_HEADER + chunk.len());
        frag.extend_from_slice(&seq.to_be_bytes());
        frag.extend_from_slice(&(index as u16).to_be_bytes());
        frag.extend_from_slice(&(count as u16).to_be_bytes());
        frag.extend_from_slice(chunk);
        out.push(frag);
    }
    out
}

struct Partial {
    count: u16,
    parts: Vec<Option<Vec<u8>>>,
    received: usize,
    bytes: usize,
    since: Instant,
}

/// Reassembles frames from fragments in arrival order.
pub(crate) struct Reassembler {
    partial: BTreeMap<u32, Partial>,
    completed: VecDeque<u32>,
}

impl Reassembler {
    pub(crate) fn new() -> Self {
        Self {
            partial: BTreeMap::new(),
            completed: VecDeque::new(),
        }
    }

    /// Feed one datagram. Returns the complete body when this datagram
    /// finishes a frame, else `None`. Malformed datagrams are ignored.
    pub(crate) fn push(&mut self, datagram: &[u8]) -> Option<Vec<u8>> {
        self.expire();
        if datagram.len() < FRAG_HEADER + 1 {
            return None;
        }
        let seq = u32::from_be_bytes(datagram[0..4].try_into().expect("4 bytes"));
        let index = usize::from(u16::from_be_bytes(
            datagram[4..6].try_into().expect("2 bytes"),
        ));
        let count = u16::from_be_bytes(datagram[6..8].try_into().expect("2 bytes"));
        if count == 0 || count > MAX_FRAGMENTS || index >= usize::from(count) {
            return None;
        }
        if self.completed.contains(&seq) {
            return None;
        }
        let payload = &datagram[FRAG_HEADER..];
        if payload.len() > 64 * 1024 {
            return None;
        }
        if self.partial.len() >= MAX_PARTIAL_FRAMES && !self.partial.contains_key(&seq) {
            if let Some(oldest) = self.partial.keys().next().copied() {
                self.partial.remove(&oldest);
            }
        }
        let partial = self.partial.entry(seq).or_insert_with(|| Partial {
            count,
            parts: vec![None; usize::from(count)],
            received: 0,
            bytes: 0,
            since: Instant::now(),
        });
        if partial.count != count {
            return None;
        }
        if partial.parts[index].is_none() {
            partial.bytes += payload.len();
            partial.parts[index] = Some(payload.to_vec());
            partial.received += 1;
        }
        if partial.received < usize::from(partial.count) {
            return None;
        }
        let partial = self.partial.remove(&seq).expect("present");
        let mut body = Vec::with_capacity(partial.bytes);
        for part in partial.parts {
            body.extend_from_slice(&part.expect("counted"));
        }
        self.completed.push_back(seq);
        while self.completed.len() > COMPLETED_MEMORY {
            self.completed.pop_front();
        }
        Some(body)
    }

    fn expire(&mut self) {
        let now = Instant::now();
        self.partial
            .retain(|_, partial| now.duration_since(partial.since) < FRAG_TTL);
    }
}
