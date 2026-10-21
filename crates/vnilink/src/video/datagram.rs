//! Loss-tolerant video fragments over QUIC datagrams.
//!
//! Each encoded body (codec header plus payload) is split into data shards
//! that fit one datagram, followed by parity shards: Reed–Solomon over the
//! data, so that any data-count of a frame's shards rebuild it. Fragments
//! may arrive out of order or not at all. A frame that does not gather
//! enough shards, or that would reach the decoder after a newer one, is
//! lost: [`Reassembler::take_loss`] says so, and the caller asks for a
//! keyframe, exactly as it does for queue drops. How much parity a frame
//! carries is the sender's choice, from what its viewer reports losing.
//! Wire bytes are screen content: do not log them.

use std::collections::{BTreeMap, VecDeque};
use std::time::{Duration, Instant};

/// Fragment header: frame sequence u32, shard index u16 (data shards first,
/// then parity), data shard count u16, parity shard count u16. Keyframe and
/// timestamp travel inside the reassembled body, which the existing
/// video-body codec validates.
pub(crate) const FRAG_HEADER: usize = 10;

/// Each frame's shards begin with its body's length, so the padding that
/// evens out parity-protected shards can be cut off again.
const LEN_PREFIX: usize = 4;

/// Largest data shard count accepted. A 1 MiB frame over ~240-byte payloads
/// needs about four thousand shards; anything larger is malformed.
const MAX_FRAGMENTS: u16 = 8192;

/// Most parity a frame carries, as a share of its data shards.
pub(crate) const MAX_PROTECTION: f64 = 0.5;

/// An incomplete frame that has had no fragment for this long never
/// completes: the decoder has already moved on and a keyframe request is in
/// flight. Silence, not age, decides: on a slow link a keyframe can take
/// longer than this to arrive while its fragments keep coming.
const FRAG_SILENCE: Duration = Duration::from_millis(500);

/// At most this many incomplete frames are tracked; older sequences are
/// evicted first.
const MAX_PARTIAL_FRAMES: usize = 8;

/// Most shard bytes the incomplete frames hold together; older sequences
/// are evicted first. The frame count alone does not bound memory: eight
/// forged frames of 12288 shards each would hold some 140 MB of one-datagram
/// shards. A frame completes once it holds its data count of shards, so an
/// honest one peaks near its body, at most [`super::MAX_VIDEO_FRAME`] and a shard.
pub(crate) const MAX_PARTIAL_BYTES: usize = 8 * 1024 * 1024;

/// Largest shard payload accepted, above any datagram a path carries.
const MAX_SHARD: usize = 64 * 1024;

/// Recently completed sequences. Straggler fragments for these are ignored
/// instead of reopening a frame the decoder already consumed.
const COMPLETED_MEMORY: usize = 16;

/// Parity shards for `data` data shards at `protection`, a share of the
/// data: none at zero, else at least one and at most [`MAX_PROTECTION`].
pub(crate) fn parity_count(data: usize, protection: f64) -> usize {
    if !(protection > 0.0) {
        return 0;
    }
    ((data as f64) * protection.min(MAX_PROTECTION))
        .ceil()
        .max(1.0) as usize
}

/// Split `body` into datagram payloads of at most `max_datagram` bytes:
/// its data shards, then [`parity_count`] parity shards at `protection`.
/// `seq` identifies the frame; the caller increments it per frame.
pub(crate) fn fragment(
    body: &[u8],
    seq: u32,
    max_datagram: usize,
    protection: f64,
) -> Vec<Vec<u8>> {
    // Reed–Solomon shards are even in length.
    let per = max_datagram.saturating_sub(FRAG_HEADER).max(64) & !1;
    let mut framed = Vec::with_capacity(LEN_PREFIX + body.len());
    framed.extend_from_slice(&u32::try_from(body.len()).unwrap_or(u32::MAX).to_be_bytes());
    framed.extend_from_slice(body);
    let mut shards: Vec<Vec<u8>> = framed
        .chunks(per)
        .take(usize::from(MAX_FRAGMENTS))
        .map(<[u8]>::to_vec)
        .collect();
    let data = shards.len();
    let mut parity = parity_count(data, protection);
    if parity > 0 {
        // Parity covers equal shards: the last data shard is padded.
        if let Some(last) = shards.last_mut() {
            last.resize(per, 0);
        }
        match reed_solomon_simd::encode(data, parity, &shards) {
            Ok(recovery) => shards.extend(recovery),
            Err(_) => {
                parity = 0;
            }
        }
    }
    shards
        .into_iter()
        .enumerate()
        .map(|(index, shard)| {
            let mut frag = Vec::with_capacity(FRAG_HEADER + shard.len());
            frag.extend_from_slice(&seq.to_be_bytes());
            frag.extend_from_slice(&(index as u16).to_be_bytes());
            frag.extend_from_slice(&(data as u16).to_be_bytes());
            frag.extend_from_slice(&(parity as u16).to_be_bytes());
            frag.extend_from_slice(&shard);
            frag
        })
        .collect()
}

struct Partial {
    data: u16,
    parity: u16,
    /// Data shards, then parity shards, as they arrive.
    shards: Vec<Option<Vec<u8>>>,
    received: usize,
    data_received: usize,
    /// Payload bytes of the shards held.
    bytes: usize,
    /// When its latest new fragment arrived.
    heard: Instant,
}

impl Partial {
    /// Shards of this frame that never arrived.
    fn missing(&self) -> u64 {
        (usize::from(self.data) + usize::from(self.parity) - self.received) as u64
    }

    fn whole(&self) -> bool {
        self.received >= usize::from(self.data)
    }

    /// The frame's body from its shards, rebuilding missing data shards
    /// from parity, with how many it rebuilt. None when the shards do not
    /// decode or do not hold a body.
    fn assemble(self) -> Option<(Vec<u8>, u64)> {
        let data = usize::from(self.data);
        let mut repaired = 0;
        let mut shards = self.shards;
        if self.data_received < data {
            let parity = usize::from(self.parity);
            let restored = reed_solomon_simd::decode(
                data,
                parity,
                shards[..data]
                    .iter()
                    .enumerate()
                    .filter_map(|(index, shard)| Some((index, shard.as_ref()?))),
                shards[data..]
                    .iter()
                    .enumerate()
                    .filter_map(|(index, shard)| Some((index, shard.as_ref()?))),
            )
            .ok()?;
            for (index, shard) in restored {
                shards[index] = Some(shard);
                repaired += 1;
            }
        }
        let mut framed = Vec::new();
        for shard in shards.into_iter().take(data) {
            framed.extend_from_slice(&shard?);
        }
        let len = u32::from_be_bytes(framed.get(..LEN_PREFIX)?.try_into().ok()?) as usize;
        let body = framed
            .get(LEN_PREFIX..LEN_PREFIX.checked_add(len)?)?
            .to_vec();
        Some((body, repaired))
    }
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

/// Fragments seen since the last ask: arrived, missing from frames that
/// were tracked, and rebuilt from parity.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct FragmentCounts {
    pub(crate) received: u64,
    pub(crate) missing: u64,
    pub(crate) repaired: u64,
}

impl Losses {
    pub(crate) fn any(&self) -> bool {
        self.total() > 0
    }

    pub(crate) fn total(&self) -> u64 {
        self.expired + self.evicted + self.gap + self.stale
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
    /// Fragments seen since [`Self::take_fragments`] was last asked.
    fragments: FragmentCounts,
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
            fragments: FragmentCounts::default(),
        }
    }

    /// Fragments seen since the last call, for the viewer's loss report.
    pub(crate) fn take_fragments(&mut self) -> FragmentCounts {
        std::mem::take(&mut self.fragments)
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
        let data = u16::from_be_bytes(datagram[6..8].try_into().expect("2 bytes"));
        let parity = u16::from_be_bytes(datagram[8..10].try_into().expect("2 bytes"));
        if data == 0
            || data > MAX_FRAGMENTS
            || usize::from(parity) > parity_count(usize::from(data), MAX_PROTECTION)
            || index >= usize::from(data) + usize::from(parity)
        {
            return None;
        }
        if self.completed.contains(&seq) {
            return None;
        }
        let payload = &datagram[FRAG_HEADER..];
        if payload.len() > MAX_SHARD {
            return None;
        }
        if self.partial.len() >= MAX_PARTIAL_FRAMES && !self.partial.contains_key(&seq) {
            if let Some(oldest) = self.partial.keys().next().copied() {
                self.evict(oldest);
            }
        }
        let partial = self.partial.entry(seq).or_insert_with(|| Partial {
            data,
            parity,
            shards: vec![None; usize::from(data) + usize::from(parity)],
            received: 0,
            data_received: 0,
            bytes: 0,
            heard: Instant::now(),
        });
        if (partial.data, partial.parity) != (data, parity) {
            return None;
        }
        if partial.shards[index].is_none() {
            partial.shards[index] = Some(payload.to_vec());
            partial.bytes += payload.len();
            partial.received += 1;
            if index < usize::from(data) {
                partial.data_received += 1;
            }
            partial.heard = Instant::now();
            self.fragments.received += 1;
        }
        if !partial.whole() {
            // Older frames make room first; this one goes last, when it
            // alone is larger than any a sender makes.
            while self.buffered_bytes() > MAX_PARTIAL_BYTES {
                let oldest = self.partial.keys().copied().find(|&other| other != seq);
                self.evict(oldest.unwrap_or(seq));
                if oldest.is_none() {
                    break;
                }
            }
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
                let mut missing = 0;
                self.partial.retain(|&pending, partial| {
                    let keep = pending.wrapping_sub(seq) <= u32::MAX / 2;
                    if !keep {
                        missing += partial.missing();
                    }
                    keep
                });
                self.fragments.missing += missing;
            }
        }
        self.newest = Some(seq);
        // Parity still on its way to a frame already whole is not missed,
        // and a data shard rebuilt from parity was.
        let Some((body, repaired)) = partial.assemble() else {
            // Shards that do not decode to a body: lost like an unfinished frame.
            self.lost.expired += 1;
            return None;
        };
        self.fragments.missing += repaired;
        self.fragments.repaired += repaired;
        Some(body)
    }

    /// Payload bytes the incomplete frames hold.
    pub(crate) fn buffered_bytes(&self) -> usize {
        self.partial.values().map(|partial| partial.bytes).sum()
    }

    /// Drops the incomplete frame `seq` to make room, counting it lost.
    fn evict(&mut self, seq: u32) {
        if let Some(evicted) = self.partial.remove(&seq) {
            self.fragments.missing += evicted.missing();
        }
        self.lost.evicted += 1;
        self.remember_gone(seq);
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
            if let Some(expired) = self.partial.remove(&seq) {
                self.fragments.missing += expired.missing();
            }
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
