use crate::{codec::MAX_PAYLOAD, Error, Format, FRAME_SAMPLES, SAMPLE_RATE};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioPacket {
    pub sequence: u64,
    /// Samples per channel from this owner's source epoch. No wall clock.
    pub timestamp: u64,
    pub format: Format,
    pub payload: Vec<u8>,
}
impl AudioPacket {
    pub fn validate(&self) -> Result<(), Error> {
        if self.payload.is_empty()
            || self.payload.len() > MAX_PAYLOAD
            || self.sequence.checked_mul(FRAME_SAMPLES as u64) != Some(self.timestamp)
            || opus::packet::get_nb_samples(&self.payload, SAMPLE_RATE)? != FRAME_SAMPLES
        {
            return Err(Error::Malformed);
        }
        Ok(())
    }
    /// Fixed 17-byte header. The owner stream authenticates and identifies epoch.
    pub fn encode(&self) -> Result<Vec<u8>, Error> {
        self.validate()?;
        let mut bytes = Vec::with_capacity(17 + self.payload.len());
        bytes.extend_from_slice(&self.sequence.to_be_bytes());
        bytes.extend_from_slice(&self.timestamp.to_be_bytes());
        bytes.push(self.format.channels() as u8);
        bytes.extend_from_slice(&self.payload);
        Ok(bytes)
    }
    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        if !(18..=17 + MAX_PAYLOAD).contains(&bytes.len()) {
            return Err(Error::Malformed);
        }
        let format = match bytes[16] {
            1 => Format::Mono,
            2 => Format::Stereo,
            _ => return Err(Error::Malformed),
        };
        let packet = Self {
            sequence: u64::from_be_bytes(bytes[..8].try_into().unwrap()),
            timestamp: u64::from_be_bytes(bytes[8..16].try_into().unwrap()),
            format,
            payload: bytes[17..].to_vec(),
        };
        packet.validate()?;
        Ok(packet)
    }
}
