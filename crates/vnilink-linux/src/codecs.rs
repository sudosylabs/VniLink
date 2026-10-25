//! What this machine encodes in hardware, for session negotiation.
//!
//! H.264 is offered when NVENC opens a session for it or a render node has
//! the entrypoint, on any GPU; its encoders are exercised by every session.
//! The device reported is the first that offers it in the order sessions
//! try encoders ([`crate::gpu::encoders`]). H.265 and
//! AV1 are offered only after a round trip: a test picture encoded on the
//! GPU must decode, in the right colour, with the shared software decoder.
//! A driver whose output does not decode is never chosen over software.

use vnilink::{CodecSupport, VideoCodec, VideoCodecCaps};
use vnilink_media::{
    EncodeParams, EncodedSample, Error, RawFrame, SoftwareDecoder, VideoDecoder, VideoEncoder,
};

use crate::encode::LinuxEncoder;
use crate::gpu::{self, EncoderChoice};
use crate::nvenc;
use crate::vaapi::{self, VaCodec};

/// Probe-validated hardware encode of this machine. Sizes are the
/// conservative documented ceiling: offers above it clamp instead of
/// failing, and a weaker driver still fails its owner at encoder open.
pub fn video_codec_caps() -> VideoCodecCaps {
    let choices = gpu::encoders(&gpu::inventory());
    // Constrained Baseline and High per choice; NVENC offers both or none.
    let offered: Vec<(&EncoderChoice, bool, bool)> = choices
        .iter()
        .map(|choice| match choice {
            EncoderChoice::Nvenc { pci_address } => {
                let offers = nvenc::offers(pci_address.as_deref(), VideoCodec::H264);
                (choice, offers, offers)
            }
            EncoderChoice::Vaapi { render_node } => {
                let baseline = vaapi::offers_on(render_node, VaCodec::H264 { high: false });
                let high = baseline && vaapi::offers_on(render_node, VaCodec::H264 { high: true });
                (choice, baseline, high)
            }
        })
        .collect();
    let first = offered
        .iter()
        .find(|(_, baseline, _)| *baseline)
        .map(|(choice, _, _)| *choice);
    let encode = first.is_some();
    let high = offered.iter().any(|(_, _, high)| *high);
    let decode = vaapi::offers_h264_decode();
    let additional: Vec<CodecSupport> = [VideoCodec::H265, VideoCodec::Av1]
        .into_iter()
        .filter(|codec| round_trips(*codec))
        .map(|codec| CodecSupport {
            codec,
            encode: true,
            // Decoding these stays with the software decoders.
            decode: false,
            hardware: true,
        })
        .collect();
    VideoCodecCaps {
        encode_baseline: encode,
        encode_high: high,
        decode_baseline: decode,
        decode_high: decode,
        max_width: 4096,
        max_height: 4096,
        hardware: encode || decode || !additional.is_empty(),
        device: match first {
            Some(EncoderChoice::Nvenc { .. }) => "nvenc",
            _ => "vaapi",
        }
        .into(),
        chroma444: false,
        additional,
    }
}

/// Whether the encoder the normal choice opens for `codec` passes
/// [`decodes`].
pub(crate) fn round_trips(codec: VideoCodec) -> bool {
    let Ok(mut encoder) = LinuxEncoder::open(
        codec,
        PROBE_SIDE,
        PROBE_SIDE,
        30,
        1_000_000,
        false,
        vnilink::YuvMatrix::Bt709,
    ) else {
        return false;
    };
    decodes(codec, |picture, key| {
        encoder.encode(
            picture,
            &EncodeParams {
                force_keyframe: key,
                recovery: false,
                bitrate_bps: 1_000_000,
                frame_rate: 30,
            },
        )
    })
}

/// Width and height of the probe pictures: above NVENC's smallest sizes.
pub(crate) const PROBE_SIDE: u16 = 256;

/// Whether a red then a green picture of [`PROBE_SIDE`], put through
/// `encode` (the first as a keyframe), decode back with the software
/// decoder and the second shows green.
pub(crate) fn decodes(
    codec: VideoCodec,
    mut encode: impl FnMut(&RawFrame, bool) -> Result<EncodedSample, Error>,
) -> bool {
    const SIDE: u16 = PROBE_SIDE;
    let result = (|| -> Result<bool, Error> {
        let mut decoder = SoftwareDecoder::open(codec)?;
        let mut shown = None;
        for (index, bgra) in [[0, 0, 255, 255], [0, 255, 0, 255]].into_iter().enumerate() {
            let timestamp_us = index as u64 * 33_333;
            let picture = RawFrame::bgra(
                SIDE,
                SIDE,
                timestamp_us,
                bgra.repeat(usize::from(SIDE) * usize::from(SIDE)),
            )?;
            let sample = encode(&picture, index == 0)?;
            let frame = vnilink::EncodedFrame::new(
                codec,
                sample.keyframe(),
                timestamp_us,
                sample.bytes().to_vec(),
            )
            .map_err(|_| Error::EncodeRejected("probe packet"))?;
            shown = Some(decoder.decode(&frame)?);
        }
        let Some(shown) = shown else {
            return Ok(false);
        };
        let middle = usize::from(SIDE) / 2 * shown.stride() + usize::from(SIDE) / 2 * 4;
        let pixel = shown.pixels().get(middle..middle + 4).unwrap_or(&[0; 4]);
        Ok((shown.width(), shown.height()) == (SIDE, SIDE)
            && pixel[1] > 128
            && pixel[0] < 96
            && pixel[2] < 96)
    })();
    result.unwrap_or(false)
}
