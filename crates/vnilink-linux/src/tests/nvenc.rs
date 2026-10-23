use vnilink::VideoCodec;
use vnilink_media::{bgra_to_nv12, parse_annex_b};

use crate::codecs::{decodes, PROBE_SIDE};
use crate::nvenc::{offers, Encoder};

/// Each codec an NVIDIA GPU offers decodes in software, in colour; on a
/// machine without NVENC there is nothing to check.
#[test]
fn nvenc_output_decodes_where_the_gpu_offers_the_codec() {
    for codec in [VideoCodec::H265, VideoCodec::Av1] {
        if !offers(codec) {
            continue;
        }
        let mut encoder =
            Encoder::open(codec, PROBE_SIDE, PROBE_SIDE, 30, 1_000_000, false).unwrap();
        assert!(
            decodes(codec, |picture, key| {
                let (y, uv) = bgra_to_nv12(picture)?.into_planes();
                encoder.encode(&y, &uv, key)
            }),
            "{codec:?} from NVENC does not decode"
        );
    }
}

/// There is no software H.264 decoder here: a keyframe must carry its
/// parameter sets and the next picture must not be one.
#[test]
fn nvenc_h264_keyframes_carry_their_parameter_sets() {
    if !offers(VideoCodec::H264) {
        return;
    }
    for high in [false, true] {
        let mut encoder = Encoder::open(
            VideoCodec::H264,
            PROBE_SIDE,
            PROBE_SIDE,
            30,
            1_000_000,
            high,
        )
        .unwrap();
        let picture = vnilink_media::RawFrame::bgra(
            PROBE_SIDE,
            PROBE_SIDE,
            0,
            [0, 0, 255, 255].repeat(usize::from(PROBE_SIDE) * usize::from(PROBE_SIDE)),
        )
        .unwrap();
        let (y, uv) = bgra_to_nv12(&picture).unwrap().into_planes();
        let key = encoder.encode(&y, &uv, true).unwrap();
        let parsed = parse_annex_b(key.bytes()).unwrap();
        assert!(key.keyframe() && parsed.has_sps() && parsed.has_pps() && parsed.has_idr());
        let next = encoder.encode(&y, &uv, false).unwrap();
        assert!(!next.keyframe());
    }
}
