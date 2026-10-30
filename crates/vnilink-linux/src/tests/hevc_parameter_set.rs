// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

use vnilink::YuvMatrix;
use vnilink_media::hevc_units;

use crate::hevc_parameter_set::{parameter_sets, slice_header, Layout};
use crate::syntax::{rbsp, Bits};

/// Intel's low-power encoder on a Raptor Lake GPU (iHD 26.1): 64x64 trees,
/// asymmetric partitions and per-unit quantizers required, the rest allowed.
const IHD_FEATURES: u32 = 0x0190_0464;
const IHD_BLOCK_SIZES: u32 = 0x88cf;

fn layout() -> Layout {
    Layout::from_attributes(Some(IHD_FEATURES), Some(IHD_BLOCK_SIZES))
}

#[test]
fn the_layout_follows_the_drivers_block_sizes_and_turns_on_what_it_allows() {
    let layout = layout();
    assert_eq!(
        (
            layout.log2_ctb,
            layout.log2_min_cb,
            layout.log2_max_tb,
            layout.log2_min_tb
        ),
        (6, 3, 5, 2)
    );
    assert_eq!((layout.depth_inter, layout.depth_intra), (2, 2));
    assert!(layout.amp && layout.sao && layout.temporal_mvp && layout.transform_skip);
    assert!(
        !layout.strong_intra_smoothing,
        "the driver does not allow it"
    );
    assert!(layout.cu_qp_delta);
    assert_eq!(layout.cu_qp_delta_depth(), 3);
    assert_eq!(layout.ctbs((1920, 1088)), 30 * 17);

    let unknown = Layout::from_attributes(None, None);
    assert_eq!((unknown.log2_ctb, unknown.log2_min_cb), (5, 3));
}

#[test]
fn the_sps_names_the_coded_size_and_crops_it_to_the_visible_one() {
    let sets = parameter_sets(&layout(), 1920, 1080, vnilink::YuvMatrix::Bt709).unwrap();
    let units = hevc_units(&sets).unwrap();
    let kinds: Vec<u8> = units.iter().map(|nal| (nal[0] >> 1) & 63).collect();
    assert_eq!(kinds, [32, 33, 34]);
    let sps = rbsp(&units[1][2..]);
    let mut bits = Bits::new(&sps);
    // Parameter set ids, sub-layers and the 96-bit profile, tier and level.
    bits.bits(8).unwrap();
    for _ in 0..3 {
        bits.bits(32).unwrap();
    }
    assert_eq!(bits.ue(), Some(0));
    assert_eq!(bits.ue(), Some(1), "4:2:0");
    assert_eq!((bits.ue(), bits.ue()), (Some(1920), Some(1088)));
    assert_eq!(bits.bit(), Some(1), "conformance window");
    let window: Vec<_> = (0..4).map(|_| bits.ue().unwrap()).collect();
    assert_eq!(window, [0, 0, 0, 4], "8 luma rows off the bottom");
    assert!(parameter_sets(&layout(), 1921, 1080, vnilink::YuvMatrix::Bt709).is_err());
}

/// `slice_type` and the bits after it, from a slice header NAL.
fn slice_start(header: &[u8]) -> (u8, u8, u32) {
    let kind = (header[4] >> 1) & 63;
    let payload = rbsp(&header[6..]);
    let mut bits = Bits::new(&payload);
    let first = bits.bit().unwrap();
    if kind == 19 {
        bits.bit().unwrap();
    }
    bits.ue().unwrap();
    (kind, first, bits.ue().unwrap())
}

#[test]
fn slice_headers_mark_keyframes_and_predicted_pictures_by_the_drivers_rules() {
    let mut layout = layout();
    let idr = slice_header(&layout, true, 0);
    assert_eq!(&idr[..4], &[0, 0, 0, 1]);
    assert_eq!(slice_start(&idr), (19, 1, 2));
    assert_eq!(slice_start(&slice_header(&layout, false, 1)), (1, 1, 1));
    layout.generalized_b = true;
    assert_eq!(slice_start(&slice_header(&layout, false, 1)), (1, 1, 0));
    // Byte aligned: the last byte ends with the stop bit or later zeros.
    let last = *slice_header(&layout, false, 4095).last().unwrap();
    assert_ne!(last, 0);
}

/// The SPS says the matrix it was asked for: its last fields are the VUI
/// with only a video signal type, the extension flag and the stop bit.
#[test]
fn the_sps_declares_the_matrix_in_its_vui() {
    for (matrix, code) in [(YuvMatrix::Bt709, 1u32), (YuvMatrix::Bt601, 6)] {
        let sets = parameter_sets(&layout(), 1920, 1080, matrix).unwrap();
        let units = hevc_units(&sets).unwrap();
        let mut bits: Vec<bool> = rbsp(&units[1][2..])
            .iter()
            .flat_map(|byte| (0..8).rev().map(move |at| byte >> at & 1 == 1))
            .collect();
        // The stop bit is the last one set; the extension flag is before it.
        while bits.pop() == Some(false) {}
        assert_eq!(bits.pop(), Some(false), "no SPS extension");
        let value = |bits: &[bool]| {
            bits.iter()
                .fold(0u32, |all, bit| all << 1 | u32::from(*bit))
        };
        // chroma location, neutral chroma, field sequence, frame field info,
        // default display window, timing and bitstream restriction: absent.
        let tail = bits.split_off(bits.len() - 7);
        assert_eq!(value(&tail), 0);
        let (primaries, transfer, matrix_coefficients) = {
            let end = bits.len();
            (
                value(&bits[end - 24..end - 16]),
                value(&bits[end - 16..end - 8]),
                value(&bits[end - 8..]),
            )
        };
        assert_eq!((primaries, transfer, matrix_coefficients), (1, 1, code));
        // video_signal_type_present, format 5, limited range, description.
        let head = &bits[bits.len() - 24 - 6..bits.len() - 24];
        assert_eq!(value(head), 0b1_101_0_1);
    }
}
