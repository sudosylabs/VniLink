//! Audio codec and clock engines, independent of Desktop and native devices.
//! Samples, encoded content and source names must not be logged.
#![forbid(unsafe_code)]

mod codec;
mod jitter;
#[cfg(feature = "native")]
mod native;
mod packet;
mod resample;

pub use codec::{AudioEncoder, Format, FRAME_SAMPLES, SAMPLE_RATE};
pub use jitter::{AudioPlayback, PlaybackBlock, PlaybackStats};
#[cfg(feature = "native")]
pub use native::{audio_devices, AudioDevice, DeviceDirection, NativeCapture, NativeOutput};
pub use packet::AudioPacket;
pub use resample::AudioResampler;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("audio format or sample block is invalid")]
    InvalidFormat,
    #[error("audio packet is malformed")]
    Malformed,
    #[error("audio stream clock is exhausted")]
    ClockExhausted,
    #[error("audio codec failed: {0}")]
    Codec(#[from] opus::Error),
    #[error("audio resampler failed")]
    Resample,
    #[error("native audio device is unavailable")]
    Device,
    #[error("native audio operation failed: {0}")]
    DeviceStage(&'static str),
    #[error("native audio device or callback failed")]
    DeviceFailed,
}

#[cfg(test)]
mod tests;
