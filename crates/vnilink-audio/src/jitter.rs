use crate::{AudioPacket, Error, Format, FRAME_SAMPLES, SAMPLE_RATE};
use std::{
    collections::BTreeMap,
    time::{Duration, Instant},
};

const PERIOD: Duration = Duration::from_millis(20);
const MAX_PACKETS: usize = 16;
const START_DELAY: Duration = Duration::from_millis(60);

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PlaybackStats {
    pub received: u64,
    pub late: u64,
    pub duplicate: u64,
    pub overflow: u64,
    pub concealed: u64,
    pub discontinuities: u64,
    pub queued: usize,
}
pub struct PlaybackBlock {
    pub timestamp: u64,
    pub samples: Vec<f32>,
    pub concealed: bool,
}
/// Caller polls on the playback clock, supplies its local monotonic time, and
/// consumes at most one 20 ms block per call. Never sleeps or allocates a backlog.
pub struct AudioPlayback {
    decoder: opus::Decoder,
    format: Format,
    packets: BTreeMap<u64, AudioPacket>,
    expected: Option<u64>,
    deadline: Option<Instant>,
    missing: u8,
    muted: bool,
    started: bool,
    silent_gaps: bool,
    stats: PlaybackStats,
}
impl AudioPlayback {
    pub fn new(format: Format) -> Result<Self, Error> {
        Ok(Self {
            decoder: opus::Decoder::new(SAMPLE_RATE, format.opus())?,
            format,
            packets: BTreeMap::new(),
            expected: None,
            deadline: None,
            missing: 0,
            muted: false,
            started: false,
            silent_gaps: false,
            stats: PlaybackStats::default(),
        })
    }
    /// System loopback can pause its source clock during silence. Once its queue
    /// drains and concealment ends, anchor resumed packets to a fresh local deadline.
    /// Microphone streams retain their continuously advancing packet clock.
    pub fn allow_silent_gaps(&mut self) {
        self.silent_gaps = true;
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
    pub fn push(&mut self, packet: AudioPacket, now: Instant) -> Result<(), Error> {
        packet.validate()?;
        if packet.format != self.format {
            return Err(Error::InvalidFormat);
        }
        let expected = *self.expected.get_or_insert_with(|| {
            self.deadline = Some(now + START_DELAY);
            packet.sequence
        });
        let expected = if packet.sequence < expected
            && !self.started
            && self
                .packets
                .last_key_value()
                .is_none_or(|(&last, _)| last - packet.sequence < MAX_PACKETS as u64)
        {
            self.expected = Some(packet.sequence);
            packet.sequence
        } else {
            expected
        };
        if packet.sequence < expected {
            self.stats.late += 1;
            return Ok(());
        }
        if packet.sequence - expected >= MAX_PACKETS as u64 || self.packets.len() == MAX_PACKETS {
            self.stats.overflow += 1;
            return Ok(());
        }
        if self.packets.contains_key(&packet.sequence) {
            self.stats.duplicate += 1;
            return Ok(());
        }
        self.stats.received += 1;
        self.packets.insert(packet.sequence, packet);
        Ok(())
    }
    pub fn poll(&mut self, now: Instant) -> Result<Option<PlaybackBlock>, Error> {
        let (Some(mut expected), Some(deadline)) = (self.expected, self.deadline) else {
            return Ok(None);
        };
        if now < deadline {
            return Ok(None);
        }
        self.started = true;
        // A stalled playback worker must not replay seconds of stale sound.
        let skipped = now.duration_since(deadline).as_millis() / PERIOD.as_millis();
        if skipped >= MAX_PACKETS as u128 {
            self.packets.clear();
            self.expected = None;
            self.deadline = None;
            self.missing = 0;
            self.started = false;
            self.decoder.reset_state()?;
            self.stats.discontinuities += 1;
            return Ok(None);
        }
        if skipped > 0 {
            expected = expected
                .checked_add(skipped as u64)
                .ok_or(Error::ClockExhausted)?;
            self.packets.retain(|&sequence, _| sequence >= expected);
            self.decoder.reset_state()?;
            self.stats.discontinuities += 1;
        }
        let timestamp = expected
            .checked_mul(FRAME_SAMPLES as u64)
            .ok_or(Error::ClockExhausted)?;
        self.expected = Some(expected.checked_add(1).ok_or(Error::ClockExhausted)?);
        self.deadline = Some(deadline + PERIOD * (skipped as u32 + 1));
        let packet = self.packets.remove(&expected);
        let concealed = packet.is_none();
        let mut samples = vec![0.0; FRAME_SAMPLES * self.format.channels()];
        if let Some(packet) = packet {
            let decoded = self
                .decoder
                .decode_float(&packet.payload, &mut samples, false)?;
            if decoded != FRAME_SAMPLES {
                return Err(Error::Malformed);
            }
            self.missing = 0;
        } else {
            self.stats.concealed += 1;
            self.missing = self.missing.saturating_add(1);
            if self.missing <= 3 {
                self.decoder.decode_float(&[], &mut samples, false)?;
            } else if self.missing == 4 {
                self.decoder.reset_state()?;
                if self.silent_gaps && self.packets.is_empty() {
                    self.expected = None;
                    self.deadline = None;
                    self.started = false;
                    self.missing = 0;
                    self.stats.discontinuities += 1;
                }
            }
        }
        if self.muted {
            samples.fill(0.0);
        }
        Ok(Some(PlaybackBlock {
            timestamp,
            samples,
            concealed,
        }))
    }
}
