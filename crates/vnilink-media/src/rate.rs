//! Encoder policy for one hosted monitor.
//!
//! [`RateController`] owns the session quality and derives the encoder
//! settings from it: the even size that fits the quality limit, the bitrate,
//! and the frame rate. A quality change reopens the encoder only when the
//! size it was opened with no longer matches; a new bitrate or frame rate
//! is applied to the running encoder where it can take one.
//! Congestion signals (send-queue depth, round-trip time, relay versus direct
//! path) plug in here when the transport reports them; the pacing modules
//! keep reading the decided settings through this interface.

use vnilink::CoreEvent;

use crate::error::Error;
use crate::frame::encoded_size;
use crate::Quality;

/// Concrete encoder inputs derived from a [`Quality`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EncoderSettings {
    pub width: u16,
    pub height: u16,
    pub bitrate_bps: u32,
    pub frame_rate: u8,
}

/// What a quality change asks of the encoder.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RateChange {
    /// The encoder settings are unchanged.
    Same,
    /// Same size; a new bitrate or frame rate the encoder may take as it runs.
    Retune,
    /// A new size: the encoder is opened again.
    Reopen,
}

/// Quality state and the reopen rule for one hosted monitor.
pub struct RateController {
    capture_width: u16,
    capture_height: u16,
    quality: Quality,
    settings: EncoderSettings,
}

impl RateController {
    /// Derive the encoder settings for `capture_width` by `capture_height`
    /// at `quality`. A size that rounds below 2 by 2 is rejected.
    pub fn open(capture_width: u16, capture_height: u16, quality: Quality) -> Result<Self, Error> {
        let settings = settings_for(capture_width, capture_height, quality)?;
        Ok(Self {
            capture_width,
            capture_height,
            quality,
            settings,
        })
    }

    pub fn quality(&self) -> Quality {
        self.quality
    }

    pub fn settings(&self) -> EncoderSettings {
        self.settings
    }

    pub fn capture_size(&self) -> (u16, u16) {
        (self.capture_width, self.capture_height)
    }

    pub fn encoded_size(&self) -> (u16, u16) {
        (self.settings.width, self.settings.height)
    }

    /// Apply a core event, and say what the encoder must do to follow
    /// [`Self::settings`]. Events other than a quality update change nothing.
    pub fn observe(&mut self, event: &CoreEvent) -> Result<RateChange, Error> {
        let CoreEvent::QualityUpdated { quality, .. } = event else {
            return Ok(RateChange::Same);
        };
        let settings = settings_for(self.capture_width, self.capture_height, *quality)?;
        self.quality = *quality;
        let previous = std::mem::replace(&mut self.settings, settings);
        Ok(if settings == previous {
            RateChange::Same
        } else if (settings.width, settings.height) == (previous.width, previous.height) {
            RateChange::Retune
        } else {
            RateChange::Reopen
        })
    }
}

fn settings_for(
    capture_width: u16,
    capture_height: u16,
    quality: Quality,
) -> Result<EncoderSettings, Error> {
    let (width, height) = encoded_size(
        capture_width,
        capture_height,
        quality.max_width,
        quality.max_height,
    )?;
    let bitrate_bps = quality
        .max_bitrate_kbps
        .checked_mul(1_000)
        .ok_or(Error::InvalidSetting(
            "bitrate does not fit in bits per second",
        ))?;
    Ok(EncoderSettings {
        width,
        height,
        bitrate_bps,
        frame_rate: quality.max_frame_rate,
    })
}
