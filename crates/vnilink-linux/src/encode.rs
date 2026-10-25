//! Linux hardware encode.
//!
//! NVENC reaches NVIDIA GPUs: H.264, H.265 Main and AV1 Main. VA-API reaches
//! Intel, AMD and the other Mesa drivers: H.264 (High when the session
//! negotiates it and the driver offers it, else Constrained Baseline) and
//! H.265 Main. Encoders are tried on the GPU the compositor draws the
//! capture on first, then on the others ([`crate::gpu`]); on an NVIDIA GPU
//! NVENC comes before VA-API. The GPU converts the capture to NV12 on NVENC
//! and where a VA-API node has a video processor; elsewhere the CPU does.
//! This build does not link a software H.264 encoder: Cisco's OpenH264 patent grant covers Cisco's binary, not a
//! source build compiled here, and x264 is GPL. Other codecs fall back to
//! the shared software encoders ([`vnilink_media::CodecEncoder`]).

#[cfg(target_os = "linux")]
use vnilink_media::bgra_to_nv12;
use vnilink_media::{EncodeParams, EncodedSample, Error, RawFrame, VideoEncoder};

#[cfg(target_os = "linux")]
use crate::gpu::EncoderChoice;
#[cfg(target_os = "linux")]
use crate::vaapi::{Input, Session, VaCodec};

pub struct LinuxEncoder {
    width: u16,
    height: u16,
    #[cfg(target_os = "linux")]
    backend: Backend,
}

#[cfg(target_os = "linux")]
enum Backend {
    Nvenc(crate::nvenc::Encoder),
    Vaapi(Session),
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
            let va_codec = match codec {
                vnilink::VideoCodec::H264 => Some(VaCodec::H264 { high }),
                vnilink::VideoCodec::H265 => Some(VaCodec::H265),
                _ => None,
            };
            let mut failure = Error::Unsupported("a hardware encoder for this codec");
            for choice in crate::gpu::encoders(&crate::gpu::inventory()) {
                // A GPU without this codec or these settings leaves it to
                // the next.
                let opened = match choice {
                    EncoderChoice::Nvenc { pci_address } => crate::nvenc::Encoder::open(
                        pci_address.as_deref(),
                        codec,
                        width,
                        height,
                        frame_rate,
                        bitrate_bps,
                        high,
                    )
                    .map(Backend::Nvenc),
                    EncoderChoice::Vaapi { render_node } => match va_codec {
                        Some(va_codec) => Session::open(
                            &render_node,
                            va_codec,
                            width,
                            height,
                            frame_rate,
                            bitrate_bps,
                        )
                        .map(Backend::Vaapi),
                        None => continue,
                    },
                };
                match opened {
                    Ok(backend) => {
                        return Ok(Self {
                            width,
                            height,
                            backend,
                        })
                    }
                    Err(error) => failure = error,
                }
            }
            return Err(failure);
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
            #[cfg(target_os = "linux")]
            name: match self.backend {
                Backend::Nvenc(_) => "nvenc",
                Backend::Vaapi(_) => "vaapi",
            },
            #[cfg(not(target_os = "linux"))]
            name: "vaapi",
            hardware: Some(true),
            #[cfg(target_os = "linux")]
            input: Some(match &self.backend {
                Backend::Nvenc(_) => vnilink::EncoderInput::GpuUpload,
                Backend::Vaapi(session) if session.converts() => vnilink::EncoderInput::GpuUpload,
                Backend::Vaapi(_) => vnilink::EncoderInput::Cpu,
            }),
            #[cfg(not(target_os = "linux"))]
            input: None,
        }
    }

    /// Both backends take a new bitrate and frame rate as they run, so a
    /// quality change at the same size neither reopens nor makes a keyframe.
    fn retune(&mut self, bitrate_bps: u32, frame_rate: u8) -> Result<bool, Error> {
        if bitrate_bps == 0 || frame_rate == 0 {
            return Ok(false);
        }
        #[cfg(target_os = "linux")]
        {
            return match &mut self.backend {
                Backend::Nvenc(encoder) => encoder.retune(bitrate_bps, frame_rate),
                Backend::Vaapi(session) => session.retune(bitrate_bps, frame_rate).map(|()| true),
            };
        }
        #[cfg(not(target_os = "linux"))]
        {
            Ok(false)
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
            return match &mut self.backend {
                Backend::Nvenc(encoder) => encoder.encode(frame, params.force_keyframe),
                Backend::Vaapi(session) => {
                    let input = if session.converts() {
                        Input::Bgra(frame.clone())
                    } else {
                        let (y, uv) = bgra_to_nv12(frame)?.into_planes();
                        Input::Nv12 { y, uv }
                    };
                    session.encode(input, params.force_keyframe)
                }
            };
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = params;
            Err(Error::Unsupported("VA-API"))
        }
    }
}
