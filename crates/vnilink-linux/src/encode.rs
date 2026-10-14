//! Linux H.264 encode.
//!
//! The hardware path is VA-API, High profile when the session negotiates it
//! and the render node offers the entrypoint, else Constrained Baseline.
//! This build does not link a software encoder. Cisco's OpenH264 patent grant
//! covers Cisco's binary, not a source build compiled here, and x264 is GPL.
//! A machine where no DRM render node offers the VA-API entrypoint reports
//! that the hardware encoder is unavailable.

#[cfg(target_os = "linux")]
use vnilink_media::bgra_to_nv12;
use vnilink_media::{EncodeParams, EncodedSample, Error, RawFrame, VideoEncoder};

#[cfg(target_os = "linux")]
use crate::vaapi::Session;

pub struct LinuxEncoder {
    width: u16,
    height: u16,
    #[cfg(target_os = "linux")]
    session: Session,
}

impl LinuxEncoder {
    pub fn open(
        width: u16,
        height: u16,
        frame_rate: u8,
        bitrate_bps: u32,
        high: bool,
    ) -> Result<Self, Error> {
        if width == 0 || height == 0 {
            return Err(Error::InvalidFrame(
                "frame width and height must be non-zero",
            ));
        }
        if width % 2 != 0 || height % 2 != 0 {
            return Err(Error::InvalidFrame("H.264 width and height must be even"));
        }
        if frame_rate == 0 {
            return Err(Error::InvalidSetting("frame rate must be non-zero"));
        }
        if bitrate_bps == 0 {
            return Err(Error::InvalidSetting(
                "bitrate does not fit in bits per second",
            ));
        }
        #[cfg(target_os = "linux")]
        {
            let session = Session::open(width, height, frame_rate, bitrate_bps, high)?;
            return Ok(Self {
                width,
                height,
                session,
            });
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = (width, height, frame_rate, bitrate_bps, high);
            Err(Error::Unsupported("VA-API H.264"))
        }
    }
}

impl VideoEncoder for LinuxEncoder {
    fn encode(&mut self, frame: &RawFrame, params: &EncodeParams) -> Result<EncodedSample, Error> {
        if frame.width() != self.width || frame.height() != self.height {
            return Err(Error::InvalidFrame(
                "the frame size does not match the encoder",
            ));
        }
        #[cfg(target_os = "linux")]
        {
            let nv12 = bgra_to_nv12(frame)?;
            let (y, uv) = nv12.into_planes();
            return self.session.encode(y, uv, params.force_keyframe);
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = params;
            Err(Error::Unsupported("VA-API H.264"))
        }
    }
}

impl LinuxEncoder {
    pub fn open_codec(
        w: u16,
        h: u16,
        fps: u8,
        bps: u32,
        codec: u8,
    ) -> Result<vnilink_media::CodecEncoder<Self>, Error> {
        vnilink_media::CodecEncoder::open(codec, w, h, fps, bps, |w, h, fps, bps| {
            Self::open(w, h, fps, bps, codec == vnilink::CODEC_H264_HIGH)
        })
    }
}
