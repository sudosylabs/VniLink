// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! Media Foundation H.264 samples are length-prefixed. SPS and PPS usually
//! sit on the output media type. The pump publishes Annex-B, and a keyframe
//! has to carry both parameter sets and an IDR.

#[cfg(any(windows, test))]
use vnilink::YuvMatrix;
#[cfg(any(windows, test))]
use vnilink_media::{parse_annex_b, to_annex_b, AvccAccessUnit};
use vnilink_media::{EncodedSample, Error};

/// An HEVC access unit from a hardware encoder. A keyframe that carries no
/// VPS, SPS and PPS of its own gets the encoder's `sequence_header` (Annex-B)
/// ahead of it.
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn hevc_access_unit(
    sample: &[u8],
    sequence_header: &[u8],
) -> Result<EncodedSample, Error> {
    match vnilink_media::hevc_sample(sample.to_vec()) {
        Err(Error::EncodeRejected(_)) if !sequence_header.is_empty() => {
            let mut joined = sequence_header.to_vec();
            joined.extend_from_slice(sample);
            vnilink_media::hevc_sample(joined)
        }
        other => other,
    }
}

/// An AV1 temporal unit from a hardware encoder: a keyframe when it carries a
/// sequence header OBU, as encoders write one with each.
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn av1_access_unit(sample: &[u8]) -> Result<EncodedSample, Error> {
    let key = av1_has_sequence_header(sample)?;
    EncodedSample::packet(vnilink::VideoCodec::Av1, key, sample.to_vec())
}

/// Whether the low-overhead OBU stream `unit` holds a sequence header
/// (type 1). Every OBU must carry its size; one that does not, or runs past
/// the unit, is malformed.
pub(crate) fn av1_has_sequence_header(unit: &[u8]) -> Result<bool, Error> {
    const MALFORMED: Error = Error::InvalidFrame("AV1 OBU");
    let mut offset = 0;
    let mut found = false;
    while offset < unit.len() {
        let header = unit[offset];
        let kind = (header >> 3) & 15;
        let extension = header & 4 != 0;
        if header & 2 == 0 {
            return Err(MALFORMED);
        }
        offset += 1 + usize::from(extension);
        let mut size = 0usize;
        for index in 0..8 {
            let byte = *unit.get(offset).ok_or(MALFORMED)?;
            offset += 1;
            size |= usize::from(byte & 0x7f) << (7 * index);
            if byte & 0x80 == 0 {
                break;
            }
            if index == 7 {
                return Err(MALFORMED);
            }
        }
        if size > unit.len() - offset.min(unit.len()) {
            return Err(MALFORMED);
        }
        found |= kind == 1;
        offset += size;
    }
    Ok(found)
}

/// `sequence_header` is `MF_MT_MPEG_SEQUENCE_HEADER`: Annex-B, or an AVC
/// decoder configuration record. An empty header is allowed for a predicted
/// frame.
#[cfg(any(windows, test))]
pub(crate) fn h264_access_unit(
    sample: &[u8],
    sequence_header: &[u8],
    matrix: YuvMatrix,
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
    EncodedSample::annex_b(keyframe, annex_b, matrix)
}

#[cfg(any(windows, test))]
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

#[cfg(any(windows, test))]
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

#[cfg(any(windows, test))]
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

#[cfg(any(windows, test))]
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

#[cfg(any(windows, test))]
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

#[cfg(any(windows, test))]
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

#[cfg(any(windows, test))]
fn is_annex_b(sample: &[u8]) -> bool {
    sample.starts_with(&[0, 0, 0, 1]) || sample.starts_with(&[0, 0, 1])
}

pub(crate) use vnilink_media::h264_picture_size as picture_size;

/// Camera transport requires a self-contained constrained-baseline IDR.
/// Validate the actual emitted SPS rather than trusting encoder attributes.
#[cfg(any(windows, test))]
pub(crate) fn camera_access_unit(bytes: &[u8]) -> Result<(), Error> {
    let parsed = parse_annex_b(bytes)?;
    if !parsed.has_idr() || !parsed.has_pps() || !parsed.has_sps() {
        return Err(Error::InvalidFrame(
            "camera keyframe needs SPS, PPS and IDR",
        ));
    }
    for unit in parsed.units().iter().filter(|unit| unit.nal_type == 7) {
        let sps = unit.bytes();
        if sps.len() < 4 || sps[1] != 66 || sps[2] & 0x40 == 0 {
            return Err(Error::Unsupported(
                "camera requires constrained baseline H.264",
            ));
        }
    }
    Ok(())
}
