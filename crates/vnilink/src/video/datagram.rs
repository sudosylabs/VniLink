//! Loss-tolerant video fragments over QUIC datagrams.
//!
//! Each encoded body (codec header plus payload) is split into fragments that
//! fit one datagram. Fragments may arrive out of order or not at all: the
//! reassembler completes a frame only when every fragment arrived recently,
//! and in order. A frame that never completes, or that would reach the
//! decoder after a newer one, is lost: [`Reassembler::take_loss`] says so,
//! and the caller asks for a keyframe, exactly as it does for queue drops.
//! Wire bytes are screen content: do not log them.

use std::collections::{BTreeMap, VecDeque};
use std::time::{Duration, Instant};

/// Fragment header: frame sequence, fragment index, fragment count.
/// Keyframe and timestamp travel inside the reassembled body, which the
/// existing video-body codec validates.
pub(crate) const FRAG_HEADER: usize = 8;

/// Largest fragment count accepted. A 1 MiB frame over ~240-byte payloads
/// needs about four thousand fragments; anything larger is malformed.
const MAX_FRAGMENTS: u16 = 8192;

/// An incomplete frame that has had no fragment for this long never
/// completes: the decoder has already moved on and a keyframe request is in
/// flight. Silence, not age, decides: on a slow link a keyframe can take
/// longer than this to arrive while its fragments keep coming.
const FRAG_SILENCE: Duration = Duration::from_millis(500);

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
    /// When its latest new fragment arrived.
    heard: Instant,
}

/// Frames lost since the last ask, by how each was lost.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Losses {
    /// Unfinished when its fragments stopped coming.
    pub(crate) expired: u64,
    /// Unfinished when newer frames needed its place.
    pub(crate) evicted: u64,
    /// Skipped: a newer frame completed first.
    pub(crate) gap: u64,
    /// Completed after a newer frame, too late to decode.
    pub(crate) stale: u64,
}

impl Losses {
    pub(crate) fn any(&self) -> bool {
        self.expired + self.evicted + self.gap + self.stale > 0
    }
}

/// Reassembles frames from fragments in arrival order.
pub(crate) struct Reassembler {
    partial: BTreeMap<u32, Partial>,
    completed: VecDeque<u32>,
    /// The newest frame handed on; an older one completing later is stale.
    newest: Option<u32>,
    /// Frames lost since [`Self::take_loss`] was last asked.
    lost: Losses,
    /// Recent sequences already counted lost unfinished, so a later gap
    /// over them does not count them twice.
    gone: VecDeque<u32>,
    /// How long an incomplete frame may go without a fragment.
    silence: Duration,
}

impl Reassembler {
    pub(crate) fn new() -> Self {
        Self {
            partial: BTreeMap::new(),
            completed: VecDeque::new(),
            newest: None,
            lost: Losses::default(),
            gone: VecDeque::new(),
            silence: FRAG_SILENCE,
        }
    }

    /// A reassembler whose incomplete frames expire after `silence`, for
    /// tests that cannot wait the real span.
    #[cfg(test)]
    pub(crate) fn with_silence(silence: Duration) -> Self {
        Self {
            silence,
            ..Self::new()
        }
    }

    /// Frames lost since the last call: ones that expired unfinished, were
    /// evicted, were skipped by a newer frame, or arrived after one. After
    /// any, the decoder needs a keyframe before the frames that follow.
    pub(crate) fn take_loss(&mut self) -> Losses {
        std::mem::take(&mut self.lost)
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
                self.lost.evicted += 1;
                self.remember_gone(oldest);
            }
        }
        let partial = self.partial.entry(seq).or_insert_with(|| Partial {
            count,
            parts: vec![None; usize::from(count)],
            received: 0,
            bytes: 0,
            heard: Instant::now(),
        });
        if partial.count != count {
            return None;
        }
        if partial.parts[index].is_none() {
            partial.bytes += payload.len();
            partial.parts[index] = Some(payload.to_vec());
            partial.received += 1;
            partial.heard = Instant::now();
        }
        if partial.received < usize::from(partial.count) {
            return None;
        }
        let partial = self.partial.remove(&seq).expect("present");
        self.completed.push_back(seq);
        while self.completed.len() > COMPLETED_MEMORY {
            self.completed.pop_front();
        }
        // Sequences only grow within a stream: a frame from before the newest
        // would reach the decoder out of order, and one past a gap means the
        // frames between are gone.
        if let Some(newest) = self.newest {
            let ahead = seq.wrapping_sub(newest);
            if ahead == 0 || ahead > u32::MAX / 2 {
                self.lost.stale += 1;
                return None;
            }
            if ahead > 1 {
                // Each skipped frame counts once: not again when it already
                // expired or was evicted unfinished.
                let counted = self
                    .gone
                    .iter()
                    .filter(|&&gone| {
                        let after = gone.wrapping_sub(newest);
                        after > 0 && after < ahead
                    })
                    .count() as u64;
                self.lost.gap += u64::from(ahead - 1).saturating_sub(counted);
                self.partial
                    .retain(|&pending, _| pending.wrapping_sub(seq) <= u32::MAX / 2);
            }
        }
        self.newest = Some(seq);
        let mut body = Vec::with_capacity(partial.bytes);
        for part in partial.parts {
            body.extend_from_slice(&part.expect("counted"));
        }
        Some(body)
    }

    fn expire(&mut self) {
        let now = Instant::now();
        let expired: Vec<u32> = self
            .partial
            .iter()
            .filter(|(_, partial)| now.duration_since(partial.heard) >= self.silence)
            .map(|(&seq, _)| seq)
            .collect();
        for seq in expired {
            self.partial.remove(&seq);
            self.lost.expired += 1;
            self.remember_gone(seq);
        }
    }

    fn remember_gone(&mut self, seq: u32) {
        self.gone.push_back(seq);
        while self.gone.len() > COMPLETED_MEMORY {
            self.gone.pop_front();
        }
    }
}
