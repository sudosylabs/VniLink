// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! H.264 sequence, picture, and slice header syntax for the VA-API decoder.
//!
//! The reader covers the Constrained Baseline, Main, and High progressive
//! subset the decoder accepts: one frame per access unit, P and IDR slices,
//! `pic_order_cnt_type` 0, sliding-window reference marking, and 4:2:0
//! eight-bit video. Anything outside that subset is refused with an explicit
//! `Unsupported` error at parse time, never misdecoded. Field pictures,
//! MBAFF, FMO, B slices, long-term references, and
//! monochrome or wide-gamut formats are not attempted.
//!
//! Scaling-list storage follows the VA-API layout: six 4x4 lists in raster
//! order (Intra Y, Intra Cr, Intra Cb, Inter Y, Inter Cr, Inter Cb). The
//! default tables are the specification defaults also shipped by FFmpeg.
//! Both 4x4 and 8x8 lists are parsed with sequence/picture fallback rules.

use vnilink_media::Error;

/// 4x4 zigzag scan: parse position to raster position.
const ZIGZAG_4X4: [usize; 16] = [0, 1, 4, 8, 5, 2, 3, 6, 9, 12, 13, 10, 7, 11, 14, 15];

/// Default 4x4 scaling lists in raster order.
const DEFAULT_INTRA_4X4: [u8; 16] = [
    6, 13, 20, 28, 13, 20, 28, 32, 20, 28, 32, 37, 28, 32, 37, 42,
];
const DEFAULT_INTER_4X4: [u8; 16] = [
    10, 14, 20, 24, 14, 20, 24, 27, 20, 24, 27, 30, 24, 27, 30, 34,
];

/// Specification default 8x8 scaling lists in raster order.
pub(crate) const DEFAULT_INTRA_8X8: [u8; 64] = [
    6, 10, 13, 16, 18, 23, 25, 27, 10, 11, 16, 18, 23, 25, 27, 29, 13, 16, 18, 23, 25, 27, 29, 31,
    16, 18, 23, 25, 27, 29, 31, 33, 18, 23, 25, 27, 29, 31, 33, 36, 23, 25, 27, 29, 31, 33, 36, 38,
    25, 27, 29, 31, 33, 36, 38, 40, 27, 29, 31, 33, 36, 38, 40, 42,
];
pub(crate) const DEFAULT_INTER_8X8: [u8; 64] = [
    9, 13, 15, 17, 19, 21, 22, 24, 13, 13, 17, 19, 21, 22, 24, 25, 15, 17, 19, 21, 22, 24, 25, 27,
    17, 19, 21, 22, 24, 25, 27, 28, 19, 21, 22, 24, 25, 27, 28, 30, 21, 22, 24, 25, 27, 28, 30, 32,
    22, 24, 25, 27, 28, 30, 32, 33, 24, 25, 27, 28, 30, 32, 33, 35,
];

/// Exp-Golomb reader over RBSP bytes.
pub(crate) struct Bits<'a> {
    bytes: &'a [u8],
    position: usize,
}

impl<'a> Bits<'a> {
    pub(crate) fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, position: 0 }
    }

    pub(crate) fn bit(&mut self) -> Option<u8> {
        self.bits(1).map(|value| value as u8)
    }

    pub(crate) fn bits(&mut self, width: u32) -> Option<u32> {
        if width == 0 || width > 32 {
            return None;
        }
        let mut value = 0u32;
        for _ in 0..width {
            let byte = *self.bytes.get(self.position / 8)?;
            let shift = 7 - (self.position % 8);
            value = (value << 1) | u32::from((byte >> shift) & 1);
            self.position += 1;
        }
        Some(value)
    }

    pub(crate) fn u8(&mut self) -> Option<u8> {
        self.bits(8).map(|value| value as u8)
    }

    pub(crate) fn ue(&mut self) -> Option<u32> {
        let mut zeros = 0u32;
        while self.bit()? == 0 {
            zeros += 1;
            if zeros > 31 {
                return None;
            }
        }
        if zeros == 0 {
            return Some(0);
        }
        let rest = self.bits(zeros)?;
        Some(((1u32 << zeros) - 1) + rest)
    }

    pub(crate) fn se(&mut self) -> Option<i32> {
        let code = self.ue()?;
        if code % 2 == 0 {
            Some(-((code / 2) as i32))
        } else {
            Some((code.div_ceil(2)) as i32)
        }
    }

    /// Bits still unread, including the RBSP stop bit and alignment.
    pub(crate) fn remaining(&self) -> usize {
        self.bytes.len() * 8 - self.position
    }

    /// True when only the RBSP stop bit and alignment zeros remain.
    pub(crate) fn at_trailing(&self) -> bool {
        let mut probe = self.position;
        let total = self.position + self.remaining();
        while probe < total {
            let byte = self.bytes[probe / 8];
            let bit = (byte >> (7 - (probe % 8))) & 1;
            if bit == 1 {
                // Everything after the first one bit must be zero.
                probe += 1;
                while probe < total {
                    let byte = self.bytes[probe / 8];
                    if (byte >> (7 - (probe % 8))) & 1 != 0 {
                        return false;
                    }
                    probe += 1;
                }
                return true;
            }
            probe += 1;
        }
        true
    }
}

/// Removes emulation-prevention bytes (`00 00 03` to `00 00`).
pub(crate) fn rbsp(nal: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(nal.len());
    let mut zeros = 0u8;
    for &byte in nal {
        if zeros == 2 && byte == 3 {
            zeros = 0;
            continue;
        }
        out.push(byte);
        zeros = if byte == 0 { zeros + 1 } else { 0 };
    }
    out
}

/// Parsed sequence parameter set: geometry plus everything the VA-API
/// picture parameters need.
#[derive(Clone, Debug)]
pub(crate) struct Sps {
    pub id: u32,
    pub profile: u8,
    /// Raw constraint-flags byte: bit 0x40 is `constraint_set1_flag`.
    pub constraints: u8,
    pub level: u8,
    pub log2_max_frame_num_minus4: u32,
    pub poc_type: u32,
    pub log2_max_poc_lsb_minus4: u32,
    pub num_ref_frames: u32,
    pub gaps_flag: bool,
    pub pic_width_mbs_minus1: u32,
    pub pic_height_map_minus1: u32,
    pub direct_8x8: bool,
    pub coded: (u16, u16),
    pub visible: (u16, u16),
    pub scaling4: [[u8; 16]; 6],
    pub scaling8: [[u8; 64]; 2],
    pub scaling_matrix_present: bool,
}

fn truncated() -> Error {
    Error::InvalidFrame("H.264 parameter set is truncated")
}

/// Parses one 4x4 scaling list. Returns true when a custom list was stored.
fn scaling_list_4x4(
    bits: &mut Bits<'_>,
    jvt: &[u8; 16],
    fallback: &[u8; 16],
    out: &mut [u8; 16],
) -> Result<bool, Error> {
    if bits.bit().ok_or_else(truncated)? == 0 {
        out.copy_from_slice(fallback);
        return Ok(false);
    }
    let mut last = 8i32;
    let mut next = 8i32;
    let mut stored = *fallback;
    for i in 0..16 {
        if next != 0 {
            let delta = bits.se().ok_or_else(truncated)?;
            if !(-128..=127).contains(&delta) {
                return Err(Error::InvalidFrame("H.264 scaling list is out of range"));
            }
            next = (last + delta) & 0xff;
        }
        if i == 0 && next == 0 {
            stored.copy_from_slice(jvt);
            break;
        }
        last = if next != 0 {
            stored[ZIGZAG_4X4[i]] = next as u8;
            next
        } else {
            last
        };
    }
    *out = stored;
    Ok(true)
}

/// Parses an 8x8 list into the raster layout expected by VA-API.
fn scaling_list_8x8(
    bits: &mut Bits<'_>,
    jvt: &[u8; 64],
    fallback: &[u8; 64],
    out: &mut [u8; 64],
) -> Result<(), Error> {
    if bits.bit().ok_or_else(truncated)? == 0 {
        *out = *fallback;
        return Ok(());
    }
    let scan = [
        0, 1, 8, 16, 9, 2, 3, 10, 17, 24, 32, 25, 18, 11, 4, 5, 12, 19, 26, 33, 40, 48, 41, 34, 27,
        20, 13, 6, 7, 14, 21, 28, 35, 42, 49, 56, 57, 50, 43, 36, 29, 22, 15, 23, 30, 37, 44, 51,
        58, 59, 52, 45, 38, 31, 39, 46, 53, 60, 61, 54, 47, 55, 62, 63,
    ];
    let mut last = 8i32;
    let mut next = 8i32;
    for (i, raster) in scan.into_iter().enumerate() {
        if next != 0 {
            let delta = bits.se().ok_or_else(truncated)?;
            if !(-128..=127).contains(&delta) {
                return Err(Error::InvalidFrame("H.264 scaling list is out of range"));
            }
            next = (last + delta) & 255;
        }
        if i == 0 && next == 0 {
            *out = *jvt;
            return Ok(());
        }
        last = if next != 0 { next } else { last };
        out[raster] = last as u8;
    }
    Ok(())
}

impl Sps {
    /// Parses a full sequence parameter set NAL unit, header byte included.
    pub(crate) fn parse(nal: &[u8]) -> Result<Self, Error> {
        let header = nal
            .first()
            .ok_or(Error::InvalidFrame("H.264 SPS is empty"))?;
        if header & 0x1F != 7 {
            return Err(Error::InvalidFrame("H.264 NAL unit is not an SPS"));
        }
        let payload = rbsp(&nal[1..]);
        let mut bits = Bits::new(&payload);
        let profile = bits.u8().ok_or_else(truncated)?;
        let constraints = bits.u8().ok_or_else(truncated)?;
        let level = bits.u8().ok_or_else(truncated)?;
        let id = bits.ue().ok_or_else(truncated)?;
        if id > 31 {
            return Err(Error::InvalidFrame("H.264 SPS id is out of range"));
        }
        // Absent sequence scaling matrices imply flat weights, not JVT
        // defaults (those apply to explicit default/fallback scaling lists).
        let mut scaling4 = [[16; 16]; 6];
        let mut scaling8 = [[16; 64]; 2];
        let mut scaling_matrix_present = false;
        if matches!(
            profile,
            100 | 110 | 122 | 244 | 44 | 83 | 86 | 118 | 128 | 138 | 139 | 134
        ) {
            let chroma = bits.ue().ok_or_else(truncated)?;
            if chroma != 1 {
                return Err(Error::Unsupported("only 4:2:0 H.264 is supported"));
            }
            let bit_depth_luma = bits.ue().ok_or_else(truncated)?;
            let bit_depth_chroma = bits.ue().ok_or_else(truncated)?;
            if bit_depth_luma != 0 || bit_depth_chroma != 0 {
                return Err(Error::Unsupported("only eight-bit H.264 is supported"));
            }
            let transform_bypass = bits.bit().ok_or_else(truncated)?;
            if transform_bypass != 0 {
                return Err(Error::Unsupported("lossless H.264 is not supported"));
            }
            scaling_matrix_present = bits.bit().ok_or_else(truncated)? == 1;
            if scaling_matrix_present {
                let mut previous = DEFAULT_INTRA_4X4;
                for (index, slot) in scaling4.iter_mut().enumerate() {
                    let jvt = if index < 3 {
                        &DEFAULT_INTRA_4X4
                    } else {
                        &DEFAULT_INTER_4X4
                    };
                    let fallback = if index == 0 || index == 3 {
                        *jvt
                    } else {
                        previous
                    };
                    let mut out = fallback;
                    scaling_list_4x4(&mut bits, jvt, &fallback, &mut out)?;
                    previous = out;
                    *slot = out;
                }
                for (index, slot) in scaling8.iter_mut().enumerate() {
                    let default = if index == 0 {
                        &DEFAULT_INTRA_8X8
                    } else {
                        &DEFAULT_INTER_8X8
                    };
                    scaling_list_8x8(&mut bits, default, default, slot)?;
                }
            }
        }
        let log2_max_frame_num_minus4 = bits.ue().ok_or_else(truncated)?;
        if log2_max_frame_num_minus4 > 12 {
            return Err(Error::InvalidFrame(
                "H.264 frame number range is out of range",
            ));
        }
        let poc_type = bits.ue().ok_or_else(truncated)?;
        if poc_type != 0 {
            return Err(Error::Unsupported(
                "only pic_order_cnt_type 0 H.264 is supported",
            ));
        }
        let log2_max_poc_lsb_minus4 = bits.ue().ok_or_else(truncated)?;
        if log2_max_poc_lsb_minus4 > 12 {
            return Err(Error::InvalidFrame(
                "H.264 picture order range is out of range",
            ));
        }
        let num_ref_frames = bits.ue().ok_or_else(truncated)?;
        if num_ref_frames > 16 {
            return Err(Error::InvalidFrame("H.264 reference count is out of range"));
        }
        let gaps_flag = bits.bit().ok_or_else(truncated)? != 0;
        let pic_width_mbs_minus1 = bits.ue().ok_or_else(truncated)?;
        let pic_height_map_minus1 = bits.ue().ok_or_else(truncated)?;
        let frame_mbs_only = bits.bit().ok_or_else(truncated)?;
        if frame_mbs_only == 0 {
            return Err(Error::Unsupported("interlaced H.264 is not supported"));
        }
        // mb_adaptive_frame_field_flag exists only for non-frame-only
        // pictures, which are refused above.
        let direct_8x8 = bits.bit().ok_or_else(truncated)?;
        let crop = if bits.bit().ok_or_else(truncated)? == 1 {
            [
                bits.ue().ok_or_else(truncated)?,
                bits.ue().ok_or_else(truncated)?,
                bits.ue().ok_or_else(truncated)?,
                bits.ue().ok_or_else(truncated)?,
            ]
        } else {
            [0, 0, 0, 0]
        };
        let coded_width = (pic_width_mbs_minus1 + 1)
            .checked_mul(16)
            .ok_or(Error::InvalidFrame(
                "H.264 SPS picture size is out of range",
            ))?;
        let coded_height =
            (pic_height_map_minus1 + 1)
                .checked_mul(16)
                .ok_or(Error::InvalidFrame(
                    "H.264 SPS picture size is out of range",
                ))?;
        // 4:2:0 frame pictures crop in two-pixel units.
        let visible_width = coded_width.saturating_sub((crop[0] + crop[1]) * 2);
        let visible_height = coded_height.saturating_sub((crop[2] + crop[3]) * 2);
        let coded = (
            u16::try_from(coded_width)
                .map_err(|_| Error::InvalidFrame("H.264 SPS picture size is out of range"))?,
            u16::try_from(coded_height)
                .map_err(|_| Error::InvalidFrame("H.264 SPS picture size is out of range"))?,
        );
        let visible = (
            u16::try_from(visible_width)
                .map_err(|_| Error::InvalidFrame("H.264 SPS picture size is out of range"))?,
            u16::try_from(visible_height)
                .map_err(|_| Error::InvalidFrame("H.264 SPS picture size is out of range"))?,
        );
        if visible.0 == 0 || visible.1 == 0 {
            return Err(Error::InvalidFrame("H.264 SPS picture size is empty"));
        }
        Ok(Self {
            id,
            profile,
            constraints,
            level,
            log2_max_frame_num_minus4,
            poc_type,
            log2_max_poc_lsb_minus4,
            num_ref_frames,
            gaps_flag,
            pic_width_mbs_minus1,
            pic_height_map_minus1,
            direct_8x8: direct_8x8 != 0,
            coded,
            visible,
            scaling4,
            scaling8,
            scaling_matrix_present,
        })
    }
}

/// Parsed picture parameter set: everything the VA-API picture parameters
/// and the slice header reader need.
#[derive(Clone, Debug)]
pub(crate) struct Pps {
    pub id: u32,
    pub sps_id: u32,
    pub entropy_cabac: bool,
    pub bottom_field_order_present: bool,
    pub num_ref_idx_l0_default: u32,
    pub num_ref_idx_l1_default: u32,
    pub weighted_pred: bool,
    pub weighted_bipred: u8,
    pub pic_init_qp: i32,
    pub pic_init_qs: i32,
    pub chroma_qp_offset: [i32; 2],
    pub deblocking_present: bool,
    pub constrained_intra: bool,
    pub redundant_present: bool,
    pub scaling4: [[u8; 16]; 6],
    pub scaling8: [[u8; 64]; 2],
    pub transform_8x8: bool,
}

impl Pps {
    /// Parses a full picture parameter set NAL unit, header byte included.
    /// `sps` supplies the fallback scaling lists and the profile rule for
    /// the trailing section.
    pub(crate) fn parse(nal: &[u8], sps: &Sps) -> Result<Self, Error> {
        let header = nal
            .first()
            .ok_or(Error::InvalidFrame("H.264 PPS is empty"))?;
        if header & 0x1F != 8 {
            return Err(Error::InvalidFrame("H.264 NAL unit is not a PPS"));
        }
        let payload = rbsp(&nal[1..]);
        let mut bits = Bits::new(&payload);
        let id = bits.ue().ok_or_else(truncated)?;
        if id > 255 {
            return Err(Error::InvalidFrame("H.264 PPS id is out of range"));
        }
        let sps_id = bits.ue().ok_or_else(truncated)?;
        if sps_id != sps.id {
            return Err(Error::InvalidFrame("H.264 PPS names an unknown SPS"));
        }
        let entropy_cabac = bits.bit().ok_or_else(truncated)? != 0;
        let bottom_field_order_present = bits.bit().ok_or_else(truncated)? != 0;
        let num_slice_groups = bits.ue().ok_or_else(truncated)?;
        if num_slice_groups != 0 {
            return Err(Error::Unsupported("H.264 slice groups are not supported"));
        }
        let num_ref_idx_l0_default = bits.ue().ok_or_else(truncated)?;
        let num_ref_idx_l1_default = bits.ue().ok_or_else(truncated)?;
        if num_ref_idx_l0_default > 31 || num_ref_idx_l1_default > 31 {
            return Err(Error::InvalidFrame("H.264 reference count is out of range"));
        }
        let weighted_pred = bits.bit().ok_or_else(truncated)? != 0;
        let weighted_bipred = bits.bits(2).ok_or_else(truncated)? as u8;
        let pic_init_qp = bits.se().ok_or_else(truncated)? + 26;
        let pic_init_qs = bits.se().ok_or_else(truncated)? + 26;
        let chroma_qp_offset_0 = bits.se().ok_or_else(truncated)?;
        if !(-12..=12).contains(&chroma_qp_offset_0) {
            return Err(Error::InvalidFrame(
                "H.264 chroma QP offset is out of range",
            ));
        }
        let deblocking_present = bits.bit().ok_or_else(truncated)? != 0;
        let constrained_intra = bits.bit().ok_or_else(truncated)? != 0;
        let redundant_present = bits.bit().ok_or_else(truncated)? != 0;
        // Constrained Baseline, Main, and Extended streams carry nothing
        // after this point. Otherwise trailing bits mean the 8x8 and second
        // chroma-offset section follows.
        let more_rbsp = if matches!(sps.profile, 66 | 77 | 88) && sps.constraints & 0xE0 != 0 {
            false
        } else {
            !bits.at_trailing()
        };
        let mut chroma_qp_offset_1 = chroma_qp_offset_0;
        let mut scaling4 = sps.scaling4;
        let mut scaling8 = sps.scaling8;
        let mut transform_8x8 = false;
        if more_rbsp {
            transform_8x8 = bits.bit().ok_or_else(truncated)? != 0;
            if bits.bit().ok_or_else(truncated)? == 1 {
                let mut previous = sps.scaling4;
                for (index, slot) in scaling4.iter_mut().enumerate() {
                    let default = if index < 3 {
                        &DEFAULT_INTRA_4X4
                    } else {
                        &DEFAULT_INTER_4X4
                    };
                    let fallback = if index == 0 || index == 3 {
                        if sps.scaling_matrix_present {
                            sps.scaling4[index]
                        } else {
                            *default
                        }
                    } else {
                        previous[index - 1]
                    };
                    let mut out = fallback;
                    scaling_list_4x4(&mut bits, default, &fallback, &mut out)?;
                    previous[index] = out;
                    *slot = out;
                }
                if transform_8x8 {
                    for (index, slot) in scaling8.iter_mut().enumerate() {
                        let default = if index == 0 {
                            &DEFAULT_INTRA_8X8
                        } else {
                            &DEFAULT_INTER_8X8
                        };
                        let fallback = if sps.scaling_matrix_present {
                            sps.scaling8[index]
                        } else {
                            *default
                        };
                        scaling_list_8x8(&mut bits, default, &fallback, slot)?;
                    }
                }
            }
            chroma_qp_offset_1 = bits.se().ok_or_else(truncated)?;
            if !(-12..=12).contains(&chroma_qp_offset_1) {
                return Err(Error::InvalidFrame(
                    "H.264 chroma QP offset is out of range",
                ));
            }
        }
        Ok(Self {
            id,
            sps_id,
            entropy_cabac,
            bottom_field_order_present,
            num_ref_idx_l0_default,
            num_ref_idx_l1_default,
            weighted_pred,
            weighted_bipred,
            pic_init_qp,
            pic_init_qs,
            chroma_qp_offset: [chroma_qp_offset_0, chroma_qp_offset_1],
            deblocking_present,
            constrained_intra,
            redundant_present,
            scaling4,
            scaling8,
            transform_8x8,
        })
    }
}

/// Slice kinds the decoder accepts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SliceKind {
    P,
    I,
}

/// Decoded-picture-buffer marking for one non-IDR picture.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Marking {
    Sliding,
    FlushAll,
    MarkNonRef { diffs: Vec<u32> },
}

/// One prediction-weight table entry.
#[derive(Clone, Debug, Default)]
pub(crate) struct WeightEntry {
    pub luma_flag: bool,
    pub luma_weight: i16,
    pub luma_offset: i16,
    pub chroma_flag: bool,
    pub chroma_weight: [i16; 2],
    pub chroma_offset: [i16; 2],
}

/// Parsed prediction weight table.
#[derive(Clone, Debug)]
pub(crate) struct WeightTable {
    pub luma_denom: u8,
    pub chroma_denom: u8,
    pub entries: Vec<WeightEntry>,
}

/// Parsed slice header: the fields the VA-API slice parameters need.
#[derive(Clone, Debug)]
pub(crate) struct Slice {
    pub first_mb: u16,
    pub kind: SliceKind,
    pub pps_id: u32,
    pub frame_num: u32,
    pub idr_pic_id: u32,
    pub poc_lsb: u32,
    pub num_ref_l0: u32,
    pub weights: Option<WeightTable>,
    pub marking: Marking,
    pub no_output_prior: bool,
    pub cabac_init: u8,
    pub qp_delta: i32,
    pub deblocking_idc: u8,
    pub alpha_offset: i32,
    pub beta_offset: i32,
    pub header_bits: u16,
}

impl Slice {
    /// Parses a slice NAL unit, header byte included. `is_idr` selects the
    /// IDR header shape. `nal_ref_idc` is the NAL header's reference bit.
    pub(crate) fn parse(
        nal: &[u8],
        sps: &Sps,
        pps: &Pps,
        is_idr: bool,
        nal_ref_idc: u8,
    ) -> Result<Self, Error> {
        let payload = rbsp(&nal[1..]);
        let mut bits = Bits::new(&payload);
        let first_mb = bits.ue().ok_or_else(truncated)?;
        if first_mb > 0xFFFF {
            return Err(Error::InvalidFrame("H.264 slice start is out of range"));
        }
        let raw_type = bits.ue().ok_or_else(truncated)?;
        let kind = match raw_type % 5 {
            0 => SliceKind::P,
            2 => SliceKind::I,
            _ => {
                return Err(Error::Unsupported(
                    "only P and I H.264 slices are supported",
                ));
            }
        };
        let pps_id = bits.ue().ok_or_else(truncated)?;
        if pps_id != pps.id {
            return Err(Error::InvalidFrame("H.264 slice names an unknown PPS"));
        }
        // IDR requires frame_num = 0, but those bits are still on the wire.
        let frame_num = bits
            .bits(sps.log2_max_frame_num_minus4 + 4)
            .ok_or_else(truncated)?;
        let idr_pic_id = if is_idr {
            let id = bits.ue().ok_or_else(truncated)?;
            if id > 65535 {
                return Err(Error::InvalidFrame("H.264 IDR id is out of range"));
            }
            id
        } else {
            0
        };
        let poc_lsb = bits
            .bits(sps.log2_max_poc_lsb_minus4 + 4)
            .ok_or_else(truncated)?;
        let _delta_poc_bottom = if pps.bottom_field_order_present {
            bits.se().ok_or_else(truncated)?
        } else {
            0
        };
        if pps.redundant_present {
            let redundant = bits.ue().ok_or_else(truncated)?;
            if redundant != 0 {
                return Err(Error::Unsupported(
                    "redundant H.264 pictures are not supported",
                ));
            }
        }
        let mut num_ref_l0 = pps.num_ref_idx_l0_default + 1;
        if matches!(kind, SliceKind::P) && bits.bit().ok_or_else(truncated)? == 1 {
            num_ref_l0 = bits.ue().ok_or_else(truncated)? + 1;
            if num_ref_l0 == 0 || num_ref_l0 > 32 {
                return Err(Error::InvalidFrame("H.264 reference count is out of range"));
            }
        }
        if matches!(kind, SliceKind::P) && bits.bit().ok_or_else(truncated)? == 1 {
            // Modification operations are consumed so the reader stays
            // aligned, then refused: reordering needs long-term bookkeeping
            // this decoder does not keep.
            match bits.ue().ok_or_else(truncated)? {
                3 => {}
                0 | 1 => {
                    let _ = bits.ue().ok_or_else(truncated)?;
                    let _ = bits.ue().ok_or_else(truncated)?;
                    return Err(Error::Unsupported(
                        "H.264 reference list modification is not supported",
                    ));
                }
                2 => {
                    let _ = bits.ue().ok_or_else(truncated)?;
                    return Err(Error::Unsupported(
                        "H.264 long-term references are not supported",
                    ));
                }
                _ => {
                    return Err(Error::InvalidFrame(
                        "H.264 reference modification is unknown",
                    ));
                }
            }
        }
        let weights = if pps.weighted_pred && matches!(kind, SliceKind::P) {
            Some(parse_weights(&mut bits, num_ref_l0)?)
        } else {
            None
        };
        let (marking, no_output_prior) = if nal_ref_idc == 0 {
            (Marking::Sliding, false)
        } else if is_idr {
            let no_output_prior = bits.bit().ok_or_else(truncated)? != 0;
            if bits.bit().ok_or_else(truncated)? != 0 {
                return Err(Error::Unsupported(
                    "H.264 long-term references are not supported",
                ));
            }
            (Marking::Sliding, no_output_prior)
        } else if bits.bit().ok_or_else(truncated)? == 0 {
            (Marking::Sliding, false)
        } else {
            let mut diffs = Vec::new();
            let mut flush = false;
            let mut long_term = false;
            loop {
                match bits.ue().ok_or_else(truncated)? {
                    0 => break,
                    1 | 2 => {
                        diffs.push(bits.ue().ok_or_else(truncated)?);
                    }
                    3 => {
                        let _ = bits.ue().ok_or_else(truncated)?;
                        long_term = true;
                    }
                    4 => {
                        let _ = bits.ue().ok_or_else(truncated)?;
                    }
                    5 => flush = true,
                    6 => {
                        let _ = bits.ue().ok_or_else(truncated)?;
                        long_term = true;
                    }
                    _ => {
                        return Err(Error::InvalidFrame("H.264 reference marking is unknown"));
                    }
                }
            }
            if long_term {
                return Err(Error::Unsupported(
                    "H.264 long-term references are not supported",
                ));
            }
            (
                if flush {
                    Marking::FlushAll
                } else {
                    Marking::MarkNonRef { diffs }
                },
                false,
            )
        };
        let cabac_init = if pps.entropy_cabac && kind != SliceKind::I {
            let idc = bits.ue().ok_or_else(truncated)?;
            if idc > 2 {
                return Err(Error::InvalidFrame("H.264 CABAC init is out of range"));
            }
            idc as u8
        } else {
            0
        };
        let qp_delta = bits.se().ok_or_else(truncated)?;
        if !(-52..=51).contains(&qp_delta) {
            return Err(Error::InvalidFrame("H.264 QP delta is out of range"));
        }
        let (deblocking_idc, alpha_offset, beta_offset) = if pps.deblocking_present {
            let idc = bits.ue().ok_or_else(truncated)?;
            if idc > 2 {
                return Err(Error::InvalidFrame("H.264 deblocking mode is unknown"));
            }
            if idc != 1 {
                let alpha = bits.se().ok_or_else(truncated)?;
                let beta = bits.se().ok_or_else(truncated)?;
                if !(-6..=6).contains(&alpha) || !(-6..=6).contains(&beta) {
                    return Err(Error::InvalidFrame(
                        "H.264 deblocking offset is out of range",
                    ));
                }
                (idc as u8, alpha, beta)
            } else {
                (idc as u8, 0, 0)
            }
        } else {
            (0, 0, 0)
        };
        // Bit offset of the first macroblock row from the start of the
        // slice data, NAL header byte included: the VA-API slice buffer
        // carries the header too.
        let header_bits = u16::try_from(bits.position + 8)
            .map_err(|_| Error::InvalidFrame("H.264 slice header is too long"))?;
        Ok(Self {
            first_mb: u16::try_from(first_mb)
                .map_err(|_| Error::InvalidFrame("H.264 slice start is out of range"))?,
            kind,
            pps_id,
            frame_num,
            idr_pic_id,
            poc_lsb,
            num_ref_l0,
            weights,
            marking,
            no_output_prior,
            cabac_init,
            qp_delta,
            deblocking_idc,
            alpha_offset,
            beta_offset,
            header_bits,
        })
    }
}

fn parse_weights(bits: &mut Bits<'_>, refs: u32) -> Result<WeightTable, Error> {
    let luma_denom = bits.ue().ok_or_else(truncated)?;
    let chroma_denom = bits.ue().ok_or_else(truncated)?;
    if luma_denom > 7 || chroma_denom > 7 {
        return Err(Error::InvalidFrame(
            "H.264 weight denominator is out of range",
        ));
    }
    if refs > 32 {
        return Err(Error::InvalidFrame("H.264 reference count is out of range"));
    }
    let mut entries = Vec::with_capacity(refs as usize);
    for _ in 0..refs {
        let mut entry = WeightEntry::default();
        if bits.bit().ok_or_else(truncated)? != 0 {
            entry.luma_flag = true;
            entry.luma_weight = bits.se().ok_or_else(truncated)? as i16;
            entry.luma_offset = bits.se().ok_or_else(truncated)? as i16;
        }
        if bits.bit().ok_or_else(truncated)? != 0 {
            entry.chroma_flag = true;
            for plane in 0..2 {
                entry.chroma_weight[plane] = bits.se().ok_or_else(truncated)? as i16;
                entry.chroma_offset[plane] = bits.se().ok_or_else(truncated)? as i16;
            }
        }
        entries.push(entry);
    }
    Ok(WeightTable {
        luma_denom: luma_denom as u8,
        chroma_denom: chroma_denom as u8,
        entries,
    })
}
