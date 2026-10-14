//! One encoded H.264 frame. The payload is screen content: do not log it.

use std::fmt;

use crate::error::Error;

use super::MAX_VIDEO_FRAME;

/// Header before the payload: codec, flags, timestamp, source display id,
/// source topology revision.
pub(crate) const VIDEO_HEADER: usize = 26;
use super::VideoCodec;

/// One encoded H.264 frame. Construct it with [`EncodedFrame::h264`].
///
/// Fields stay visible so a caller can replace the payload. [`crate::Core::publish_video`]
/// checks the payload again.
///
/// `display_id`/`revision` name the captured source: the topology display
/// id and the topology revision the producer captured under. Zero means
/// unstamped (synthetic, non-display, or whole-desktop frames that name no
/// single display). Desktop producers always stamp via
/// [`EncodedFrame::with_source`]; publish backstops a zero revision from
/// the live topology but never invents a display id, so viewers map
/// pointer input by attested source id (fencing unknown sources) instead
/// of guessing from picture dimensions.
#[derive(Clone, PartialEq, Eq)]
pub struct EncodedFrame {
    pub codec: VideoCodec,
    pub keyframe: bool,
    pub timestamp_us: u64,
    pub payload: Vec<u8>,
    pub display_id: u64,
    pub revision: u64,
}

impl EncodedFrame {
    pub fn h264(keyframe: bool, timestamp_us: u64, payload: Vec<u8>) -> Result<Self, Error> {
        Self::new(VideoCodec::H264, keyframe, timestamp_us, payload)
    }

    pub fn new(
        codec: VideoCodec,
        keyframe: bool,
        timestamp_us: u64,
        payload: Vec<u8>,
    ) -> Result<Self, Error> {
        validate_payload(&payload)?;
        Ok(Self {
            codec,
            keyframe,
            timestamp_us,
            payload,
            display_id: 0,
            revision: 0,
        })
    }

    /// Stamps the captured source: topology display id and revision.
    pub fn with_source(mut self, display_id: u64, revision: u64) -> Self {
        self.display_id = display_id;
        self.revision = revision;
        self
    }

    pub(crate) fn check(&self) -> Result<(), Error> {
        validate_payload(&self.payload)
    }
}

impl fmt::Debug for EncodedFrame {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("EncodedFrame")
            .field("keyframe", &self.keyframe)
            .field("timestamp_us", &self.timestamp_us)
            .field("display_id", &self.display_id)
            .field("revision", &self.revision)
            .field("payload_len", &self.payload.len())
            .finish()
    }
}

fn validate_payload(payload: &[u8]) -> Result<(), Error> {
    if payload.is_empty() {
        return Err(Error::InvalidSetting("encoded frame is empty"));
    }
    if payload.len() > MAX_VIDEO_FRAME {
        return Err(Error::InvalidSetting("encoded frame is larger than 1 MiB"));
    }
    Ok(())
}
pub(crate) fn check_video_len(len: usize) -> Result<(), Error> {
    if len < VIDEO_HEADER || len > VIDEO_HEADER + MAX_VIDEO_FRAME {
        Err(Error::Malformed)
    } else {
        Ok(())
    }
}

pub(crate) fn encode_video_body(frame: &EncodedFrame) -> Vec<u8> {
    let mut body = Vec::with_capacity(VIDEO_HEADER + frame.payload.len());
    body.push(frame.codec.wire_id());
    body.push(u8::from(frame.keyframe));
    body.extend_from_slice(&frame.timestamp_us.to_be_bytes());
    body.extend_from_slice(&frame.display_id.to_be_bytes());
    body.extend_from_slice(&frame.revision.to_be_bytes());
    body.extend_from_slice(&frame.payload);
    body
}

pub(crate) fn decode_video_body(body: &[u8]) -> Result<EncodedFrame, Error> {
    check_video_len(body.len())?;
    let codec = VideoCodec::from_wire_id(body[0]).ok_or(Error::Unsupported("this video codec"))?;
    if codec.wire_id() != body[0] {
        return Err(Error::Unsupported("this video codec"));
    }
    let timestamp_us = u64::from_be_bytes(body[2..10].try_into().expect("8 bytes"));
    let display_id = u64::from_be_bytes(body[10..18].try_into().expect("8 bytes"));
    let revision = u64::from_be_bytes(body[18..26].try_into().expect("8 bytes"));
    EncodedFrame::new(codec, body[1] & 1 == 1, timestamp_us, body[26..].to_vec())
        .map(|frame| frame.with_source(display_id, revision))
        .map_err(|_| Error::Malformed)
}
