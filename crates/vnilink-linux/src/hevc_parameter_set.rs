// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! H.265 Main parameter sets for the VA-API encoder, and the coding tools
//! the driver allows.
//!
//! The coded size is the visible size rounded up to 16; the SPS conformance
//! window crops the padding back off. Each picture predicts from the one
//! before it, so one reference picture is decoded ahead. The SPS carries the
//! stream's colour in its VUI ([`vnilink_media::VideoSignal::stream`]), as
//! every H.264 SPS does once the media crate has rewritten it.

use vnilink::YuvMatrix;
use vnilink_media::{Error, VideoSignal};

use crate::parameter_set::{push_nal, round_up_16, Bits};

/// `log2_max_pic_order_cnt_lsb_minus4`: picture order wraps after 4096.
pub(crate) const LOG2_MAX_POC_LSB_MINUS4: u32 = 8;

/// `VA_FEATURE_NOT_SUPPORTED` in `va.h`; anything else lets the tool on.
const NOT_SUPPORTED: u32 = 0;

/// Block sizes and coding tools for one driver, from its
/// `VAConfigAttribEncHEVCBlockSizes` and `VAConfigAttribEncHEVCFeatures`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Layout {
    pub(crate) log2_ctb: u32,
    pub(crate) log2_min_cb: u32,
    pub(crate) log2_max_tb: u32,
    pub(crate) log2_min_tb: u32,
    pub(crate) depth_inter: u32,
    pub(crate) depth_intra: u32,
    pub(crate) amp: bool,
    pub(crate) sao: bool,
    pub(crate) temporal_mvp: bool,
    pub(crate) strong_intra_smoothing: bool,
    pub(crate) transform_skip: bool,
    /// Rate control adjusts quantizers per coding unit, down to the
    /// smallest one.
    pub(crate) cu_qp_delta: bool,
    pub(crate) loop_filter_across_slices: bool,
    /// The driver predicts only from both lists: a predicted picture is a
    /// B slice with the same reference in each (a generalized B picture).
    pub(crate) generalized_b: bool,
}

impl Layout {
    /// What a driver allows, or the common 32x32 tree when it does not say.
    /// Every tool the driver supports is on, as FFmpeg's VA-API encoder
    /// asks of it: Intel's low-power encoder fails pictures without them.
    pub(crate) fn from_attributes(features: Option<u32>, block_sizes: Option<u32>) -> Self {
        let field = |value: u32, index: u32| (value >> (index * 2)) & 3;
        let mut layout = Self {
            log2_ctb: 5,
            log2_min_cb: 3,
            log2_max_tb: 5,
            log2_min_tb: 2,
            depth_inter: 2,
            depth_intra: 2,
            amp: true,
            sao: true,
            temporal_mvp: true,
            strong_intra_smoothing: true,
            transform_skip: true,
            cu_qp_delta: true,
            loop_filter_across_slices: true,
            generalized_b: false,
        };
        if let Some(sizes) = block_sizes {
            layout.log2_ctb = field(sizes, 0) + 3;
            layout.log2_min_cb = field(sizes, 2) + 3;
            layout.log2_max_tb = (field(sizes, 3) + 2).min(layout.log2_ctb);
            layout.log2_min_tb = field(sizes, 4) + 2;
            // The smallest depth the driver takes that still reaches the
            // smallest transform from the largest coding block.
            layout.depth_inter = field(sizes, 6).max(field(sizes, 5).min(2));
            layout.depth_intra = field(sizes, 8).max(field(sizes, 7).min(2));
        }
        if let Some(tools) = features {
            let supported = |index: u32| field(tools, index) != NOT_SUPPORTED;
            layout.amp = supported(2);
            layout.sao = supported(3);
            layout.temporal_mvp = supported(5);
            layout.strong_intra_smoothing = supported(6);
            layout.transform_skip = supported(10);
            layout.cu_qp_delta = supported(11);
        }
        layout
    }

    /// `diff_cu_qp_delta_depth`: quantizers may change in the smallest
    /// coding unit.
    pub(crate) fn cu_qp_delta_depth(&self) -> u32 {
        if self.cu_qp_delta {
            self.log2_ctb - self.log2_min_cb
        } else {
            0
        }
    }

    /// Coding tree blocks in one picture of `coded` size.
    pub(crate) fn ctbs(&self, (width, height): (u32, u32)) -> u32 {
        let side = 1 << self.log2_ctb;
        width.div_ceil(side) * height.div_ceil(side)
    }
}

/// `general_level_idc` (30 times the level) for a picture of `samples` luma
/// samples at up to 60 pictures a second.
pub(crate) fn level_idc(samples: u32) -> u8 {
    match samples {
        0..=552_960 => 93,
        552_961..=2_228_224 => 123,
        2_228_225..=8_912_896 => 153,
        _ => 183,
    }
}

/// Coded size for a visible size: each edge rounded up to 16.
pub(crate) fn coded_size(width: u16, height: u16) -> (u32, u32) {
    (
        u32::from(round_up_16(width)),
        u32::from(round_up_16(height)),
    )
}

fn profile_tier_level(bits: &mut Bits, level: u8) {
    // Main profile, Main tier; compatible with Main and Main 10.
    bits.bits(0, 2);
    bits.bit(0);
    bits.bits(1, 5);
    bits.u32(0x6000_0000);
    // Progressive frames only.
    bits.bit(1);
    bits.bit(0);
    bits.bit(0);
    bits.bit(1);
    bits.bits(0, 32);
    bits.bits(0, 12);
    bits.u8(level);
}

/// Annex-B VPS, SPS and PPS for a visible picture. Dimensions are even.
pub(crate) fn parameter_sets(
    layout: &Layout,
    width: u16,
    height: u16,
    matrix: YuvMatrix,
) -> Result<Vec<u8>, Error> {
    if width == 0 || height == 0 || width % 2 != 0 || height % 2 != 0 {
        return Err(Error::InvalidFrame("H.265 width and height must be even"));
    }
    let (coded_width, coded_height) = coded_size(width, height);
    let level = level_idc(coded_width * coded_height);

    let mut vps = Bits::default();
    vps.bits(0, 4);
    vps.bit(1);
    vps.bit(1);
    vps.bits(0, 6);
    vps.bits(0, 3);
    vps.bit(1);
    vps.bits(0xFFFF, 16);
    profile_tier_level(&mut vps, level);
    vps.bit(0);
    // One reference picture, no reordering.
    vps.ue(1);
    vps.ue(0);
    vps.ue(0);
    vps.bits(0, 6);
    vps.ue(0);
    vps.bit(0);
    vps.bit(0);
    vps.trailing();

    let mut sps = Bits::default();
    sps.bits(0, 4);
    sps.bits(0, 3);
    sps.bit(1);
    profile_tier_level(&mut sps, level);
    sps.ue(0);
    sps.ue(1);
    sps.ue(coded_width);
    sps.ue(coded_height);
    let crop_right = coded_width - u32::from(width);
    let crop_bottom = coded_height - u32::from(height);
    if crop_right == 0 && crop_bottom == 0 {
        sps.bit(0);
    } else {
        // In chroma samples: two luma samples each way for 4:2:0.
        sps.bit(1);
        sps.ue(0);
        sps.ue(crop_right / 2);
        sps.ue(0);
        sps.ue(crop_bottom / 2);
    }
    sps.ue(0);
    sps.ue(0);
    sps.ue(LOG2_MAX_POC_LSB_MINUS4);
    sps.bit(0);
    sps.ue(1);
    sps.ue(0);
    sps.ue(0);
    sps.ue(layout.log2_min_cb - 3);
    sps.ue(layout.log2_ctb - layout.log2_min_cb);
    sps.ue(layout.log2_min_tb - 2);
    sps.ue(layout.log2_max_tb - layout.log2_min_tb);
    sps.ue(layout.depth_inter);
    sps.ue(layout.depth_intra);
    sps.bit(0);
    sps.bit(u32::from(layout.amp));
    sps.bit(u32::from(layout.sao));
    sps.bit(0);
    // Reference picture sets are written in each slice header.
    sps.ue(0);
    sps.bit(0);
    sps.bit(u32::from(layout.temporal_mvp));
    sps.bit(u32::from(layout.strong_intra_smoothing));
    // vui_parameters_present_flag: the video signal type and nothing else.
    let signal = VideoSignal::stream(matrix);
    sps.bit(1);
    sps.bit(0); // aspect_ratio_info_present_flag
    sps.bit(0); // overscan_info_present_flag
    sps.bit(1); // video_signal_type_present_flag
    sps.bits(5, 3); // video_format: unspecified
    sps.bit(u32::from(signal.full_range));
    sps.bit(1); // colour_description_present_flag
    sps.bits(u32::from(signal.primaries), 8);
    sps.bits(u32::from(signal.transfer), 8);
    sps.bits(u32::from(signal.matrix), 8);
    sps.bit(0); // chroma_loc_info_present_flag
    sps.bit(0); // neutral_chroma_indication_flag
    sps.bit(0); // field_seq_flag
    sps.bit(0); // frame_field_info_present_flag
    sps.bit(0); // default_display_window_flag
    sps.bit(0); // vui_timing_info_present_flag
    sps.bit(0); // bitstream_restriction_flag
    sps.bit(0); // sps_extension_present_flag
    sps.trailing();

    let mut pps = Bits::default();
    pps.ue(0);
    pps.ue(0);
    pps.bit(0);
    pps.bit(0);
    pps.bits(0, 3);
    pps.bit(0);
    pps.bit(0);
    pps.ue(0);
    pps.ue(0);
    pps.se(0);
    pps.bit(0);
    pps.bit(u32::from(layout.transform_skip));
    pps.bit(u32::from(layout.cu_qp_delta));
    if layout.cu_qp_delta {
        pps.ue(layout.cu_qp_delta_depth());
    }
    pps.se(0);
    pps.se(0);
    pps.bit(0);
    pps.bit(0);
    pps.bit(0);
    pps.bit(0);
    pps.bit(0);
    pps.bit(0);
    pps.bit(u32::from(layout.loop_filter_across_slices));
    pps.bit(0);
    pps.bit(0);
    pps.bit(0);
    pps.ue(0);
    pps.bit(0);
    pps.bit(0);
    pps.trailing();

    let mut out = Vec::new();
    push_nal(&mut out, &nal_header(32), &vps.finish());
    push_nal(&mut out, &nal_header(33), &sps.finish());
    push_nal(&mut out, &nal_header(34), &pps.finish());
    Ok(out)
}

/// The two-byte NAL header of `kind` on the base layer.
pub(crate) fn nal_header(kind: u8) -> [u8; 2] {
    [kind << 1, 1]
}

/// The Annex-B slice segment header of a picture made of one slice, up to
/// and including its byte alignment; the driver appends the slice data. A
/// keyframe is an IDR picture; any other picture predicts from the one
/// before it, `poc` being its picture order.
pub(crate) fn slice_header(layout: &Layout, key: bool, poc: u32) -> Vec<u8> {
    let mut slice = Bits::default();
    slice.bit(1);
    if key {
        slice.bit(0);
    }
    slice.ue(0);
    slice.ue(slice_type(layout, key).into());
    if !key {
        slice.bits(
            poc % (1 << (LOG2_MAX_POC_LSB_MINUS4 + 4)),
            LOG2_MAX_POC_LSB_MINUS4 + 4,
        );
        // An explicit set: one picture before this one, used for prediction.
        slice.bit(0);
        slice.ue(1);
        slice.ue(0);
        slice.ue(0);
        slice.bit(1);
        if layout.temporal_mvp {
            slice.bit(1);
        }
    }
    if layout.sao {
        slice.bit(1);
        slice.bit(1);
    }
    if !key {
        slice.bit(0);
        if layout.generalized_b {
            slice.bit(0);
            if layout.temporal_mvp {
                // Collocated from list 0.
                slice.bit(1);
            }
        }
        // Five merge candidates.
        slice.ue(0);
    }
    slice.se(0);
    if layout.loop_filter_across_slices {
        slice.bit(0);
    }
    slice.trailing();
    let mut out = Vec::new();
    push_nal(
        &mut out,
        &nal_header(if key { IDR_W_RADL } else { TRAIL_R }),
        &slice.finish(),
    );
    out
}

/// `slice_type`: 2 is I, 1 is P and 0 is B.
pub(crate) fn slice_type(layout: &Layout, key: bool) -> u8 {
    if key {
        2
    } else if layout.generalized_b {
        0
    } else {
        1
    }
}

/// NAL unit types: an IDR picture without leading pictures, and a
/// reference picture after it.
pub(crate) const IDR_W_RADL: u8 = 19;
pub(crate) const TRAIL_R: u8 = 1;
