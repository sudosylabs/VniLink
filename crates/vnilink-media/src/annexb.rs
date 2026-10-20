//! H.264 access units as Annex-B.
//!
//! VideoToolbox emits length-prefixed NALs and keeps SPS and PPS beside the
//! sample. A Windows or Linux decoder cannot start from those bytes alone.
//! [`to_annex_b`] writes start codes and, for a keyframe, the parameter sets
//! ahead of the picture. Payloads are screen content: do not log them.

use vnilink::MAX_VIDEO_FRAME;

use crate::error::Error;

const START_CODE: [u8; 4] = [0, 0, 0, 1];

/// One length-prefixed H.264 sample plus the parameter sets that describe it.
pub struct AvccAccessUnit<'a> {
    /// Big-endian length of each NAL in [`Self::avcc`]. Only 1, 2, or 4.
    pub nal_length_size: u8,
    /// Length-prefixed NAL units, without start codes.
    pub avcc: &'a [u8],
    /// SPS, PPS, and any other parameter sets, without start codes.
    pub parameter_sets: &'a [Vec<u8>],
    /// When true, the parameter sets are written in front of the picture.
    pub keyframe: bool,
}

/// One NAL unit without its start code.
pub struct NalUnit {
    pub nal_type: u8,
    bytes: Vec<u8>,
}

impl NalUnit {
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }
}

impl std::fmt::Debug for NalUnit {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("NalUnit")
            .field("nal_type", &self.nal_type)
            .field("len", &self.bytes.len())
            .finish()
    }
}

/// Annex-B NALs split out of one access unit.
#[derive(Debug)]
pub struct ParsedAccessUnit {
    units: Vec<NalUnit>,
}

impl ParsedAccessUnit {
    pub fn units(&self) -> &[NalUnit] {
        &self.units
    }

    pub fn has_sps(&self) -> bool {
        self.has_type(7)
    }

    pub fn has_pps(&self) -> bool {
        self.has_type(8)
    }

    pub fn has_idr(&self) -> bool {
        self.has_type(5)
    }

    fn has_type(&self, nal_type: u8) -> bool {
        self.units.iter().any(|unit| unit.nal_type == nal_type)
    }
}

/// Convert length-prefixed NALs to Annex-B.
///
/// A keyframe is prefixed with each parameter set, in order. A predicted
/// frame does not repeat them.
pub fn to_annex_b(unit: &AvccAccessUnit<'_>) -> Result<Vec<u8>, Error> {
    if !matches!(unit.nal_length_size, 1 | 2 | 4) {
        return Err(Error::InvalidFrame(
            "H.264 length prefix must be 1, 2, or 4 bytes",
        ));
    }
    if unit.avcc.is_empty() {
        return Err(Error::InvalidFrame("encoded frame is empty"));
    }
    if unit.avcc.len() > MAX_VIDEO_FRAME {
        return Err(Error::InvalidFrame(crate::error::FRAME_TOO_LARGE));
    }
    if unit.keyframe && unit.parameter_sets.is_empty() {
        return Err(Error::InvalidFrame("a keyframe needs its parameter sets"));
    }

    let mut out = Vec::new();
    if unit.keyframe {
        for set in unit.parameter_sets {
            push_nal(&mut out, set)?;
        }
    }
    let size = usize::from(unit.nal_length_size);
    let mut rest = unit.avcc;
    while !rest.is_empty() {
        if rest.len() < size {
            return Err(Error::InvalidFrame("H.264 length prefix is truncated"));
        }
        let nal_len = read_len(rest, size);
        rest = &rest[size..];
        if nal_len == 0 {
            return Err(Error::InvalidFrame("an H.264 NAL unit is empty"));
        }
        if rest.len() < nal_len {
            return Err(Error::InvalidFrame("H.264 NAL unit is truncated"));
        }
        push_nal(&mut out, &rest[..nal_len])?;
        rest = &rest[nal_len..];
    }
    if out.len() > MAX_VIDEO_FRAME {
        return Err(Error::InvalidFrame(crate::error::FRAME_TOO_LARGE));
    }
    if out.is_empty() {
        return Err(Error::InvalidFrame("encoded frame is empty"));
    }
    Ok(out)
}

/// Split Annex-B start codes into NAL units.
pub fn parse_annex_b(data: &[u8]) -> Result<ParsedAccessUnit, Error> {
    if data.is_empty() || data.len() > MAX_VIDEO_FRAME {
        return Err(Error::InvalidFrame(if data.is_empty() {
            "encoded frame is empty"
        } else {
            crate::error::FRAME_TOO_LARGE
        }));
    }
    let mut units = Vec::new();
    let mut index = 0;
    while index + 3 <= data.len() {
        let Some(start_len) = start_code_len(data, index) else {
            index += 1;
            continue;
        };
        let nal_start = index + start_len;
        let nal_end = find_start_code(data, nal_start).unwrap_or(data.len());
        if nal_end == nal_start {
            return Err(Error::InvalidFrame("an H.264 NAL unit is empty"));
        }
        let bytes = data[nal_start..nal_end].to_vec();
        units.push(NalUnit {
            nal_type: bytes[0] & 0x1F,
            bytes,
        });
        index = nal_end;
    }
    if units.is_empty() {
        return Err(Error::InvalidFrame("H.264 frame is not Annex-B"));
    }
    Ok(ParsedAccessUnit { units })
}

fn push_nal(out: &mut Vec<u8>, nal: &[u8]) -> Result<(), Error> {
    if nal.is_empty() {
        return Err(Error::InvalidFrame("an H.264 NAL unit is empty"));
    }
    if nal.starts_with(&[0, 0, 0, 1]) || nal.starts_with(&[0, 0, 1]) {
        return Err(Error::InvalidFrame(
            "an H.264 NAL unit already has a start code",
        ));
    }
    if out.len().saturating_add(START_CODE.len() + nal.len()) > MAX_VIDEO_FRAME {
        return Err(Error::InvalidFrame(crate::error::FRAME_TOO_LARGE));
    }
    out.extend_from_slice(&START_CODE);
    out.extend_from_slice(nal);
    Ok(())
}

fn read_len(data: &[u8], size: usize) -> usize {
    match size {
        1 => usize::from(data[0]),
        2 => usize::from(u16::from_be_bytes([data[0], data[1]])),
        _ => u32::from_be_bytes([data[0], data[1], data[2], data[3]]) as usize,
    }
}

fn start_code_len(data: &[u8], index: usize) -> Option<usize> {
    if index + 4 <= data.len() && data[index..index + 4] == START_CODE {
        Some(4)
    } else if index + 3 <= data.len() && data[index..index + 3] == [0, 0, 1] {
        Some(3)
    } else {
        None
    }
}

fn find_start_code(data: &[u8], from: usize) -> Option<usize> {
    let mut index = from;
    while index + 3 <= data.len() {
        if start_code_len(data, index).is_some() {
            return Some(index);
        }
        index += 1;
    }
    None
}
