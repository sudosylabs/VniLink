// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! In-process realtime software fallback. No subprocess or whole-frame JSON.
//!
//! Encoders convert to the session's matrix ([`VideoSignal::stream`]) and
//! declare it where the codec can; the decoder shows each picture in the colour its
//! stream declares.
use crate::colour::{declared_signal, VideoSignal, YuvMatrix};
use crate::nv12::{bgra_to_i420, i420_to_bgra};
use crate::{EncodeParams, EncodedSample, Error, RawFrame, VideoDecoder, VideoEncoder};
use shiguredo_aom as aom;
use shiguredo_libvpx as vpx;
use vnilink::{CodecSupport, EncodedFrame, VideoCodec};

pub enum SoftwareEncoder {
    Vpx(vpx::Encoder, VideoCodec, YuvMatrix),
    Av1(aom::Encoder, YuvMatrix),
    Hevc(vnilink_hevc::Encoder, YuvMatrix),
}
impl SoftwareEncoder {
    pub fn open(
        codec: VideoCodec,
        matrix: YuvMatrix,
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
        let signal = VideoSignal::stream(matrix);
        match codec {
            VideoCodec::H265 => Ok(Self::Hevc(
                vnilink_hevc::Encoder::open(
                    width,
                    height,
                    fps,
                    bitrate,
                    threads,
                    vnilink_hevc::Colour {
                        primaries: signal.primaries,
                        transfer: signal.transfer,
                        matrix: signal.matrix,
                        full_range: signal.full_range,
                    },
                )
                .map_err(|_| Error::Unsupported("Kvazaar encoder"))?,
                matrix,
            )),
            VideoCodec::Vp8 | VideoCodec::Vp9 => {
                let profile = if codec == VideoCodec::Vp8 {
                    vpx::CodecConfig::Vp8(Default::default())
                } else {
                    vpx::CodecConfig::Vp9(vpx::Vp9Config {
                        row_mt: true,
                        tune_content: Some(vpx::ContentType::Screen),
                        color_space: Some(match matrix {
                            YuvMatrix::Bt709 => vpx::ColorSpace::Bt709,
                            YuvMatrix::Bt601 => vpx::ColorSpace::Bt601,
                        }),
                        full_range: Some(signal.full_range),
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
                    matrix,
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
                config.color_primaries = Some(signal.primaries.into());
                config.transfer_characteristics = Some(signal.transfer.into());
                config.matrix_coefficients = Some(signal.matrix.into());
                config.color_range = Some(signal.full_range.into());
                Ok(Self::Av1(
                    aom::Encoder::new(config).map_err(|_| Error::Unsupported("libaom encoder"))?,
                    matrix,
                ))
            }
            _ => Err(Error::Unsupported("software codec")),
        }
    }
}
impl VideoEncoder for SoftwareEncoder {
    fn implementation(&self) -> vnilink::EncoderImplementation {
        vnilink::EncoderImplementation {
            name: match self {
                Self::Vpx(..) => "libvpx",
                Self::Av1(..) => "libaom",
                Self::Hevc(..) => "x265",
            },
            hardware: Some(false),
            input: Some(vnilink::EncoderInput::Cpu),
        }
    }
    fn encode(&mut self, frame: &RawFrame, params: &EncodeParams) -> Result<EncodedSample, Error> {
        let matrix = match self {
            Self::Vpx(_, _, matrix) | Self::Av1(_, matrix) | Self::Hevc(_, matrix) => *matrix,
        };
        let (y, u, v) = bgra_to_i420(frame, VideoSignal::stream(matrix))?;
        match self {
            Self::Hevc(encoder, _) => {
                let (key, bytes) = encoder
                    .encode(&y, &u, &v, params.force_keyframe)
                    .map_err(|_| Error::EncodeRejected("Kvazaar frame"))?;
                EncodedSample::packet(VideoCodec::H265, key, bytes)
            }
            Self::Vpx(encoder, codec, _) => {
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
            Self::Av1(encoder, _) => {
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
                    declared_signal(Some(picture.matrix), picture.full_range),
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
                    declared_signal(
                        picture.color_space().map(|space| match space {
                            vpx::ColorSpace::Bt601 => 6,
                            vpx::ColorSpace::Bt709 => 1,
                        }),
                        picture.full_range(),
                    ),
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
                    declared_signal(
                        u8::try_from(picture.matrix_coefficients()).ok(),
                        picture.full_range(),
                    ),
                );
                if decoder.next_frame().is_some() {
                    return Err(Error::DecodeRejected("multiple AV1 pictures"));
                }
                result
            }
        }
    }
}
fn from_i420(
    w: usize,
    h: usize,
    y: &[u8],
    u: &[u8],
    v: &[u8],
    strides: [usize; 3],
    time: u64,
    signal: crate::VideoSignal,
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
    i420_to_bgra(w as u16, h as u16, time, [y, u, v], strides, signal)
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
    let mut encoder = SoftwareEncoder::open(codec, YuvMatrix::Bt709, 64, 64, 30, 500_000)?;
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
    use crate::colour::UNDECLARED_SIGNAL;

    /// A platform encoder that only ever opens; it is never asked to encode.
    struct Platform;
    impl VideoEncoder for Platform {
        fn encode(&mut self, _: &RawFrame, _: &EncodeParams) -> Result<EncodedSample, Error> {
            Err(Error::EncodeRejected("not used"))
        }
    }

    #[test]
    fn the_platform_encoder_takes_any_codec_and_only_h264_has_no_fallback() {
        let opened = |codec: VideoCodec, platform: bool| {
            CodecEncoder::open(
                codec.wire_id(),
                YuvMatrix::Bt709,
                64,
                64,
                30,
                500_000,
                |asked, _, _, _, _| {
                    assert_eq!(asked, codec);
                    if platform {
                        Ok(Platform)
                    } else {
                        Err(Error::Unsupported("platform encoder"))
                    }
                },
            )
        };
        assert!(matches!(
            opened(VideoCodec::H265, true),
            Ok(CodecEncoder::Native(_))
        ));
        assert!(matches!(
            opened(VideoCodec::Av1, false),
            Ok(CodecEncoder::Software(_))
        ));
        assert!(matches!(
            opened(VideoCodec::H264, false),
            Err(Error::Unsupported("platform encoder"))
        ));
    }
    #[test]
    fn hevc_owners_keep_separate_references_and_recover_at_a_forced_keyframe() {
        let mut first =
            SoftwareEncoder::open(VideoCodec::H265, YuvMatrix::Bt709, 64, 64, 30, 500_000).unwrap();
        let mut second =
            SoftwareEncoder::open(VideoCodec::H265, YuvMatrix::Bt709, 128, 64, 30, 500_000)
                .unwrap();
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
            let mut encoder =
                SoftwareEncoder::open(codec, YuvMatrix::Bt709, 64, 64, 30, 500_000).unwrap();
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

    /// Black, white, grey, a mid tone and the primaries come back as they
    /// went in through every software codec, in either matrix: each
    /// converts to the session's colour and the decoder reads that colour
    /// back. VP8 cannot declare BT.601, so its decoder shows BT.709 only.
    #[test]
    fn software_codecs_show_each_colour_as_it_went_in() {
        for (codec, matrix) in [
            VideoCodec::Vp8,
            VideoCodec::Vp9,
            VideoCodec::Av1,
            VideoCodec::H265,
        ]
        .into_iter()
        .flat_map(|codec| [YuvMatrix::Bt709, YuvMatrix::Bt601].map(move |matrix| (codec, matrix)))
        .filter(|(codec, matrix)| !(*codec == VideoCodec::Vp8 && *matrix == YuvMatrix::Bt601))
        {
            for bgra in [
                [200u8, 120, 40, 255],
                [0, 0, 0, 255],
                [255, 255, 255, 255],
                [128, 128, 128, 255],
                [0, 0, 255, 255],
                [0, 255, 0, 255],
                [255, 0, 0, 255],
            ] {
                let mut encoder =
                    SoftwareEncoder::open(codec, matrix, 64, 64, 30, 4_000_000).unwrap();
                let frame = RawFrame::bgra(64, 64, 0, bgra.repeat(4096)).unwrap();
                let params = EncodeParams {
                    force_keyframe: true,
                    recovery: false,
                    bitrate_bps: 4_000_000,
                    frame_rate: 30,
                };
                let sample = encoder.encode(&frame, &params).unwrap();
                let encoded = EncodedFrame::new(codec, true, 0, sample.bytes().to_vec()).unwrap();
                let picture = SoftwareDecoder::open(codec)
                    .unwrap()
                    .decode(&encoded)
                    .unwrap();
                let middle = 32 * picture.stride() + 32 * 4;
                let shown = &picture.pixels()[middle..middle + 3];
                assert!(
                    shown
                        .iter()
                        .zip(&bgra)
                        .all(|(shown, sent)| shown.abs_diff(*sent) <= 3),
                    "{codec:?} {matrix:?} {bgra:?} came back as {shown:?}"
                );
            }
        }
    }

    /// H.265, AV1 and VP9 streams say they are in the session's matrix in
    /// limited range; VP8 has nowhere to say it.
    #[test]
    fn software_encoders_declare_their_colour() {
        for matrix in [YuvMatrix::Bt709, YuvMatrix::Bt601] {
            software_encoders_declare(matrix);
        }
    }

    fn software_encoders_declare(matrix: YuvMatrix) {
        let params = EncodeParams {
            force_keyframe: true,
            recovery: false,
            bitrate_bps: 500_000,
            frame_rate: 30,
        };
        let frame = RawFrame::bgra(64, 64, 0, [200u8, 120, 40, 255].repeat(4096)).unwrap();
        let encode = |codec| {
            SoftwareEncoder::open(codec, matrix, 64, 64, 30, 500_000)
                .unwrap()
                .encode(&frame, &params)
                .unwrap()
        };

        let hevc = encode(VideoCodec::H265);
        let picture = vnilink_hevc::Decoder::open()
            .unwrap()
            .decode(hevc.bytes())
            .unwrap();
        assert_eq!((picture.matrix, picture.full_range), (matrix.code(), false));

        let av1 = encode(VideoCodec::Av1);
        let mut decoder = aom::Decoder::new(aom::DecoderConfig::new()).unwrap();
        decoder.decode(av1.bytes()).unwrap();
        let picture = decoder.next_frame().unwrap();
        assert_eq!(
            (picture.matrix_coefficients(), picture.full_range()),
            (u32::from(matrix.code()), false)
        );

        for (codec, declared) in [
            (
                VideoCodec::Vp9,
                Some(match matrix {
                    YuvMatrix::Bt709 => vpx::ColorSpace::Bt709,
                    YuvMatrix::Bt601 => vpx::ColorSpace::Bt601,
                }),
            ),
            (VideoCodec::Vp8, None),
        ] {
            let sample = encode(codec);
            let mut decoder = vpx::Decoder::new(vpx::DecoderConfig::new(match codec {
                VideoCodec::Vp8 => vpx::DecoderCodec::Vp8,
                _ => vpx::DecoderCodec::Vp9,
            }))
            .unwrap();
            decoder.decode(sample.bytes()).unwrap();
            let picture = decoder.next_frame().unwrap().unwrap();
            assert_eq!(
                (picture.color_space(), picture.full_range()),
                (declared, false),
                "{codec:?}"
            );
        }
    }

    /// A picture is shown in the matrix and range its stream declares; one
    /// that declares none, or a matrix not handled, in the stream's colour.
    #[test]
    fn decoders_follow_the_declared_colour_else_the_stream_colour() {
        use crate::colour::declared_signal;
        use crate::YuvMatrix;

        assert_eq!(
            declared_signal(Some(6), false).yuv_matrix(),
            YuvMatrix::Bt601
        );
        let full = declared_signal(Some(1), true);
        assert!(full.full_range && full.yuv_matrix() == YuvMatrix::Bt709);
        assert_eq!(declared_signal(Some(2), false), UNDECLARED_SIGNAL);
        assert_eq!(declared_signal(None, true), UNDECLARED_SIGNAL);
        assert_eq!(declared_signal(Some(9), false), UNDECLARED_SIGNAL);
    }
}

/// The platform's encoders and the shared software ones behind the same
/// negotiated interface.
pub enum CodecEncoder<N> {
    Native(N),
    Software(SoftwareEncoder),
}
impl<N: VideoEncoder> CodecEncoder<N> {
    /// The platform's encoder for the negotiated codec when it opens one,
    /// else the software encoder. H.264 has no software encoder, so its
    /// platform failure is final; any other codec falls back to software.
    pub fn open(
        codec: u8,
        matrix: YuvMatrix,
        w: u16,
        h: u16,
        fps: u8,
        bps: u32,
        native: impl FnOnce(vnilink::VideoCodec, u16, u16, u8, u32) -> Result<N, Error>,
    ) -> Result<Self, Error> {
        let codec = vnilink::VideoCodec::from_wire_id(codec)
            .ok_or(Error::Unsupported("negotiated codec"))?;
        match native(codec, w, h, fps, bps) {
            Ok(encoder) => Ok(Self::Native(encoder)),
            Err(error) if codec == vnilink::VideoCodec::H264 => {
                tracing::warn!(
                    target: vnilink::trace::ENCODE,
                    failure = vnilink::trace::Failure::failure_name(&error),
                    reason = vnilink::trace::Failure::failure_reason(&error),
                    "no H.264 encoder opened, and H.264 has no software encoder"
                );
                Err(error)
            }
            Err(error) => {
                tracing::info!(
                    target: vnilink::trace::ENCODE,
                    codec = ?codec,
                    failure = vnilink::trace::Failure::failure_name(&error),
                    reason = vnilink::trace::Failure::failure_reason(&error),
                    "this computer's encoder did not open: the software encoder serves instead"
                );
                Ok(Self::Software(SoftwareEncoder::open(
                    codec, matrix, w, h, fps, bps,
                )?))
            }
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
    fn implementation(&self) -> vnilink::EncoderImplementation {
        match self {
            Self::Native(encoder) => encoder.implementation(),
            Self::Software(encoder) => encoder.implementation(),
        }
    }

    fn scales_native(&self) -> bool {
        match self {
            Self::Native(encoder) => encoder.scales_native(),
            Self::Software(encoder) => encoder.scales_native(),
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
