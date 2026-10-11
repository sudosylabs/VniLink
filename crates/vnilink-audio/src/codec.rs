use crate::{AudioPacket, Error};
pub const SAMPLE_RATE: u32 = 48_000;
/// Every packet is exactly 20 ms, in samples per channel.
pub const FRAME_SAMPLES: usize = 960;
pub(crate) const MAX_PAYLOAD: usize = 1200;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Mono,
    Stereo,
}
impl Format {
    pub fn channels(self) -> usize {
        match self {
            Self::Mono => 1,
            Self::Stereo => 2,
        }
    }
    pub(crate) fn opus(self) -> opus::Channels {
        match self {
            Self::Mono => opus::Channels::Mono,
            Self::Stereo => opus::Channels::Stereo,
        }
    }
}

pub struct AudioEncoder {
    encoder: opus::Encoder,
    format: Format,
    sequence: u64,
}
impl AudioEncoder {
    /// Source PCM must already be converted to finite interleaved 48 kHz.
    /// A new encoder belongs to a new owner/stream epoch, never a reconnect.
    pub fn new(format: Format, voice: bool) -> Result<Self, Error> {
        let mut encoder = opus::Encoder::new(
            SAMPLE_RATE,
            format.opus(),
            if voice {
                opus::Application::Voip
            } else {
                opus::Application::Audio
            },
        )?;
        encoder.set_bitrate(opus::Bitrate::Bits(if voice { 32_000 } else { 96_000 }))?;
        encoder.set_vbr(true)?;
        encoder.set_vbr_constraint(true)?;
        encoder.set_inband_fec(voice)?;
        encoder.set_packet_loss_perc(5)?;
        Ok(Self {
            encoder,
            format,
            sequence: 0,
        })
    }
    pub fn encode(&mut self, samples: &[f32]) -> Result<AudioPacket, Error> {
        if samples.len() != FRAME_SAMPLES * self.format.channels()
            || samples.iter().any(|v| !v.is_finite() || v.abs() > 1.0)
        {
            return Err(Error::InvalidFormat);
        }
        let timestamp = self
            .sequence
            .checked_mul(FRAME_SAMPLES as u64)
            .ok_or(Error::ClockExhausted)?;
        let next = self.sequence.checked_add(1).ok_or(Error::ClockExhausted)?;
        let mut payload = vec![0; MAX_PAYLOAD];
        let size = self.encoder.encode_float(samples, &mut payload)?;
        payload.truncate(size);
        let packet = AudioPacket {
            sequence: self.sequence,
            timestamp,
            format: self.format,
            payload,
        };
        self.sequence = next;
        Ok(packet)
    }
}
