//! One encoded H.264 frame. The payload is screen content: do not log it.

use std::fmt;

use crate::error::Error;

use super::MAX_VIDEO_FRAME;

/// Header before the payload: codec, flags, timestamp.
pub(crate) const VIDEO_HEADER: usize = 10;
const CODEC_H264: u8 = 1;

/// One encoded H.264 frame. Construct it with [`EncodedFrame::h264`].
///
/// Fields stay visible so a caller can replace the payload. [`crate::Core::publish_video`]
/// checks the payload again.
#[derive(Clone, PartialEq, Eq)]
pub struct EncodedFrame {
    pub keyframe: bool,
    pub timestamp_us: u64,
    pub payload: Vec<u8>,
}

impl EncodedFrame {
    pub fn h264(keyframe: bool, timestamp_us: u64, payload: Vec<u8>) -> Result<Self, Error> {
        validate_payload(&payload)?;
        Ok(Self {
            keyframe,
            timestamp_us,
            payload,
        })
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
    body.push(CODEC_H264);
    body.push(u8::from(frame.keyframe));
    body.extend_from_slice(&frame.timestamp_us.to_be_bytes());
    body.extend_from_slice(&frame.payload);
    body
}

pub(crate) fn decode_video_body(body: &[u8]) -> Result<EncodedFrame, Error> {
    check_video_len(body.len())?;
    if body[0] != CODEC_H264 {
        return Err(Error::Unsupported("this video codec"));
    }
    let timestamp_us = u64::from_be_bytes(body[2..10].try_into().expect("8 bytes"));
    EncodedFrame::h264(body[1] & 1 == 1, timestamp_us, body[10..].to_vec())
        .map_err(|_| Error::Malformed)
}
