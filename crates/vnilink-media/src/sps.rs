//! Checked SPS geometry shared by decoders and recording.
use crate::Error;
/// Visible picture size from a sequence parameter set NAL unit, without the
/// header byte. The coded size rounds up to macroblocks; the crop rectangle
/// cuts the padding back off.
pub fn h264_picture_size(sps: &[u8]) -> Result<(u16, u16), Error> {
    parse(sps).map(|facts| facts.size)
}

/// What [`parse`] reads from an SPS beyond its size.
struct SpsFacts {
    size: (u16, u16),
    max_num_ref_frames: u32,
    /// Bit offset of `vui_parameters_present_flag` in the RBSP.
    vui_flag_at: usize,
    vui_present: bool,
}

/// An SPS that tells decoders they may show each picture as soon as it is
/// decoded, or `None` when `sps` already carries VUI or cannot be read.
///
/// Without VUI a decoder must assume pictures may be reordered and can hold
/// back a whole picture buffer before showing one; VideoToolbox writes no VUI
/// even with reordering turned off. This adds VUI with only
/// `bitstream_restriction`: no reordering, and a decoded-picture buffer of
/// the stream's reference frames, as Media Foundation's low-delay SPS says.
pub fn h264_low_delay_sps(sps: &[u8]) -> Option<Vec<u8>> {
    let facts = parse(sps).ok()?;
    if facts.vui_present {
        return None;
    }
    let payload = rbsp(&sps[1..]);
    let mut out = BitWriter::default();
    let mut bits = Bits::new(&payload);
    for _ in 0..facts.vui_flag_at {
        out.bit(bits.bit()?);
    }
    out.bit(1); // vui_parameters_present_flag
                // aspect ratio, overscan, video signal, chroma location, timing, NAL
                // HRD, VCL HRD and picture structure: none.
    for _ in 0..8 {
        out.bit(0);
    }
    out.bit(1); // bitstream_restriction_flag
    out.bit(1); // motion_vectors_over_pic_boundaries_flag
    out.ue(2); // max_bytes_per_pic_denom
    out.ue(1); // max_bits_per_mb_denom
    out.ue(16); // log2_max_mv_length_horizontal
    out.ue(16); // log2_max_mv_length_vertical
    out.ue(0); // max_num_reorder_frames
    out.ue(facts.max_num_ref_frames.max(1)); // max_dec_frame_buffering
    out.bit(1); // rbsp_stop_one_bit
    let mut nal = vec![sps[0]];
    nal.extend(emulation_prevented(&out.finish()));
    Some(nal)
}

fn parse(sps: &[u8]) -> Result<SpsFacts, Error> {
    if sps.len() > 65535 {
        return Err(Error::InvalidFrame("H.264 SPS is too large"));
    }
    let truncated = || Error::InvalidFrame("H.264 SPS is truncated");
    let header = sps
        .first()
        .ok_or(Error::InvalidFrame("H.264 SPS is empty"))?;
    if header & 0x1F != 7 {
        return Err(Error::InvalidFrame("H.264 NAL unit is not an SPS"));
    }
    let payload = rbsp(&sps[1..]);
    let mut bits = Bits::new(&payload);
    let profile = bits.u8().ok_or_else(truncated)?;
    bits.u8().ok_or_else(truncated)?;
    bits.u8().ok_or_else(truncated)?;
    // The sequence id precedes the profile-specific fields.
    bits.ue().ok_or_else(truncated)?;
    // 4:2:0 unless the high-profile prefix says otherwise.
    let mut chroma = (1u32, 2u32, 2u32);
    if matches!(
        profile,
        100 | 110 | 122 | 244 | 44 | 83 | 86 | 118 | 128 | 138 | 139 | 134
    ) {
        let chroma_format = bits.ue().ok_or_else(truncated)?;
        if chroma_format == 3 && bits.bit().ok_or_else(truncated)? == 1 {
            return Err(Error::Unsupported("separate colour planes"));
        }
        chroma = match chroma_format {
            0 => (0, 1, 1),
            1 => (1, 2, 2),
            2 => (2, 2, 1),
            3 => (3, 1, 1),
            _ => return Err(Error::InvalidFrame("H.264 chroma format is invalid")),
        };
        bits.ue().ok_or_else(truncated)?;
        bits.ue().ok_or_else(truncated)?;
        bits.bit().ok_or_else(truncated)?;
        if bits.bit().ok_or_else(truncated)? == 1 {
            let lists = if chroma.0 == 3 { 12 } else { 8 };
            for list in 0..lists {
                if bits.bit().ok_or_else(truncated)? == 1 {
                    skip_scaling_list(&mut bits, if list < 6 { 16 } else { 64 })?;
                }
            }
        }
    }
    bits.ue().ok_or_else(truncated)?;
    match bits.ue().ok_or_else(truncated)? {
        0 => {
            bits.ue().ok_or_else(truncated)?;
        }
        1 => {
            bits.bit().ok_or_else(truncated)?;
            bits.se().ok_or_else(truncated)?;
            bits.se().ok_or_else(truncated)?;
            let cycles = bits.ue().ok_or_else(truncated)?;
            if cycles > 255 {
                return Err(Error::InvalidFrame("H.264 SPS cycle count is invalid"));
            }
            for _ in 0..cycles {
                bits.se().ok_or_else(truncated)?;
            }
        }
        // H.264 7.3.2.1.1: type 2 has no additional picture-order fields.
        // Media Foundation emits it for low-delay Baseline streams.
        2 => {}
        _ => return Err(Error::InvalidFrame("H.264 SPS picture order is unknown")),
    }
    let max_num_ref_frames = bits.ue().ok_or_else(truncated)?;
    bits.bit().ok_or_else(truncated)?;
    let width_mbs = bits.ue().ok_or_else(truncated)?;
    let height_maps = bits.ue().ok_or_else(truncated)?;
    let frame_only = bits.bit().ok_or_else(truncated)?;
    if frame_only == 0 {
        bits.bit().ok_or_else(truncated)?;
    }
    bits.bit().ok_or_else(truncated)?;
    let (crop_left, crop_right, crop_top, crop_bottom) = if bits.bit().ok_or_else(truncated)? == 1 {
        (
            bits.ue().ok_or_else(truncated)?,
            bits.ue().ok_or_else(truncated)?,
            bits.ue().ok_or_else(truncated)?,
            bits.ue().ok_or_else(truncated)?,
        )
    } else {
        (0, 0, 0, 0)
    };
    let vui_flag_at = bits.at;
    let vui_present = bits.bit().is_some_and(|flag| flag == 1);
    let invalid = || Error::InvalidFrame("H.264 SPS picture size is out of range");
    let coded_width = width_mbs
        .checked_add(1)
        .and_then(|v| v.checked_mul(16))
        .ok_or_else(invalid)?;
    let coded_height = height_maps
        .checked_add(1)
        .and_then(|v| v.checked_mul(16 * (2 - frame_only)))
        .ok_or_else(invalid)?;
    let crop_x = crop_left
        .checked_add(crop_right)
        .and_then(|v| v.checked_mul(chroma.1))
        .ok_or_else(invalid)?;
    let crop_y = crop_top
        .checked_add(crop_bottom)
        .and_then(|v| v.checked_mul(chroma.2 * (2 - frame_only)))
        .ok_or_else(invalid)?;
    let width = coded_width.checked_sub(crop_x).ok_or_else(invalid)?;
    let height = coded_height.checked_sub(crop_y).ok_or_else(invalid)?;
    let (width, height) = (
        u16::try_from(width)
            .map_err(|_| Error::InvalidFrame("H.264 SPS picture size is out of range"))?,
        u16::try_from(height)
            .map_err(|_| Error::InvalidFrame("H.264 SPS picture size is out of range"))?,
    );
    if width == 0 || height == 0 {
        return Err(Error::InvalidFrame("H.264 SPS picture size is empty"));
    }
    Ok(SpsFacts {
        size: (width, height),
        max_num_ref_frames,
        vui_flag_at,
        vui_present,
    })
}

/// Inserts emulation-prevention `0x03` wherever two zero bytes precede a byte of 3 or less.
fn emulation_prevented(rbsp: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(rbsp.len() + rbsp.len() / 64 + 1);
    let mut zeros = 0u8;
    for &byte in rbsp {
        if zeros == 2 && byte <= 3 {
            out.push(3);
            zeros = 0;
        }
        out.push(byte);
        zeros = if byte == 0 { (zeros + 1).min(2) } else { 0 };
    }
    out
}

/// Big-endian bit writer for one RBSP.
#[derive(Default)]
struct BitWriter {
    bytes: Vec<u8>,
    used: u32,
}

impl BitWriter {
    fn bit(&mut self, bit: u32) {
        if self.used % 8 == 0 {
            self.bytes.push(0);
        }
        if bit != 0 {
            if let Some(last) = self.bytes.last_mut() {
                *last |= 0x80 >> (self.used % 8);
            }
        }
        self.used += 1;
    }

    /// Unsigned Exp-Golomb.
    fn ue(&mut self, value: u32) {
        let code = u64::from(value) + 1;
        let width = 64 - code.leading_zeros();
        for _ in 1..width {
            self.bit(0);
        }
        for shift in (0..width).rev() {
            self.bit(u32::from((code >> shift) & 1 == 1));
        }
    }

    /// The bytes, padded with zero bits to a byte boundary.
    fn finish(self) -> Vec<u8> {
        self.bytes
    }
}

/// Copies bytes while dropping emulation-prevention `0x03` after `0x00 0x00`.
fn rbsp(nal: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(nal.len());
    let mut zeros = 0u8;
    for &byte in nal {
        if zeros == 2 && byte == 0x03 {
            zeros = 0;
            continue;
        }
        out.push(byte);
        zeros = if byte == 0 { (zeros + 1).min(2) } else { 0 };
    }
    out
}

fn skip_scaling_list(bits: &mut Bits<'_>, size: u32) -> Result<(), Error> {
    let mut next = 8u32;
    for _ in 0..size {
        if next != 0 {
            let delta = bits
                .se()
                .ok_or(Error::InvalidFrame("H.264 SPS is truncated"))?;
            next = next.wrapping_add(delta as u32) & 0xFF;
        }
    }
    Ok(())
}

/// Big-endian bit reader over one RBSP. Short reads return `None`.
struct Bits<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Bits<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, at: 0 }
    }

    fn bit(&mut self) -> Option<u32> {
        self.u(1)
    }

    fn u8(&mut self) -> Option<u8> {
        self.u(8).and_then(|value| u8::try_from(value).ok())
    }

    fn u(&mut self, width: u32) -> Option<u32> {
        if width == 0 || width > 32 {
            return None;
        }
        let mut value = 0u32;
        for _ in 0..width {
            let byte = *self.bytes.get(self.at / 8)?;
            value = value.checked_shl(1)? | u32::from((byte >> (7 - self.at % 8)) & 1);
            self.at += 1;
        }
        Some(value)
    }

    /// Unsigned Exp-Golomb.
    fn ue(&mut self) -> Option<u32> {
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
        let rest = self.u(zeros)?;
        rest.checked_add((1 << zeros) - 1)
    }

    /// Signed Exp-Golomb.
    fn se(&mut self) -> Option<i32> {
        let code = self.ue()?;
        if code % 2 == 0 {
            Some(-(i32::try_from(code / 2).ok()?))
        } else {
            Some(i32::try_from(code / 2 + 1).ok()?)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn baseline_type_two_geometry_is_valid_and_unknown_order_is_rejected() {
        // Baseline, sequence 0, four-bit frame numbers, POC type 2, one
        // reference, 4x4 macroblocks, progressive, no crop or VUI.
        let valid = [0x67, 0x42, 0x00, 0x1e, 0xda, 0x10, 0x99];
        assert_eq!(h264_picture_size(&valid).unwrap(), (64, 64));
        let unknown = [0x67, 0x42, 0x00, 0x1e, 0xc8, 0x84, 0x26, 0x40];
        assert!(matches!(
            h264_picture_size(&unknown),
            Err(Error::InvalidFrame("H.264 SPS picture order is unknown"))
        ));
        assert!(matches!(
            h264_picture_size(&valid[..5]),
            Err(Error::InvalidFrame("H.264 SPS is truncated"))
        ));
    }

    #[test]
    fn a_low_delay_sps_keeps_the_picture_and_says_nothing_is_reordered() {
        let valid = [0x67, 0x42, 0x00, 0x1e, 0xda, 0x10, 0x99];
        let low = h264_low_delay_sps(&valid).unwrap();
        // The same sequence, now with VUI that a decoder reads back.
        assert_eq!(h264_picture_size(&low).unwrap(), (64, 64));
        let facts = parse(&low).unwrap();
        assert!(facts.vui_present);
        assert_eq!(facts.max_num_ref_frames, 1);
        let payload = rbsp(&low[1..]);
        let mut bits = Bits::new(&payload);
        bits.at = facts.vui_flag_at + 1;
        for _ in 0..8 {
            assert_eq!(bits.bit(), Some(0));
        }
        assert_eq!(bits.bit(), Some(1), "bitstream_restriction_flag");
        assert_eq!(bits.bit(), Some(1));
        assert_eq!(
            [
                bits.ue(),
                bits.ue(),
                bits.ue(),
                bits.ue(),
                bits.ue(),
                bits.ue()
            ],
            [Some(2), Some(1), Some(16), Some(16), Some(0), Some(1)]
        );
        assert_eq!(bits.bit(), Some(1), "rbsp_stop_one_bit");
        // VideoToolbox's 720p Constrained Baseline SPS, which has no VUI.
        let videotoolbox = [0x27, 0x42, 0xc0, 0x1f, 0xab, 0x40, 0x28, 0x02, 0xdc, 0x80];
        let low_vt = h264_low_delay_sps(&videotoolbox).unwrap();
        assert_eq!(h264_picture_size(&low_vt).unwrap(), (1280, 720));
        assert!(parse(&low_vt).unwrap().vui_present);
        // The bytes ffmpeg reads back as max_num_reorder_frames 0, max_dec_frame_buffering 1.
        assert_eq!(
            low_vt,
            [0x27, 0x42, 0xc0, 0x1f, 0xab, 0x40, 0x28, 0x02, 0xdd, 0x00, 0xda, 0x08, 0x84, 0x6a]
        );
        // An SPS that already has VUI is left as its encoder wrote it.
        assert_eq!(h264_low_delay_sps(&low), None);
        assert_eq!(h264_low_delay_sps(&valid[..5]), None);
    }

    #[test]
    fn written_bits_round_trip_and_zero_runs_are_escaped() {
        let mut out = BitWriter::default();
        for value in [0, 1, 2, 7, 255, 65535] {
            out.ue(value);
        }
        let bytes = out.finish();
        let mut bits = Bits::new(&bytes);
        for value in [0, 1, 2, 7, 255, 65535] {
            assert_eq!(bits.ue(), Some(value));
        }
        let escaped = emulation_prevented(&[0, 0, 1, 0, 0, 0, 5, 0, 0]);
        assert_eq!(escaped, [0, 0, 3, 1, 0, 0, 3, 0, 5, 0, 0]);
        assert_eq!(rbsp(&escaped), [0, 0, 1, 0, 0, 0, 5, 0, 0]);
    }
}
