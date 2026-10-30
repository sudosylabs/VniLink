// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! Loss-tolerant video fragments over QUIC datagrams.
//!
//! Each encoded body (codec header plus payload) is split into data shards
//! that fit one datagram, followed by parity shards: Reed–Solomon over the
//! data, so that any data-count of a frame's shards rebuild it. Fragments
//! may arrive out of order or not at all. The fragment sequence only
//! regroups a frame's shards: whether a picture is missing or late is the
//! picture number's to say, in the body (see `ViewerPictures`). Here a frame
//! that does not gather enough shards is given up, and its fragments are
//! counted for the viewer's loss report. How much parity a frame carries is
//! the sender's choice, from what its viewer reports losing.
//! Wire bytes are screen content: do not log them.

use std::collections::{BTreeMap, VecDeque};
use std::time::{Duration, Instant};

/// Fragment header: frame sequence u32, shard index u16 (data shards first,
/// then parity), data shard count u16, parity shard count u16. The sequence
/// counts the frames sent as datagrams, stills among them, and only groups
/// their shards. Keyframe, timestamp and picture number travel inside the
/// reassembled body, which the video-body codec validates.
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

/// Fewest parity shards a protected frame carries: a small frame of one or
/// two fragments then survives two lost datagrams, as a large one does at
/// the same share. Sunshine keeps the same floor.
const MIN_PARITY: usize = 2;

/// Parity shards for `data` data shards at `protection`, a share of the
/// data: none at zero, else [`MAX_PROTECTION`] at most but never fewer than
/// [`MIN_PARITY`].
pub(crate) fn parity_count(data: usize, protection: f64) -> usize {
    if !(protection > 0.0) {
        return 0;
    }
    (((data as f64) * protection.min(MAX_PROTECTION)).ceil() as usize).max(MIN_PARITY)
}

/// The datagrams [`fragment`] makes of a body of `body` bytes, and their
/// bytes together, without making them: what a frame costs on the wire.
pub(crate) fn fragmented_size(body: usize, max_datagram: usize, protection: f64) -> (usize, usize) {
    let per = max_datagram.saturating_sub(FRAG_HEADER).max(64) & !1;
    let framed = LEN_PREFIX + body;
    let data = framed.div_ceil(per).min(usize::from(MAX_FRAGMENTS));
    let parity = parity_count(data, protection);
    // Unprotected, the last data shard keeps its own length; protected, it
    // is padded to the others'.
    let shard_bytes = if parity > 0 {
        (data + parity) * per
    } else {
        framed.min(data * per)
    };
    (data + parity, shard_bytes + (data + parity) * FRAG_HEADER)
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

/// Unfinished frames given up since the last ask: stills among them, and
/// each stream picture among them is also a gap once a later one arrives.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Unfinished {
    /// Its fragments stopped coming.
    pub(crate) expired: u64,
    /// Newer frames needed its place.
    pub(crate) evicted: u64,
}

impl Unfinished {
    pub(crate) fn any(&self) -> bool {
        self.expired + self.evicted > 0
    }
}

/// How the viewer lost pictures, for diagnostics: frames the reassembler
/// gave up, and stream pictures its numbers say were skipped or late.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Losses {
    /// Unfinished when its fragments stopped coming.
    pub(crate) expired: u64,
    /// Unfinished when newer frames needed its place.
    pub(crate) evicted: u64,
    /// Skipped: a later stream picture arrived first.
    pub(crate) gap: u64,
    /// Arrived after a later stream picture, too late to decode.
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
    /// Stream pictures the decoder did not get. An unfinished frame is
    /// counted here as the gap it leaves, so it counts once.
    pub(crate) fn total(&self) -> u64 {
        self.gap + self.stale
    }
}

/// Reassembles frames from fragments in arrival order.
pub(crate) struct Reassembler {
    partial: BTreeMap<u32, Partial>,
    completed: VecDeque<u32>,
    /// Frames given up since [`Self::take_unfinished`] was last asked.
    unfinished: Unfinished,
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
            unfinished: Unfinished::default(),
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

    /// Frames given up unfinished since the last call. They ask nothing of
    /// the host by themselves: a stream picture among them leaves a gap in
    /// the picture numbers, and a still leaves none.
    pub(crate) fn take_unfinished(&mut self) -> Unfinished {
        std::mem::take(&mut self.unfinished)
    }

    /// Feed one datagram. Returns the complete body when this datagram
    /// finishes a frame, else `None`, in the order frames complete: whether
    /// one came too late is the picture number's to say. Malformed
    /// datagrams are ignored.
    #[cfg(any(test, feature = "fuzzing"))]
    pub(crate) fn push(&mut self, datagram: &[u8]) -> Option<Vec<u8>> {
        self.push_at(datagram, Instant::now())
    }

    /// [`Self::push`] with the time the datagram arrived, so silence can be
    /// stepped through without waiting it out.
    pub(crate) fn push_at(&mut self, datagram: &[u8], now: Instant) -> Option<Vec<u8>> {
        self.expire(now);
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
            heard: now,
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
            partial.heard = now;
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
        // Frames are sent in sequence: an older one still incomplete would
        // complete only after this one, too late to decode. Its missing
        // fragments are counted now; the gap it leaves is the picture
        // number's to report.
        let mut missing = 0;
        self.partial.retain(|&pending, partial| {
            let keep = pending.wrapping_sub(seq) <= u32::MAX / 2;
            if !keep {
                missing += partial.missing();
            }
            keep
        });
        self.fragments.missing += missing;
        // Parity still on its way to a frame already whole is not missed,
        // and a data shard rebuilt from parity was.
        let Some((body, repaired)) = partial.assemble() else {
            // Shards that do not decode to a body: given up like an
            // unfinished frame.
            self.unfinished.expired += 1;
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
        self.unfinished.evicted += 1;
    }

    fn expire(&mut self, now: Instant) {
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
            self.unfinished.expired += 1;
        }
    }
}

#[cfg(test)]
#[path = "datagram_tests.rs"]
mod tests;
