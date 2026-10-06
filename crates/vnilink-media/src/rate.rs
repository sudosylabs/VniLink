//! Encoder policy for one hosted monitor.
//!
//! [`RateController`] owns the session quality and derives the encoder
//! settings from it: the even size that fits the quality limit, the bitrate,
//! and the frame rate. A quality change reopens the encoder only when the
//! size, frame rate, or bitrate it was opened with no longer matches.
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

    /// Apply a core event. Returns true when the encoder must be reopened at
    /// [`Self::settings`]. Events other than a quality update never reopen.
    pub fn observe(&mut self, event: &CoreEvent) -> Result<bool, Error> {
        let CoreEvent::QualityUpdated { quality, .. } = event else {
            return Ok(false);
        };
        let settings = settings_for(self.capture_width, self.capture_height, *quality)?;
        self.quality = *quality;
        if settings == self.settings {
            return Ok(false);
        }
        self.settings = settings;
        Ok(true)
    }
}

/// Ordered quality ladder for adaptive mode, low to high. Matches the
/// manual presets plus a floor for poor paths. Single source of truth lives
/// in `vnilink::suggest_quality`; this wrapper keeps the media-facing
/// `Congestion` struct beside the encoder.

/// Congestion snapshot the transport reports. `rtt_ms` comes from
/// `Core::session_paths`, `relay` from the selected path, `queued` and
/// `dropped_delta` from `Core::video_backpressure` (host) or
/// `VideoSubscription::lagged` delta (controller).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Congestion {
    pub rtt_ms: u64,
    pub relay: bool,
    pub queued: usize,
    pub dropped_delta: u64,
}

/// Suggest one ladder step down when the path struggles, one step up when it
/// is clean, else `None`. Single policy lives in `vnilink::suggest_quality`.
pub fn suggest_quality(current: Quality, congestion: Congestion) -> Option<Quality> {
    vnilink::suggest_quality(
        &current,
        congestion.rtt_ms,
        congestion.relay,
        congestion.queued,
        congestion.dropped_delta,
    )
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
