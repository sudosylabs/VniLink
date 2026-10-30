// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! H.264 SPS and PPS for the VA-API encoder.
//!
//! The driver writes the slice header from the same numbers. A keyframe that
//! arrives without parameter sets is prefixed with this Annex-B header.
//! Baseline (profile 66) is the interoperable default; High (profile 100)
//! enables CABAC entropy coding for ~10-15% better detail at the same
//! bitrate. This encoder configuration leaves the 8x8 transform off,
//! although the decoder supports it in streams from other encoders.

use vnilink_media::Error;

pub(crate) const LOG2_MAX_FRAME_NUM_MINUS4: u32 = 4;
pub(crate) const LOG2_MAX_POC_LSB_MINUS4: u32 = 4;
pub(crate) const MAX_FRAME_NUM: u32 = 1 << (LOG2_MAX_FRAME_NUM_MINUS4 + 4);
pub(crate) const MAX_POC_LSB: i32 = 1 << (LOG2_MAX_POC_LSB_MINUS4 + 4);

/// Encoder profile selected by session negotiation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EncodeProfile {
    Baseline,
    High,
}

impl EncodeProfile {
    pub(crate) fn idc(self) -> u8 {
        match self {
            EncodeProfile::Baseline => 66,
            EncodeProfile::High => 100,
        }
    }
}

/// Annex-B SPS and PPS for a visible picture. Dimensions are even.
/// The coded size rounds each edge up to a multiple of 16, and the SPS crops
/// the padding back off.
pub(crate) fn parameter_sets(
    profile: EncodeProfile,
    width: u16,
    height: u16,
    frame_rate: u8,
) -> Result<Vec<u8>, Error> {
    if width == 0 || height == 0 || width % 2 != 0 || height % 2 != 0 {
        return Err(Error::InvalidFrame("H.264 width and height must be even"));
    }
    if frame_rate == 0 {
        return Err(Error::InvalidSetting("frame rate must be non-zero"));
    }
    let coded_width = u32::from(round_up_16(width));
    let coded_height = u32::from(round_up_16(height));
    let mut sps = Bits::default();
    sps.u8(profile.idc());
    match profile {
        EncodeProfile::Baseline => sps.u8(0xC0),
        EncodeProfile::High => sps.u8(0x00),
    }
    sps.u8(level_idc(coded_width / 16 * (coded_height / 16)));
    sps.ue(0);
    if profile == EncodeProfile::High {
        // chroma_format_idc = 1 (4:2:0), 8-bit luma/chroma, no scaling matrix.
        sps.ue(1);
        sps.ue(0);
        sps.ue(0);
        sps.bit(0);
        sps.bit(0);
    }
    sps.ue(LOG2_MAX_FRAME_NUM_MINUS4);
    sps.ue(0);
    sps.ue(LOG2_MAX_POC_LSB_MINUS4);
    sps.ue(1);
    sps.bit(0);
    sps.ue(coded_width / 16 - 1);
    sps.ue(coded_height / 16 - 1);
    sps.bit(1);
    sps.bit(1);
    let crop_right = (coded_width - u32::from(width)) / 2;
    let crop_bottom = (coded_height - u32::from(height)) / 2;
    if crop_right == 0 && crop_bottom == 0 {
        sps.bit(0);
    } else {
        sps.bit(1);
        sps.ue(0);
        sps.ue(crop_right);
        sps.ue(0);
        sps.ue(crop_bottom);
    }
    sps.bit(1);
    sps.bit(0);
    sps.bit(0);
    sps.bit(0);
    sps.bit(0);
    sps.bit(1);
    sps.u32(1);
    sps.u32(u32::from(frame_rate) * 2);
    sps.bit(1);
    sps.bit(0);
    sps.bit(0);
    sps.bit(0);
    sps.bit(1);
    sps.bit(1);
    sps.ue(0);
    sps.ue(0);
    sps.ue(16);
    sps.ue(16);
    sps.ue(0);
    sps.ue(1);
    sps.trailing();

    let mut pps = Bits::default();
    pps.ue(0);
    pps.ue(0);
    // CABAC for High, CAVLC for Baseline. The driver is told the same mode
    // through the VA-API picture parameters, so slices match this header.
    pps.bit(u32::from(profile == EncodeProfile::High));
    pps.bit(0);
    pps.ue(0);
    pps.ue(0);
    pps.ue(0);
    pps.bit(0);
    pps.bits(0, 2);
    pps.se(0);
    pps.se(0);
    pps.se(0);
    pps.bit(1); // deblocking_filter_control_present_flag
    pps.bit(0); // constrained_intra_pred_flag
    pps.bit(0); // redundant_pic_cnt_present_flag
    if profile == EncodeProfile::High {
        // CABAC entropy coding and neutral second chroma offset. The 8x8
        // transform flag stays 0 for this encoder configuration.
        pps.bit(0);
        pps.bit(0);
        pps.se(0);
    }
    pps.trailing();

    let mut out = Vec::new();
    push_nal(&mut out, &[0x67], &sps.finish());
    push_nal(&mut out, &[0x68], &pps.finish());
    Ok(out)
}

pub(crate) fn round_up_16(value: u16) -> u16 {
    value.saturating_add(15) & !15
}

pub(crate) fn level_idc(macroblocks: u32) -> u8 {
    if macroblocks <= 99 {
        10
    } else if macroblocks <= 396 {
        21
    } else if macroblocks <= 1_620 {
        30
    } else if macroblocks <= 3_600 {
        31
    } else if macroblocks <= 5_120 {
        32
    } else if macroblocks <= 8_192 {
        40
    } else if macroblocks <= 8_704 {
        42
    } else if macroblocks <= 22_080 {
        50
    } else {
        51
    }
}

/// Appends one Annex-B NAL unit: start code, `header`, then `rbsp` with
/// emulation prevention bytes.
pub(crate) fn push_nal(out: &mut Vec<u8>, header: &[u8], rbsp: &[u8]) {
    out.extend_from_slice(&[0, 0, 0, 1]);
    out.extend_from_slice(header);
    let mut zeros = 0u8;
    for &byte in rbsp {
        if zeros == 2 && byte <= 3 {
            out.push(3);
            zeros = 0;
        }
        out.push(byte);
        zeros = if byte == 0 { zeros + 1 } else { 0 };
    }
}

/// Most significant bit first, as H.264 and H.265 syntax is written.
#[derive(Default)]
pub(crate) struct Bits {
    bytes: Vec<u8>,
    acc: u64,
    bits: u32,
}

impl Bits {
    pub(crate) fn bit(&mut self, value: u32) {
        self.bits(value, 1);
    }

    pub(crate) fn u8(&mut self, value: u8) {
        self.bits(u32::from(value), 8);
    }

    pub(crate) fn u32(&mut self, value: u32) {
        self.bits(value, 32);
    }

    pub(crate) fn bits(&mut self, value: u32, width: u32) {
        self.acc = (self.acc << width) | u64::from(value);
        self.bits += width;
        while self.bits >= 8 {
            let shift = self.bits - 8;
            self.bytes.push((self.acc >> shift) as u8);
            if shift == 0 {
                self.acc = 0;
            } else {
                self.acc &= (1u64 << shift) - 1;
            }
            self.bits -= 8;
        }
    }

    pub(crate) fn ue(&mut self, value: u32) {
        let code = value + 1;
        let width = 32 - code.leading_zeros();
        self.bits(0, width - 1);
        self.bits(code, width);
    }

    pub(crate) fn se(&mut self, value: i32) {
        let mapped = if value > 0 {
            ((value as u32) << 1) - 1
        } else {
            (-value as u32) << 1
        };
        self.ue(mapped);
    }

    pub(crate) fn trailing(&mut self) {
        self.bit(1);
        if self.bits > 0 {
            self.bits(0, 8 - self.bits);
        }
    }

    pub(crate) fn finish(self) -> Vec<u8> {
        self.bytes
    }
}
