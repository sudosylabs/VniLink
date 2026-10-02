//! Pace raw frames and ask the platform encoder for Annex-B.
//!
//! The first picture, a quality-size change, and [`vnilink::CoreEvent::KeyframeRequested`]
//! all force a keyframe. Frames that arrive faster than the quality frame rate
//! are skipped, unless a keyframe is still owed.

use vnilink::Quality;
use vnilink::{Core, CoreEvent, EncodedFrame, MAX_VIDEO_FRAME};

use crate::annexb::parse_annex_b;
use crate::error::{Error, PublishError};
use crate::frame::{encoded_size, scale_bgra, RawFrame};

/// Limits passed to the encoder for one picture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EncodeParams {
    pub force_keyframe: bool,
    pub bitrate_bps: u32,
    pub frame_rate: u8,
}

/// Annex-B bytes an encoder returns. A keyframe must contain SPS, PPS, and an IDR.
pub struct EncodedSample {
    keyframe: bool,
    annex_b: Vec<u8>,
}

impl EncodedSample {
    pub fn annex_b(keyframe: bool, annex_b: Vec<u8>) -> Result<Self, Error> {
        if annex_b.is_empty() {
            return Err(Error::InvalidFrame("encoded frame is empty"));
        }
        if annex_b.len() > MAX_VIDEO_FRAME {
            return Err(Error::InvalidFrame("encoded frame is larger than 1 MiB"));
        }
        let parsed = parse_annex_b(&annex_b)?;
        let keyframe = keyframe || parsed.has_idr();
        if keyframe && (!parsed.has_sps() || !parsed.has_pps() || !parsed.has_idr()) {
            return Err(Error::EncodeRejected(
                "a keyframe needs SPS, PPS, and an IDR",
            ));
        }
        Ok(Self { keyframe, annex_b })
    }

    pub fn keyframe(&self) -> bool {
        self.keyframe
    }

    pub fn bytes(&self) -> &[u8] {
        &self.annex_b
    }
}

impl std::fmt::Debug for EncodedSample {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("EncodedSample")
            .field("keyframe", &self.keyframe)
            .field("len", &self.annex_b.len())
            .finish()
    }
}

/// Platform H.264 encoder. Implementations must not capture the screen themselves.
pub trait VideoEncoder: Send {
    fn encode(&mut self, frame: &RawFrame, params: &EncodeParams) -> Result<EncodedSample, Error>;
}

/// Platform H.264 decoder. The returned picture is what the UI paints.
pub trait VideoDecoder: Send {
    fn decode(&mut self, frame: &EncodedFrame) -> Result<RawFrame, Error>;
}

/// What the pump did with one raw frame.
#[derive(Debug, PartialEq, Eq)]
pub enum PumpOutput {
    Encoded(EncodedFrame),
    Skipped,
}

/// Applies [`Quality`] and remembers when the next keyframe is required.
pub struct FramePump {
    quality: Quality,
    force_keyframe: bool,
    awaiting_keyframe: bool,
    last_sent: Option<u64>,
}

impl FramePump {
    pub fn new(quality: Quality) -> Self {
        Self {
            quality,
            force_keyframe: false,
            awaiting_keyframe: true,
            last_sent: None,
        }
    }

    pub fn quality(&self) -> Quality {
        self.quality
    }

    pub fn set_quality(&mut self, quality: Quality) {
        let size_changed = quality.max_width != self.quality.max_width
            || quality.max_height != self.quality.max_height;
        self.quality = quality;
        if size_changed {
            self.force_keyframe = true;
            self.awaiting_keyframe = true;
        }
    }

    pub fn request_keyframe(&mut self) {
        self.force_keyframe = true;
    }

    pub fn push(
        &mut self,
        frame: &RawFrame,
        encoder: &mut dyn VideoEncoder,
    ) -> Result<PumpOutput, Error> {
        let (width, height) = encoded_size(
            frame.width(),
            frame.height(),
            self.quality.max_width,
            self.quality.max_height,
        )?;
        let force = self.force_keyframe || self.awaiting_keyframe;
        if !force && !self.due(frame.timestamp_us()) {
            return Ok(PumpOutput::Skipped);
        }
        let scaled;
        let input = if width == frame.width() && height == frame.height() {
            frame
        } else {
            scaled = scale_bgra(frame, width, height)?;
            &scaled
        };
        let params = EncodeParams {
            force_keyframe: force,
            bitrate_bps: self.quality.max_bitrate_kbps.checked_mul(1_000).ok_or(
                Error::InvalidSetting("bitrate does not fit in bits per second"),
            )?,
            frame_rate: self.quality.max_frame_rate,
        };
        let sample = encoder.encode(input, &params)?;
        let keyframe = sample.keyframe();
        if force && !keyframe {
            return Err(Error::EncodeRejected(
                "the encoder did not produce a keyframe",
            ));
        }
        let encoded = EncodedFrame::h264(keyframe, frame.timestamp_us(), sample.annex_b)
            .map_err(|_| Error::InvalidFrame("encoded frame was rejected"))?;
        if keyframe {
            self.force_keyframe = false;
            self.awaiting_keyframe = false;
        }
        self.last_sent = Some(frame.timestamp_us());
        Ok(PumpOutput::Encoded(encoded))
    }

    fn due(&self, timestamp_us: u64) -> bool {
        match self.last_sent {
            None => true,
            Some(previous) if timestamp_us < previous => true,
            Some(previous) => {
                timestamp_us.saturating_sub(previous) >= interval_us(self.quality.max_frame_rate)
            }
        }
    }
}

fn interval_us(frame_rate: u8) -> u64 {
    1_000_000 / u64::from(frame_rate)
}

/// Encoder plus pump. The host calls [`Self::observe`] with core events.
pub struct HostVideo<E> {
    pump: FramePump,
    encoder: E,
}

impl<E: VideoEncoder> HostVideo<E> {
    pub fn new(quality: Quality, encoder: E) -> Self {
        Self {
            pump: FramePump::new(quality),
            encoder,
        }
    }

    pub fn request_keyframe(&mut self) {
        self.pump.request_keyframe();
    }

    pub fn set_quality(&mut self, quality: Quality) {
        self.pump.set_quality(quality);
    }

    pub fn quality(&self) -> Quality {
        self.pump.quality()
    }

    /// A keyframe request forces the next encoded picture.
    /// A quality update replaces the limits the encoder sees.
    pub fn observe(&mut self, event: &CoreEvent) {
        match event {
            CoreEvent::KeyframeRequested { .. } => self.request_keyframe(),
            CoreEvent::QualityUpdated { quality, .. } => self.set_quality(*quality),
            _ => {}
        }
    }

    pub fn push(&mut self, frame: &RawFrame) -> Result<PumpOutput, Error> {
        self.pump.push(frame, &mut self.encoder)
    }

    pub fn publish_to(
        &mut self,
        sink: &mut dyn FrameSink,
        session_id: u64,
        frame: &RawFrame,
    ) -> Result<PumpOutput, PublishError> {
        match self.push(frame)? {
            PumpOutput::Skipped => Ok(PumpOutput::Skipped),
            PumpOutput::Encoded(encoded) => {
                sink.publish_frame(session_id, encoded.clone())?;
                Ok(PumpOutput::Encoded(encoded))
            }
        }
    }
}

/// Where a paced frame is published. [`Core`] is one sink.
pub trait FrameSink {
    fn publish_frame(&mut self, session_id: u64, frame: EncodedFrame)
        -> Result<(), vnilink::Error>;
}

/// Borrows a [`Core`] so [`HostVideo::publish_to`] can call [`Core::publish_video`].
pub struct CoreSink<'a> {
    core: &'a Core,
}

impl<'a> CoreSink<'a> {
    pub fn new(core: &'a Core) -> Self {
        Self { core }
    }
}

impl FrameSink for CoreSink<'_> {
    fn publish_frame(
        &mut self,
        session_id: u64,
        frame: EncodedFrame,
    ) -> Result<(), vnilink::Error> {
        self.core.publish_video(session_id, frame)
    }
}
