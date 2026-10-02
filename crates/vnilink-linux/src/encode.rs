//! Linux H.264 encode.
//!
//! The hardware path is VA-API. This build does not link a software encoder.
//! Cisco's OpenH264 patent grant covers Cisco's binary, not a source build
//! compiled here, and x264 is GPL. Until a Linux build opens a VA-API entry
//! point, encode reports that the hardware encoder is unavailable.

use vnilink_media::{EncodeParams, EncodedSample, Error, RawFrame, VideoEncoder};

pub struct LinuxEncoder;

impl LinuxEncoder {
    pub fn open(_width: u16, _height: u16) -> Result<Self, Error> {
        Err(Error::Unsupported("VA-API H.264"))
    }
}

impl VideoEncoder for LinuxEncoder {
    fn encode(
        &mut self,
        _frame: &RawFrame,
        _params: &EncodeParams,
    ) -> Result<EncodedSample, Error> {
        Err(Error::Unsupported("VA-API H.264"))
    }
}
