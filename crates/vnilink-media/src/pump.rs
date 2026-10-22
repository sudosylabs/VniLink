//! Pace raw frames and ask the platform encoder for Annex-B.
//!
//! The first picture, a quality-size change, and [`vnilink::CoreEvent::KeyframeRequested`]
//! all force a keyframe. Frames that arrive faster than the quality frame rate
//! are skipped, unless a keyframe is still owed.
//!
//! A requested keyframe answers a loss, so it is a recovery keyframe: the
//! encoder may cap its size and let quality refine over the pictures after
//! it. The first picture and a size change still get a full keyframe.

use vnilink::Quality;
use vnilink::{Core, CoreEvent, EncodedFrame, MAX_VIDEO_FRAME};

use crate::annexb::parse_annex_b;
use crate::error::{Error, PublishError};
use crate::frame::{encoded_size, scale_bgra, RawFrame};

/// Limits passed to the encoder for one picture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EncodeParams {
    pub force_keyframe: bool,
    /// The forced keyframe answers a loss: an encoder that can should hold
    /// it to a few pictures' bytes, at lower quality, so that resending it
    /// does not crowd the link. Never set without `force_keyframe`.
    pub recovery: bool,
    pub bitrate_bps: u32,
    pub frame_rate: u8,
}

/// Annex-B bytes an encoder returns. A keyframe must contain SPS, PPS, and an IDR.
pub struct EncodedSample {
    codec: vnilink::VideoCodec,
    keyframe: bool,
    annex_b: Vec<u8>,
}

impl EncodedSample {
    pub fn annex_b(keyframe: bool, annex_b: Vec<u8>) -> Result<Self, Error> {
        if annex_b.is_empty() {
            return Err(Error::InvalidFrame("encoded frame is empty"));
        }
        if annex_b.len() > MAX_VIDEO_FRAME {
            return Err(Error::InvalidFrame(crate::error::FRAME_TOO_LARGE));
        }
        let parsed = parse_annex_b(&annex_b)?;
        let keyframe = keyframe || parsed.has_idr();
        if keyframe && (!parsed.has_sps() || !parsed.has_pps() || !parsed.has_idr()) {
            return Err(Error::EncodeRejected(
                "a keyframe needs SPS, PPS, and an IDR",
            ));
        }
        Ok(Self {
            codec: vnilink::VideoCodec::H264,
            keyframe,
            annex_b,
        })
    }

    pub fn packet(
        codec: vnilink::VideoCodec,
        keyframe: bool,
        bytes: Vec<u8>,
    ) -> Result<Self, Error> {
        if codec == vnilink::VideoCodec::H264 {
            return Self::annex_b(keyframe, bytes);
        }
        if bytes.is_empty() || bytes.len() > MAX_VIDEO_FRAME {
            return Err(Error::InvalidFrame("encoded packet size"));
        }
        Ok(Self {
            codec,
            keyframe,
            annex_b: bytes,
        })
    }
    pub fn codec(&self) -> vnilink::VideoCodec {
        self.codec
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

    /// Takes a new bitrate and frame rate while running, with no keyframe.
    /// `Ok(false)` means this encoder cannot, and is opened again instead.
    fn retune(&mut self, bitrate_bps: u32, frame_rate: u8) -> Result<bool, Error> {
        let _ = (bitrate_bps, frame_rate);
        Ok(false)
    }

    /// Whether pictures of an unchanged screen after a keyframe bring it to
    /// full quality. One that does not gets a full-quality keyframe after a
    /// coarse recovery one instead; see [`REFRESH_AFTER_US`].
    fn refines_still_pictures(&self) -> bool {
        false
    }

    /// Which encoder this is, for diagnostics and for telling the viewer
    /// whether the host encodes in hardware. Asked when it opens.
    fn implementation(&self) -> vnilink::EncoderImplementation {
        vnilink::EncoderImplementation::UNKNOWN
    }
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
    /// Every request since the last keyframe was for recovery.
    recovery: bool,
    awaiting_keyframe: bool,
    last_sent: Option<u64>,
    /// When a full-quality keyframe replaces the latest recovery one.
    refresh_at: Option<u64>,
    /// Until when the unchanged picture is encoded again at the frame rate
    /// after a keyframe, for the encoder to refine it.
    refine_until: Option<u64>,
}

/// How long after a keyframe an unchanged picture is encoded again at the
/// frame rate. A still screen otherwise brings a picture a second, and an
/// encoder refines a coarse keyframe over tens of pictures: VideoToolbox
/// took a recovery keyframe from 15 to 46 dB in two seconds at 30 fps.
pub const REFINE_FOR_US: u64 = 2_000_000;

/// How long after a recovery keyframe, with no further loss, a full-quality
/// keyframe replaces it, for an encoder that does not refine a still
/// picture ([`VideoEncoder::refines_still_pictures`]). Encoders in
/// low-latency mode make keyframes only when asked: the coarse picture
/// would otherwise stay until the next loss.
pub const REFRESH_AFTER_US: u64 = 2_000_000;

impl FramePump {
    pub fn new(quality: Quality) -> Self {
        Self {
            quality,
            force_keyframe: false,
            recovery: false,
            awaiting_keyframe: true,
            last_sent: None,
            refresh_at: None,
            refine_until: None,
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

    /// Forces the next picture as a full-quality keyframe.
    pub fn request_keyframe(&mut self) {
        self.force_keyframe = true;
        self.recovery = false;
    }

    /// Forces the next picture as a recovery keyframe, unless a full one is
    /// already owed.
    pub fn request_recovery_keyframe(&mut self) {
        if !self.force_keyframe {
            self.recovery = true;
        }
        self.force_keyframe = true;
    }

    /// Whether the next encoded picture must be a keyframe.
    pub fn keyframe_pending(&self) -> bool {
        self.force_keyframe || self.awaiting_keyframe
    }

    /// Whether an unchanged picture should still be encoded at the frame
    /// rate, for the encoder to refine the latest keyframe.
    pub fn refining(&self) -> bool {
        self.refine_until.is_some()
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
        if self
            .refine_until
            .is_some_and(|until| frame.timestamp_us() >= until)
        {
            self.refine_until = None;
        }
        if self
            .refresh_at
            .is_some_and(|at| frame.timestamp_us() >= at && !self.force_keyframe)
        {
            self.refresh_at = None;
            self.request_keyframe();
        }
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
            recovery: force && self.recovery && !self.awaiting_keyframe,
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
        let encoded =
            EncodedFrame::new(sample.codec, keyframe, frame.timestamp_us(), sample.annex_b)
                .map_err(|_| Error::InvalidFrame("encoded frame was rejected"))?;
        if keyframe {
            // An encoder that refines gets the unchanged picture again for a
            // while; one that does not gets a full-quality keyframe once the
            // link is quiet.
            let refines = encoder.refines_still_pictures();
            self.refresh_at = (params.recovery && !refines)
                .then(|| frame.timestamp_us().saturating_add(REFRESH_AFTER_US));
            self.refine_until = refines.then(|| frame.timestamp_us().saturating_add(REFINE_FOR_US));
            self.force_keyframe = false;
            self.recovery = false;
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

    pub fn request_recovery_keyframe(&mut self) {
        self.pump.request_recovery_keyframe();
    }

    /// Whether the next encoded picture must be a keyframe.
    pub fn keyframe_pending(&self) -> bool {
        self.pump.keyframe_pending()
    }

    /// See [`FramePump::refining`].
    pub fn refining(&self) -> bool {
        self.pump.refining()
    }

    pub fn set_quality(&mut self, quality: Quality) {
        self.pump.set_quality(quality);
    }

    pub fn quality(&self) -> Quality {
        self.pump.quality()
    }

    /// A keyframe request forces the next encoded picture as a recovery
    /// keyframe: the core asks for one when pictures were lost or dropped.
    /// A quality update replaces the limits the encoder sees.
    pub fn observe(&mut self, event: &CoreEvent) {
        match event {
            CoreEvent::KeyframeRequested { .. } => self.request_recovery_keyframe(),
            CoreEvent::QualityUpdated { quality, .. } => self.set_quality(*quality),
            _ => {}
        }
    }

    pub fn push(&mut self, frame: &RawFrame) -> Result<PumpOutput, Error> {
        self.pump.push(frame, &mut self.encoder)
    }

    /// See [`VideoEncoder::implementation`].
    pub fn implementation(&self) -> vnilink::EncoderImplementation {
        self.encoder.implementation()
    }

    /// The running encoder takes a new bitrate and frame rate; see
    /// [`VideoEncoder::retune`].
    pub fn retune(&mut self, bitrate_bps: u32, frame_rate: u8) -> Result<bool, Error> {
        self.encoder.retune(bitrate_bps, frame_rate)
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
