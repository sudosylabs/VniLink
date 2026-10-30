// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

use vnilink::{VideoCodec, YuvMatrix};
use vnilink_media::{
    bgra_to_nv12, nv12_to_bgra, parse_annex_b, RawFrame, SoftwareDecoder, VideoDecoder, VideoSignal,
};

use crate::codecs::{decodes, PROBE_SIDE};
use crate::nvenc::{anchor_state, offers, Encoder};

/// Each codec an NVIDIA GPU offers decodes in software, in colour; on a
/// machine without NVENC there is nothing to check.
#[test]
fn nvenc_output_decodes_where_the_gpu_offers_the_codec() {
    for codec in [VideoCodec::H265, VideoCodec::Av1] {
        if !offers(None, codec) {
            continue;
        }
        let mut encoder = Encoder::open(
            None,
            codec,
            PROBE_SIDE,
            PROBE_SIDE,
            30,
            1_000_000,
            false,
            vnilink::YuvMatrix::Bt709,
        )
        .unwrap();
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
    if !offers(None, VideoCodec::H264) {
        return;
    }
    for high in [false, true] {
        let mut encoder = Encoder::open(
            None,
            VideoCodec::H264,
            PROBE_SIDE,
            PROBE_SIDE,
            30,
            1_000_000,
            high,
            vnilink::YuvMatrix::Bt709,
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
    if !offers(None, VideoCodec::H264) {
        return;
    }
    let open = || {
        Encoder::open(
            None,
            VideoCodec::H264,
            PROBE_SIDE,
            PROBE_SIDE,
            30,
            1_000_000,
            false,
            vnilink::YuvMatrix::Bt709,
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
/// conversion does ([`bgra_to_nv12`]), in either matrix: flat pictures,
/// black, white and the primaries included, decode to the same pixels. The
/// conversion comes before the codec; H.264 has no software decoder here.
#[test]
fn nvenc_conversion_matches_the_cpu_conversion() {
    for (codec, matrix) in [VideoCodec::H265, VideoCodec::Av1]
        .into_iter()
        .flat_map(|codec| [YuvMatrix::Bt709, YuvMatrix::Bt601].map(move |matrix| (codec, matrix)))
    {
        if !offers(None, codec) {
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
            let mut encoder = Encoder::open(
                None, codec, PROBE_SIDE, PROBE_SIDE, 30, 8_000_000, false, matrix,
            )
            .unwrap();
            let sample = encoder.encode(&picture, true).unwrap();
            let frame =
                vnilink::EncodedFrame::new(codec, true, 0, sample.bytes().to_vec()).unwrap();
            let decoded = SoftwareDecoder::open(codec)
                .unwrap()
                .decode(&frame)
                .unwrap();
            let middle = side / 2 * decoded.stride() + side / 2 * 4;
            let gpu = &decoded.pixels()[middle..middle + 3];
            let (y, uv) = bgra_to_nv12(&picture, matrix).unwrap().into_planes();
            let cpu = nv12_to_bgra(
                PROBE_SIDE,
                PROBE_SIDE,
                0,
                &y,
                &uv,
                VideoSignal::stream(matrix),
            )
            .unwrap();
            let cpu = &cpu.pixels()[middle..middle + 3];
            assert!(
                gpu.iter()
                    .zip(cpu)
                    .all(|(gpu, cpu)| gpu.abs_diff(*cpu) <= 2),
                "{codec:?} {matrix:?} {bgra:?} decoded as {gpu:?} from NVENC, {cpu:?} through the CPU"
            );
        }
    }
}

/// NVENC finds each NVIDIA GPU by the PCI address sysfs gives it, the way
/// sessions name it ([`crate::gpu::encoders`]).
#[test]
fn nvenc_opens_on_the_gpu_sysfs_names() {
    if !offers(None, VideoCodec::H264) {
        return;
    }
    let nvidia: Vec<_> = crate::gpu::inventory()
        .into_iter()
        .filter(|gpu| gpu.vendor == Some(crate::gpu::NVIDIA))
        .filter_map(|gpu| gpu.pci_address)
        .collect();
    assert!(
        !nvidia.is_empty(),
        "NVENC opens but sysfs lists no NVIDIA GPU"
    );
    for address in nvidia {
        assert!(
            offers(Some(&address), VideoCodec::H264),
            "NVENC does not open on {address}"
        );
    }
    assert!(!offers(Some("0000:ff:1f.7"), VideoCodec::H264));
}

/// A pattern that moves everywhere from one picture to the next, so each
/// costs what the bitrate allows.
fn moving(index: usize) -> RawFrame {
    let side = usize::from(PROBE_SIDE);
    let pixels = (0..side * side)
        .flat_map(|pixel| {
            let (x, y) = (pixel % side, pixel / side);
            let value = (x + 2 * y + index * 3) as u8;
            [value, value.wrapping_mul(3), (x ^ y) as u8, 255]
        })
        .collect();
    RawFrame::bgra(PROBE_SIDE, PROBE_SIDE, index as u64, pixels).unwrap()
}

/// Only the first picture is a keyframe however long the stream runs, and
/// a new bitrate taken by the open session makes no keyframe and changes
/// what each picture costs.
#[test]
fn nvenc_keyframes_come_only_when_asked_and_a_retune_makes_none() {
    if !offers(None, VideoCodec::H264) {
        return;
    }
    let mut encoder = Encoder::open(
        None,
        VideoCodec::H264,
        PROBE_SIDE,
        PROBE_SIDE,
        30,
        4_000_000,
        true,
        YuvMatrix::Bt709,
    )
    .unwrap();
    let mut sizes = Vec::new();
    for index in 0..300 {
        if index == 150 {
            assert!(
                encoder.retune(200_000, 30).unwrap(),
                "NVENC refused a retune"
            );
        }
        let sample = encoder
            .encode(&moving(index), false)
            .unwrap_or_else(|error| panic!("picture {index}: {error}"));
        assert_eq!(sample.keyframe(), index == 0, "picture {index}");
        sizes.push(sample.bytes().len());
    }
    let mean =
        |range: std::ops::Range<usize>| sizes[range.clone()].iter().sum::<usize>() / range.len();
    assert!(
        mean(260..300) * 4 < mean(100..140),
        "pictures after the retune average {} bytes against {} before",
        mean(260..300),
        mean(100..140)
    );
    assert!(encoder.encode(&moving(300), true).unwrap().keyframe());
}
