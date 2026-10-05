//! Media Foundation H.264 samples are length-prefixed. SPS and PPS usually
//! sit on the output media type. The pump publishes Annex-B, and a keyframe
//! has to carry both parameter sets and an IDR.

use vnilink_media::{parse_annex_b, to_annex_b, AvccAccessUnit, EncodedSample, Error};

/// `sequence_header` is `MF_MT_MPEG_SEQUENCE_HEADER`: Annex-B, or an AVC
/// decoder configuration record. An empty header is allowed for a predicted
/// frame.
pub(crate) fn h264_access_unit(
    sample: &[u8],
    sequence_header: &[u8],
) -> Result<EncodedSample, Error> {
    if sample.is_empty() {
        return Err(Error::InvalidFrame("encoded frame is empty"));
    }
    let sets = parameter_sets(sequence_header)?;
    let annex_b = if is_annex_b(sample) {
        with_parameter_sets(sample, &sets)?
    } else {
        avcc_to_annex_b(sample, &sets)?
    };
    let parsed = parse_annex_b(&annex_b)?;
    let keyframe = parsed.has_sps() && parsed.has_pps() && parsed.has_idr();
    EncodedSample::annex_b(keyframe, annex_b)
}

fn avcc_to_annex_b(sample: &[u8], sets: &[Vec<u8>]) -> Result<Vec<u8>, Error> {
    let idr = avcc_has_type(sample, 5)?;
    let in_band = avcc_has_type(sample, 7)? && avcc_has_type(sample, 8)?;
    let prepend = idr && !in_band;
    if prepend && sets.is_empty() {
        return Err(Error::EncodeRejected(
            "a keyframe needs SPS, PPS, and an IDR",
        ));
    }
    to_annex_b(&AvccAccessUnit {
        nal_length_size: 4,
        avcc: sample,
        parameter_sets: if prepend { sets } else { &[] },
        keyframe: prepend,
    })
}

fn with_parameter_sets(sample: &[u8], sets: &[Vec<u8>]) -> Result<Vec<u8>, Error> {
    let parsed = parse_annex_b(sample)?;
    if !parsed.has_idr() || (parsed.has_sps() && parsed.has_pps()) {
        return Ok(sample.to_vec());
    }
    if sets.is_empty() {
        return Err(Error::EncodeRejected(
            "a keyframe needs SPS, PPS, and an IDR",
        ));
    }
    let mut out = Vec::new();
    for set in sets {
        out.extend_from_slice(&[0, 0, 0, 1]);
        out.extend_from_slice(set);
    }
    out.extend_from_slice(sample);
    Ok(out)
}

fn parameter_sets(header: &[u8]) -> Result<Vec<Vec<u8>>, Error> {
    if header.is_empty() {
        return Ok(Vec::new());
    }
    if is_annex_b(header) {
        let parsed = parse_annex_b(header)?;
        return Ok(parsed
            .units()
            .iter()
            .filter(|unit| unit.nal_type == 7 || unit.nal_type == 8)
            .map(|unit| unit.bytes().to_vec())
            .collect());
    }
    if header[0] == 1 {
        return avc_decoder_config(header);
    }
    Err(Error::InvalidFrame("H.264 sequence header is not Annex-B"))
}

fn avc_decoder_config(header: &[u8]) -> Result<Vec<Vec<u8>>, Error> {
    if header.len() < 7 || header[0] != 1 {
        return Err(Error::InvalidFrame("H.264 sequence header is not Annex-B"));
    }
    let length_size = (header[4] & 0x03) + 1;
    if !matches!(length_size, 1 | 2 | 4) {
        return Err(Error::InvalidFrame(
            "H.264 length prefix must be 1, 2, or 4 bytes",
        ));
    }
    let mut index = 5;
    let sps_count = usize::from(header[index] & 0x1F);
    index += 1;
    let mut sets = Vec::with_capacity(sps_count);
    for _ in 0..sps_count {
        sets.push(read_counted_nal(header, &mut index)?);
    }
    if index >= header.len() {
        return Err(Error::InvalidFrame("H.264 sequence header is truncated"));
    }
    let pps_count = usize::from(header[index]);
    index += 1;
    for _ in 0..pps_count {
        sets.push(read_counted_nal(header, &mut index)?);
    }
    Ok(sets)
}

fn read_counted_nal(header: &[u8], index: &mut usize) -> Result<Vec<u8>, Error> {
    if header.len() < index.saturating_add(2) {
        return Err(Error::InvalidFrame("H.264 sequence header is truncated"));
    }
    let len = usize::from(u16::from_be_bytes([header[*index], header[*index + 1]]));
    *index += 2;
    if len == 0 || header.len() < index.saturating_add(len) {
        return Err(Error::InvalidFrame("H.264 sequence header is truncated"));
    }
    let nal = header[*index..*index + len].to_vec();
    *index += len;
    Ok(nal)
}

fn avcc_has_type(sample: &[u8], nal_type: u8) -> Result<bool, Error> {
    let mut rest = sample;
    let mut found = false;
    while !rest.is_empty() {
        if rest.len() < 4 {
            return Err(Error::InvalidFrame("H.264 length prefix is truncated"));
        }
        let len = u32::from_be_bytes([rest[0], rest[1], rest[2], rest[3]]) as usize;
        rest = &rest[4..];
        if len == 0 || rest.len() < len {
            return Err(Error::InvalidFrame("H.264 NAL unit is truncated"));
        }
        if rest[0] & 0x1F == nal_type {
            found = true;
        }
        rest = &rest[len..];
    }
    Ok(found)
}

fn is_annex_b(sample: &[u8]) -> bool {
    sample.starts_with(&[0, 0, 0, 1]) || sample.starts_with(&[0, 0, 1])
}

/// Visible picture size from a sequence parameter set NAL unit, without the
/// header byte. The coded size rounds up to macroblocks; the crop rectangle
/// cuts the padding back off.
pub(crate) fn picture_size(sps: &[u8]) -> Result<(u16, u16), Error> {
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
            _ => (3, 1, 1),
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
            for _ in 0..cycles {
                bits.se().ok_or_else(truncated)?;
            }
        }
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
    let coded_width = (width_mbs + 1).checked_mul(16).ok_or(Error::InvalidFrame(
        "H.264 SPS picture size is out of range",
    ))?;
    let coded_height =
        (height_maps + 1)
            .checked_mul(16 * (2 - frame_only))
            .ok_or(Error::InvalidFrame(
                "H.264 SPS picture size is out of range",
            ))?;
    let width = coded_width.saturating_sub((crop_left + crop_right) * chroma.1);
    let height = coded_height.saturating_sub((crop_top + crop_bottom) * chroma.2);
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
        zeros = if byte == 0 { zeros + 1 } else { 0 };
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
