//! Checked SPS geometry shared by decoders and recording.
use crate::Error;
/// Visible picture size from a sequence parameter set NAL unit, without the
/// header byte. The coded size rounds up to macroblocks; the crop rectangle
/// cuts the padding back off.
pub fn h264_picture_size(sps: &[u8]) -> Result<(u16, u16), Error> {
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
    bits.ue().ok_or_else(truncated)?;
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
    Ok((width, height))
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
}
