//! Linux hardware encode.
//!
//! VA-API reaches Intel, AMD and the other Mesa drivers: H.264 (High when
//! the session negotiates it and the driver offers it, else Constrained
//! Baseline) and H.265 Main. NVIDIA GPUs are not used yet: see
//! `crate::nvenc`. This build does not link a software H.264
//! encoder: Cisco's OpenH264 patent grant covers Cisco's binary, not a
//! source build compiled here, and x264 is GPL. Other codecs fall back to
//! the shared software encoders ([`vnilink_media::CodecEncoder`]).

#[cfg(target_os = "linux")]
use vnilink_media::bgra_to_nv12;
use vnilink_media::{EncodeParams, EncodedSample, Error, RawFrame, VideoEncoder};

#[cfg(target_os = "linux")]
use crate::vaapi::{Session, VaCodec};

pub struct LinuxEncoder {
    width: u16,
    height: u16,
    #[cfg(target_os = "linux")]
    session: Session,
}

impl LinuxEncoder {
    /// A hardware encoder for `codec`; `high` asks for H.264 High.
    pub fn open(
        codec: vnilink::VideoCodec,
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
            return Err(Error::InvalidFrame("width and height must be even"));
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
            let codec = match codec {
                vnilink::VideoCodec::H264 => VaCodec::H264 { high },
                vnilink::VideoCodec::H265 => VaCodec::H265,
                _ => return Err(Error::Unsupported("VA-API for this codec")),
            };
            let session = Session::open(codec, width, height, frame_rate, bitrate_bps)?;
            return Ok(Self {
                width,
                height,
                session,
            });
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = (codec, high);
            Err(Error::Unsupported("VA-API"))
        }
    }

    /// The hardware encoder for a negotiated codec, else software.
    pub fn open_codec(
        w: u16,
        h: u16,
        fps: u8,
        bps: u32,
        codec: u8,
    ) -> Result<vnilink_media::CodecEncoder<Self>, Error> {
        vnilink_media::CodecEncoder::open(codec, w, h, fps, bps, |kind, w, h, fps, bps| {
            Self::open(kind, w, h, fps, bps, codec == vnilink::CODEC_H264_HIGH)
        })
    }
}

impl VideoEncoder for LinuxEncoder {
    fn implementation(&self) -> vnilink::EncoderImplementation {
        vnilink::EncoderImplementation {
            name: "vaapi",
            hardware: Some(true),
            input: None,
        }
    }

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
            Err(Error::Unsupported("VA-API"))
        }
    }
}
