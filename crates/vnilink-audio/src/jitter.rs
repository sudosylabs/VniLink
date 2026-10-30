use crate::{AudioPacket, Error, Format, FRAME_SAMPLES, SAMPLE_RATE};
use std::collections::BTreeMap;

/// Packets kept waiting to be decoded: 60 ms. Sound arrives on a reliable,
/// ordered stream, so more waiting only ever means playback is behind.
const MAX_PACKETS: usize = 3;
/// Packets behind the last one decoded beyond which the sender restarted
/// its count: one second.
const RESTART_DISTANCE: u64 = 50;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PlaybackStats {
    pub received: u64,
    /// Packets older than one already decoded, or seen before.
    pub late: u64,
    pub duplicate: u64,
    /// Waiting packets dropped, oldest first, so playback catches up.
    pub overflow: u64,
    /// Places where the sequence skipped: the host or this side dropped sound.
    pub discontinuities: u64,
    pub queued: usize,
}
pub struct PlaybackBlock {
    pub timestamp: u64,
    pub samples: Vec<f32>,
}
/// Decodes packets as they arrive, oldest first. Pacing belongs to the
/// output, whose device clock plays the blocks; this keeps no clock of its
/// own, so a backlog cannot settle here. At most [`MAX_PACKETS`] wait.
pub struct AudioPlayback {
    decoder: opus::Decoder,
    format: Format,
    packets: BTreeMap<u64, AudioPacket>,
    /// The sequence after the last one decoded.
    next: Option<u64>,
    muted: bool,
    stats: PlaybackStats,
}
impl AudioPlayback {
    pub fn new(format: Format) -> Result<Self, Error> {
        Ok(Self {
            decoder: opus::Decoder::new(SAMPLE_RATE, format.opus())?,
            format,
            packets: BTreeMap::new(),
            next: None,
            muted: false,
            stats: PlaybackStats::default(),
        })
    }
    /// Mute consumes packets and advances decoder state; it does not stop capture.
    pub fn set_muted(&mut self, muted: bool) {
        self.muted = muted;
    }
    pub fn stats(&self) -> PlaybackStats {
        PlaybackStats {
            queued: self.packets.len(),
            ..self.stats
        }
    }
    pub fn push(&mut self, packet: AudioPacket) -> Result<(), Error> {
        packet.validate()?;
        if packet.format != self.format {
            return Err(Error::InvalidFormat);
        }
        if let Some(next) = self.next.filter(|next| packet.sequence < *next) {
            // Far behind can only be a sender that started counting again.
            if next - packet.sequence <= RESTART_DISTANCE {
                self.stats.late += 1;
                return Ok(());
            }
            self.next = None;
            self.packets.clear();
            self.decoder.reset_state()?;
            self.stats.discontinuities += 1;
        }
        if self.packets.contains_key(&packet.sequence) {
            self.stats.duplicate += 1;
            return Ok(());
        }
        self.stats.received += 1;
        self.packets.insert(packet.sequence, packet);
        while self.packets.len() > MAX_PACKETS {
            self.packets.pop_first();
            self.stats.overflow += 1;
        }
        Ok(())
    }
    /// The oldest waiting packet, decoded; `None` while none waits.
    pub fn poll(&mut self) -> Result<Option<PlaybackBlock>, Error> {
        let Some((sequence, packet)) = self.packets.pop_first() else {
            return Ok(None);
        };
        if self.next.is_some_and(|next| sequence != next) {
            self.stats.discontinuities += 1;
        }
        self.next = Some(sequence.checked_add(1).ok_or(Error::ClockExhausted)?);
        let mut samples = vec![0.0; FRAME_SAMPLES * self.format.channels()];
        let decoded = self
            .decoder
            .decode_float(&packet.payload, &mut samples, false)?;
        if decoded != FRAME_SAMPLES {
            return Err(Error::Malformed);
        }
        if self.muted {
            samples.fill(0.0);
        }
        Ok(Some(PlaybackBlock {
            timestamp: sequence
                .checked_mul(FRAME_SAMPLES as u64)
                .ok_or(Error::ClockExhausted)?,
            samples,
        }))
    }
}
