use vnilink::VideoCodec;
use vnilink_media::{
    bgra_to_nv12, nv12_to_bgra, parse_annex_b, RawFrame, SoftwareDecoder, VideoDecoder,
    STREAM_SIGNAL,
};

use crate::codecs::{decodes, PROBE_SIDE};
use crate::nvenc::{anchor_state, offers, Encoder};

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
            decodes(codec, |picture, key| encoder.encode(picture, key)),
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
        let key = encoder.encode(&picture, true).unwrap();
        let parsed = parse_annex_b(key.bytes()).unwrap();
        assert!(key.keyframe() && parsed.has_sps() && parsed.has_pps() && parsed.has_idr());
        let next = encoder.encode(&picture, false).unwrap();
        assert!(!next.keyframe());
    }
}

/// A host reopens its encoder by opening the new session first: the anchor
/// stays open across that overlap. Other tests may hold sessions at the
/// same time, so when it closes is not checked here.
#[test]
fn nvenc_anchor_outlives_overlapping_sessions() {
    if !offers(VideoCodec::H264) {
        return;
    }
    let open = || {
        Encoder::open(
            VideoCodec::H264,
            PROBE_SIDE,
            PROBE_SIDE,
            30,
            1_000_000,
            false,
        )
        .unwrap()
    };
    let first = open();
    let mut second = open();
    drop(first);
    let (sessions, anchor) = anchor_state();
    assert!(sessions >= 1 && anchor);
    let picture = vnilink_media::RawFrame::bgra(
        PROBE_SIDE,
        PROBE_SIDE,
        0,
        [0, 0, 255, 255].repeat(usize::from(PROBE_SIDE) * usize::from(PROBE_SIDE)),
    )
    .unwrap();
    assert!(second.encode(&picture, true).unwrap().keyframe());
}

/// NVENC converts BGRA to the colour the stream declares as the CPU
/// conversion does ([`bgra_to_nv12`]): flat pictures, black, white and the
/// primaries included, decode to the same pixels. The conversion comes
/// before the codec; H.264 has no software decoder here.
#[test]
fn nvenc_conversion_matches_the_cpu_conversion() {
    for codec in [VideoCodec::H265, VideoCodec::Av1] {
        if !offers(codec) {
            continue;
        }
        let side = usize::from(PROBE_SIDE);
        for bgra in [
            [200, 120, 40, 255],
            [0, 0, 0, 255],
            [255, 255, 255, 255],
            [128, 128, 128, 255],
            [0, 0, 255, 255],
            [0, 255, 0, 255],
            [255, 0, 0, 255],
        ] {
            let picture =
                RawFrame::bgra(PROBE_SIDE, PROBE_SIDE, 0, bgra.repeat(side * side)).unwrap();
            let mut encoder =
                Encoder::open(codec, PROBE_SIDE, PROBE_SIDE, 30, 8_000_000, false).unwrap();
            let sample = encoder.encode(&picture, true).unwrap();
            let frame =
                vnilink::EncodedFrame::new(codec, true, 0, sample.bytes().to_vec()).unwrap();
            let decoded = SoftwareDecoder::open(codec)
                .unwrap()
                .decode(&frame)
                .unwrap();
            let middle = side / 2 * decoded.stride() + side / 2 * 4;
            let gpu = &decoded.pixels()[middle..middle + 3];
            let (y, uv) = bgra_to_nv12(&picture).unwrap().into_planes();
            let cpu = nv12_to_bgra(PROBE_SIDE, PROBE_SIDE, 0, &y, &uv, STREAM_SIGNAL).unwrap();
            let cpu = &cpu.pixels()[middle..middle + 3];
            assert!(
                gpu.iter()
                    .zip(cpu)
                    .all(|(gpu, cpu)| gpu.abs_diff(*cpu) <= 2),
                "{codec:?} {bgra:?} decoded as {gpu:?} from NVENC, {cpu:?} through the CPU"
            );
        }
    }
}
