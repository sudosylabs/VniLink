//! In-process realtime software fallback. No subprocess or whole-frame JSON.
use crate::{EncodeParams, EncodedSample, Error, RawFrame, VideoDecoder, VideoEncoder};
use shiguredo_aom as aom;
use shiguredo_libvpx as vpx;
use vnilink::{CodecSupport, EncodedFrame, VideoCodec};

pub enum SoftwareEncoder {
    Vpx(vpx::Encoder, VideoCodec),
    Av1(aom::Encoder),
    Hevc(vnilink_hevc::Encoder),
}
impl SoftwareEncoder {
    pub fn open(
        codec: VideoCodec,
        width: u16,
        height: u16,
        fps: u8,
        bitrate: u32,
    ) -> Result<Self, Error> {
        if width == 0
            || height == 0
            || width > 4096
            || height > 4096
            || width % 2 != 0
            || height % 2 != 0
            || fps == 0
            || fps > 60
            || bitrate == 0
        {
            return Err(Error::InvalidSetting("software encoder limits"));
        }
        let threads = std::thread::available_parallelism()
            .map(|n| n.get().min(8))
            .unwrap_or(1);
        match codec {
            VideoCodec::H265 => Ok(Self::Hevc(
                vnilink_hevc::Encoder::open(width, height, fps, bitrate, threads)
                    .map_err(|_| Error::Unsupported("Kvazaar encoder"))?,
            )),
            VideoCodec::Vp8 | VideoCodec::Vp9 => {
                let profile = if codec == VideoCodec::Vp8 {
                    vpx::CodecConfig::Vp8(Default::default())
                } else {
                    vpx::CodecConfig::Vp9(vpx::Vp9Config {
                        row_mt: true,
                        tune_content: Some(vpx::ContentType::Screen),
                        ..Default::default()
                    })
                };
                let mut config = vpx::EncoderConfig::new(
                    width as usize,
                    height as usize,
                    vpx::ImageFormat::I420,
                    profile,
                );
                config.fps_numerator = fps as usize;
                config.target_bitrate = bitrate as usize;
                config.deadline = vpx::EncodingDeadline::Realtime;
                config.lag_in_frames = Some(0);
                config.cpu_used = Some(if codec == VideoCodec::Vp8 { 8 } else { 6 });
                config.rate_control = vpx::RateControlMode::Cbr;
                config.error_resilient = true;
                config.threads = std::num::NonZeroUsize::new(threads);
                Ok(Self::Vpx(
                    vpx::Encoder::new(config).map_err(|_| Error::Unsupported("libvpx encoder"))?,
                    codec,
                ))
            }
            VideoCodec::Av1 => {
                let mut config =
                    aom::EncoderConfig::new(width.into(), height.into(), aom::ImageFormat::I420);
                config.g_usage = aom::Usage::Realtime;
                config.g_timebase = aom::AomRational {
                    num: 1,
                    den: fps.into(),
                };
                config.g_threads = Some(threads as u32);
                config.g_lag_in_frames = Some(0);
                config.cpu_used = Some(10);
                config.rc_end_usage = aom::RateControlMode::Cbr;
                config.rc_target_bitrate = (bitrate / 1000).max(1);
                config.tune_content = Some(aom::ContentType::Screen);
                Ok(Self::Av1(
                    aom::Encoder::new(config).map_err(|_| Error::Unsupported("libaom encoder"))?,
                ))
            }
            _ => Err(Error::Unsupported("software codec")),
        }
    }
}
impl VideoEncoder for SoftwareEncoder {
    fn encode(&mut self, frame: &RawFrame, params: &EncodeParams) -> Result<EncodedSample, Error> {
        let (y, u, v) = to_i420(frame)?;
        match self {
            Self::Hevc(encoder) => {
                let (key, bytes) = encoder
                    .encode(&y, &u, &v, params.force_keyframe)
                    .map_err(|_| Error::EncodeRejected("Kvazaar frame"))?;
                EncodedSample::packet(VideoCodec::H265, key, bytes)
            }
            Self::Vpx(encoder, codec) => {
                encoder
                    .encode(
                        &vpx::ImageData::I420 {
                            y: &y,
                            u: &u,
                            v: &v,
                        },
                        &vpx::EncodeOptions {
                            force_keyframe: params.force_keyframe,
                        },
                    )
                    .map_err(|_| Error::EncodeRejected("libvpx frame"))?;
                let packet = encoder
                    .next_frame()
                    .ok_or(Error::EncodeRejected("libvpx produced no frame"))?;
                // Codec is known from the encoder; store it beside the opaque native owner.

                let sample =
                    EncodedSample::packet(*codec, packet.is_keyframe(), packet.data().to_vec())?;
                if encoder.next_frame().is_some() {
                    return Err(Error::EncodeRejected("unexpected delayed VPX frame"));
                }
                Ok(sample)
            }
            Self::Av1(encoder) => {
                encoder
                    .encode(
                        &aom::ImageData::I420 {
                            y: &y,
                            u: &u,
                            v: &v,
                        },
                        &aom::EncodeOptions {
                            force_keyframe: params.force_keyframe,
                        },
                    )
                    .map_err(|_| Error::EncodeRejected("libaom frame"))?;
                let packet = encoder
                    .next_frame()
                    .ok_or(Error::EncodeRejected("libaom produced no frame"))?;
                let sample = EncodedSample::packet(
                    VideoCodec::Av1,
                    packet.is_keyframe(),
                    packet
                        .data()
                        .map_err(|_| Error::EncodeRejected("libaom packet"))?
                        .to_vec(),
                )?;
                if encoder.next_frame().is_some() {
                    return Err(Error::EncodeRejected("unexpected delayed AV1 frame"));
                }
                Ok(sample)
            }
        }
    }
}
pub enum SoftwareDecoder {
    Vpx(vpx::Decoder),
    Av1(aom::Decoder),
    Hevc(vnilink_hevc::Decoder),
}
impl SoftwareDecoder {
    pub fn open(codec: VideoCodec) -> Result<Self, Error> {
        match codec {
            VideoCodec::H265 => Ok(Self::Hevc(
                vnilink_hevc::Decoder::open()
                    .map_err(|_| Error::Unsupported("libde265 decoder"))?,
            )),
            VideoCodec::Vp8 | VideoCodec::Vp9 => Ok(Self::Vpx(
                vpx::Decoder::new(vpx::DecoderConfig::new(if codec == VideoCodec::Vp8 {
                    vpx::DecoderCodec::Vp8
                } else {
                    vpx::DecoderCodec::Vp9
                }))
                .map_err(|_| Error::Unsupported("libvpx decoder"))?,
            )),
            VideoCodec::Av1 => Ok(Self::Av1(
                aom::Decoder::new(aom::DecoderConfig::new())
                    .map_err(|_| Error::Unsupported("libaom decoder"))?,
            )),
            _ => Err(Error::Unsupported("software codec")),
        }
    }
}
impl VideoDecoder for SoftwareDecoder {
    fn decode(&mut self, frame: &EncodedFrame) -> Result<RawFrame, Error> {
        if frame.payload.is_empty() || frame.payload.len() > vnilink::MAX_VIDEO_FRAME {
            return Err(Error::DecodeRejected("software packet size"));
        }
        match self {
            Self::Hevc(decoder) => {
                let picture = decoder
                    .decode(&frame.payload)
                    .map_err(|_| Error::DecodeRejected("libde265 frame"))?;
                let y = picture.width * picture.height;
                let uv = y / 4;
                from_i420(
                    picture.width,
                    picture.height,
                    &picture.i420[..y],
                    &picture.i420[y..y + uv],
                    &picture.i420[y + uv..],
                    [picture.width, picture.width / 2, picture.width / 2],
                    frame.timestamp_us,
                )
            }
            Self::Vpx(decoder) => {
                decoder
                    .decode(&frame.payload)
                    .map_err(|_| Error::DecodeRejected("libvpx bitstream"))?;
                let picture = decoder
                    .next_frame()
                    .map_err(|_| Error::DecodeRejected("libvpx picture"))?
                    .ok_or(Error::DecodeRejected("libvpx produced no frame"))?;
                if picture.is_high_depth() {
                    return Err(Error::DecodeRejected("unsupported sample depth"));
                }
                let result = from_i420(
                    picture.width(),
                    picture.height(),
                    picture.y_plane(),
                    picture.u_plane(),
                    picture.v_plane(),
                    [picture.y_stride(), picture.u_stride(), picture.v_stride()],
                    frame.timestamp_us,
                );
                if decoder
                    .next_frame()
                    .map_err(|_| Error::DecodeRejected("libvpx drain"))?
                    .is_some()
                {
                    return Err(Error::DecodeRejected("multiple VPX pictures"));
                }
                result
            }
            Self::Av1(decoder) => {
                decoder
                    .decode(&frame.payload)
                    .map_err(|_| Error::DecodeRejected("libaom bitstream"))?;
                let picture = decoder
                    .next_frame()
                    .ok_or(Error::DecodeRejected("libaom produced no frame"))?;
                if picture.is_high_depth()
                    || picture
                        .format()
                        .map_err(|_| Error::DecodeRejected("libaom format"))?
                        != aom::ImageFormat::I420
                {
                    return Err(Error::DecodeRejected("unsupported sample format"));
                }
                let invalid = |_| Error::DecodeRejected("libaom plane");
                let result = from_i420(
                    picture.width(),
                    picture.height(),
                    picture.y_plane().map_err(invalid)?,
                    picture.u_plane().map_err(invalid)?,
                    picture.v_plane().map_err(invalid)?,
                    [
                        picture.y_stride().map_err(invalid)?,
                        picture.u_stride().map_err(invalid)?,
                        picture.v_stride().map_err(invalid)?,
                    ],
                    frame.timestamp_us,
                );
                if decoder.next_frame().is_some() {
                    return Err(Error::DecodeRejected("multiple AV1 pictures"));
                }
                result
            }
        }
    }
}
fn to_i420(frame: &RawFrame) -> Result<(Vec<u8>, Vec<u8>, Vec<u8>), Error> {
    let w = frame.width() as usize;
    let h = frame.height() as usize;
    if w % 2 != 0 || h % 2 != 0 {
        return Err(Error::InvalidFrame("software YUV dimensions must be even"));
    }
    let mut y = vec![0; w * h];
    let mut u = vec![0; w * h / 4];
    let mut v = vec![0; w * h / 4];
    for row in 0..h {
        for col in 0..w {
            let pixel = &frame.pixels()[row * frame.stride() + col * 4..];
            let (b, g, r) = (pixel[0] as i32, pixel[1] as i32, pixel[2] as i32);
            y[row * w + col] = ((66 * r + 129 * g + 25 * b + 128) / 256 + 16).clamp(0, 255) as u8;
        }
    }
    for row in (0..h).step_by(2) {
        for col in (0..w).step_by(2) {
            let mut rgb = [0i32; 3];
            for dy in 0..2 {
                for dx in 0..2 {
                    let pixel = &frame.pixels()[(row + dy) * frame.stride() + (col + dx) * 4..];
                    for i in 0..3 {
                        rgb[i] += pixel[i] as i32;
                    }
                }
            }
            let (b, g, r) = (rgb[0] / 4, rgb[1] / 4, rgb[2] / 4);
            let index = (row / 2) * (w / 2) + col / 2;
            u[index] = ((-38 * r - 74 * g + 112 * b + 128) / 256 + 128).clamp(0, 255) as u8;
            v[index] = ((112 * r - 94 * g - 18 * b + 128) / 256 + 128).clamp(0, 255) as u8;
        }
    }
    Ok((y, u, v))
}
fn from_i420(
    w: usize,
    h: usize,
    y: &[u8],
    u: &[u8],
    v: &[u8],
    strides: [usize; 3],
    time: u64,
) -> Result<RawFrame, Error> {
    if w == 0 || h == 0 || w > 4096 || h > 4096 || w % 2 != 0 || h % 2 != 0 {
        return Err(Error::DecodeRejected("decoded dimensions"));
    }
    for (plane, stride, width, height) in [
        (y, strides[0], w, h),
        (u, strides[1], w / 2, h / 2),
        (v, strides[2], w / 2, h / 2),
    ] {
        if stride < width
            || stride
                .checked_mul(height - 1)
                .and_then(|n| n.checked_add(width))
                .is_none_or(|n| n > plane.len())
        {
            return Err(Error::DecodeRejected("decoded plane bounds"));
        }
    }
    let mut pixels = vec![0; w * h * 4];
    for row in 0..h {
        for col in 0..w {
            let c = (y[row * strides[0] + col] as i32 - 16).max(0);
            let d = u[(row / 2) * strides[1] + col / 2] as i32 - 128;
            let e = v[(row / 2) * strides[2] + col / 2] as i32 - 128;
            let target = &mut pixels[(row * w + col) * 4..];
            target[0] = ((298 * c + 516 * d + 128) >> 8).clamp(0, 255) as u8;
            target[1] = ((298 * c - 100 * d - 208 * e + 128) >> 8).clamp(0, 255) as u8;
            target[2] = ((298 * c + 409 * e + 128) >> 8).clamp(0, 255) as u8;
            target[3] = 255;
        }
    }
    RawFrame::bgra(w as u16, h as u16, time, pixels)
}
/// Only a successful synthetic roundtrip makes a codec available.
pub fn probe_software_codecs() -> Vec<CodecSupport> {
    [
        VideoCodec::Vp8,
        VideoCodec::Vp9,
        VideoCodec::Av1,
        VideoCodec::H265,
    ]
    .into_iter()
    .map(|codec| {
        let success = probe(codec).is_ok();
        CodecSupport {
            codec,
            encode: success,
            decode: success,
            hardware: false,
        }
    })
    .collect()
}
fn probe(codec: VideoCodec) -> Result<(), Error> {
    let frame = RawFrame::bgra(64, 64, 0, [32u8, 64, 128, 255].repeat(4096))?;
    let mut encoder = SoftwareEncoder::open(codec, 64, 64, 30, 500_000)?;
    let sample = encoder.encode(
        &frame,
        &EncodeParams {
            force_keyframe: true,
            recovery: false,
            bitrate_bps: 500_000,
            frame_rate: 30,
        },
    )?;
    if !sample.keyframe() {
        return Err(Error::EncodeRejected("probe needs keyframe"));
    }
    let encoded = EncodedFrame::new(codec, true, 0, sample.bytes().to_vec())
        .map_err(|_| Error::EncodeRejected("probe packet"))?;
    let picture = SoftwareDecoder::open(codec)?.decode(&encoded)?;
    if picture.width() != 64 || picture.height() != 64 {
        return Err(Error::DecodeRejected("probe geometry"));
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn hevc_owners_keep_separate_references_and_recover_at_a_forced_keyframe() {
        let mut first = SoftwareEncoder::open(VideoCodec::H265, 64, 64, 30, 500_000).unwrap();
        let mut second = SoftwareEncoder::open(VideoCodec::H265, 128, 64, 30, 500_000).unwrap();
        let mut decoders = [
            SoftwareDecoder::open(VideoCodec::H265).unwrap(),
            SoftwareDecoder::open(VideoCodec::H265).unwrap(),
        ];
        for index in 0..4 {
            for (owner, (encoder, width, color)) in
                [(&mut first, 64, 64u8), (&mut second, 128, 160u8)]
                    .into_iter()
                    .enumerate()
            {
                let raw = RawFrame::bgra(
                    width,
                    64,
                    index * 33333,
                    [32, 64, color, 255].repeat(width as usize * 64),
                )
                .unwrap();
                let force = index == 0 || index == 3;
                let sample = encoder
                    .encode(
                        &raw,
                        &EncodeParams {
                            force_keyframe: force,
                            recovery: false,
                            bitrate_bps: 500_000,
                            frame_rate: 30,
                        },
                    )
                    .unwrap();
                if force {
                    assert!(sample.keyframe());
                }
                let frame = EncodedFrame::new(
                    VideoCodec::H265,
                    sample.keyframe(),
                    raw.timestamp_us(),
                    sample.bytes().to_vec(),
                )
                .unwrap();
                let picture = decoders[owner].decode(&frame).unwrap();
                assert_eq!((picture.width(), picture.height()), (width, 64));
                assert!(picture.pixels()[2].abs_diff(color) < 20);
                if index == 2 {
                    let bad =
                        EncodedFrame::new(VideoCodec::H265, true, 0, vec![0, 0, 0, 1, 0xff, 0xff])
                            .unwrap();
                    assert!(decoders[owner].decode(&bad).is_err());
                }
            }
        }
    }
    #[test]
    fn all_software_codecs_process_real_keyframes_and_predicted_frames() {
        for codec in [
            VideoCodec::Vp8,
            VideoCodec::Vp9,
            VideoCodec::Av1,
            VideoCodec::H265,
        ] {
            probe(codec).unwrap();
            let mut encoder = SoftwareEncoder::open(codec, 64, 64, 30, 500_000).unwrap();
            let mut decoder = SoftwareDecoder::open(codec).unwrap();
            for i in 0..3 {
                let frame =
                    RawFrame::bgra(64, 64, i * 33333, [32u8, 64, 128, 255].repeat(4096)).unwrap();
                let sample = encoder
                    .encode(
                        &frame,
                        &EncodeParams {
                            force_keyframe: i == 0,
                            recovery: false,
                            bitrate_bps: 500_000,
                            frame_rate: 30,
                        },
                    )
                    .unwrap();
                assert_eq!(sample.codec(), codec);
                let encoded = EncodedFrame::new(
                    codec,
                    sample.keyframe(),
                    frame.timestamp_us(),
                    sample.bytes().to_vec(),
                )
                .unwrap();
                let picture = decoder.decode(&encoded).unwrap();
                assert_eq!((picture.width(), picture.height()), (64, 64));
                assert_eq!(picture.timestamp_us(), frame.timestamp_us());
                assert!(picture.pixels()[2].abs_diff(128) < 20);
            }
        }
    }
}

/// Native H.264 and shared software encoders behind the same negotiated interface.
pub enum CodecEncoder<N> {
    Native(N),
    Software(SoftwareEncoder),
}
impl<N: VideoEncoder> CodecEncoder<N> {
    pub fn open(
        codec: u8,
        w: u16,
        h: u16,
        fps: u8,
        bps: u32,
        native: impl FnOnce(u16, u16, u8, u32) -> Result<N, Error>,
    ) -> Result<Self, Error> {
        let codec = vnilink::VideoCodec::from_wire_id(codec)
            .ok_or(Error::Unsupported("negotiated codec"))?;
        if codec == vnilink::VideoCodec::H264 {
            Ok(Self::Native(native(w, h, fps, bps)?))
        } else {
            Ok(Self::Software(SoftwareEncoder::open(
                codec, w, h, fps, bps,
            )?))
        }
    }
}
impl<N: VideoEncoder> VideoEncoder for CodecEncoder<N> {
    fn encode(&mut self, frame: &RawFrame, params: &EncodeParams) -> Result<EncodedSample, Error> {
        match self {
            Self::Native(encoder) => encoder.encode(frame, params),
            Self::Software(encoder) => encoder.encode(frame, params),
        }
    }

    fn retune(&mut self, bitrate_bps: u32, frame_rate: u8) -> Result<bool, Error> {
        match self {
            Self::Native(encoder) => encoder.retune(bitrate_bps, frame_rate),
            Self::Software(encoder) => encoder.retune(bitrate_bps, frame_rate),
        }
    }

    fn refines_still_pictures(&self) -> bool {
        match self {
            Self::Native(encoder) => encoder.refines_still_pictures(),
            Self::Software(encoder) => encoder.refines_still_pictures(),
        }
    }
}
#[derive(Default)]
pub struct SoftwareDecoderSlot(Option<(VideoCodec, SoftwareDecoder)>);
impl SoftwareDecoderSlot {
    pub fn decode(&mut self, frame: &EncodedFrame) -> Option<Result<RawFrame, Error>> {
        if frame.codec == VideoCodec::H264 {
            return None;
        }
        Some((|| {
            if self
                .0
                .as_ref()
                .is_none_or(|(codec, _)| *codec != frame.codec)
            {
                self.0 = Some((frame.codec, SoftwareDecoder::open(frame.codec)?));
            }
            self.0.as_mut().unwrap().1.decode(frame)
        })())
    }
}
