// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! HEVC Annex-B access units, including self-contained IRAP keyframes.
use crate::Error;
use vnilink::MAX_VIDEO_FRAME;
pub fn hevc_units(bytes: &[u8]) -> Result<Vec<&[u8]>, Error> {
    if bytes.is_empty() || bytes.len() > MAX_VIDEO_FRAME {
        return Err(Error::InvalidFrame("HEVC packet size"));
    }
    let mut starts = Vec::new();
    let mut i = 0;
    while i + 3 <= bytes.len() {
        let size = if bytes[i..].starts_with(&[0, 0, 0, 1]) {
            4
        } else if bytes[i..].starts_with(&[0, 0, 1]) {
            3
        } else {
            0
        };
        if size > 0 {
            starts.push((i, i + size));
            i += size;
        } else {
            i += 1;
        }
    }
    if starts.first().is_none_or(|s| s.0 != 0) {
        return Err(Error::InvalidFrame("HEVC start code"));
    }
    let mut units = Vec::new();
    for (index, (_, start)) in starts.iter().enumerate() {
        let end = starts.get(index + 1).map(|s| s.0).unwrap_or(bytes.len());
        let nal = &bytes[*start..end];
        if nal.len() < 2 || nal[0] & 0x80 != 0 || nal[1] & 7 == 0 {
            return Err(Error::InvalidFrame("HEVC NAL header"));
        }
        units.push(nal);
    }
    Ok(units)
}
pub fn hevc_sample(bytes: Vec<u8>) -> Result<crate::EncodedSample, Error> {
    let units = hevc_units(&bytes)?;
    let kind = |nal: &&[u8]| (nal[0] >> 1) & 63;
    let key = units.iter().any(|nal| (16..=21).contains(&kind(nal)));
    if key
        && ![32, 33, 34]
            .into_iter()
            .all(|expected| units.iter().any(|nal| kind(nal) == expected))
    {
        return Err(Error::EncodeRejected(
            "HEVC keyframe needs VPS, SPS and PPS",
        ));
    }
    crate::EncodedSample::packet(vnilink::VideoCodec::H265, key, bytes)
}
pub fn hevc_annex_b(
    data: &[u8],
    length_size: u8,
    sets: &[Vec<u8>],
) -> Result<crate::EncodedSample, Error> {
    if !matches!(length_size, 1 | 2 | 4) {
        return Err(Error::InvalidFrame("HEVC length prefix"));
    }
    let mut picture = Vec::new();
    let mut offset = 0;
    let mut key = false;
    while offset < data.len() {
        let size = length_size as usize;
        if offset + size > data.len() {
            return Err(Error::InvalidFrame("HEVC truncated length"));
        }
        let len = data[offset..offset + size]
            .iter()
            .fold(0usize, |n, b| (n << 8) | *b as usize);
        offset += size;
        if len < 2 || len > data.len() - offset {
            return Err(Error::InvalidFrame("HEVC truncated NAL"));
        }
        let nal = &data[offset..offset + len];
        key |= (16..=21).contains(&((nal[0] >> 1) & 63));
        picture.extend_from_slice(&[0, 0, 0, 1]);
        picture.extend_from_slice(nal);
        offset += len;
    }
    let mut output = Vec::new();
    if key {
        for set in sets {
            if set.len() < 2 {
                return Err(Error::InvalidFrame("HEVC parameter set"));
            }
            output.extend_from_slice(&[0, 0, 0, 1]);
            output.extend_from_slice(set);
        }
    }
    output.extend_from_slice(&picture);
    hevc_sample(output)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_non_self_contained_keyframe_and_truncated_lengths() {
        assert!(hevc_sample(vec![0, 0, 0, 1, 38, 1, 4]).is_err());
        assert!(hevc_annex_b(&[0, 0, 0, 10, 38, 1], 4, &[]).is_err());
        assert!(hevc_units(&[0, 0, 0, 1, 0x80, 1]).is_err());
    }
}
