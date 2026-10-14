//! Codec-specific Matroska initialization and access units; no filesystem work.
use crate::{RecordingError, VideoDecoder};
use vnilink::{EncodedFrame, VideoCodec};
fn invalid() -> RecordingError {
    RecordingError::InvalidMedia
}
pub(crate) struct RecordingFormat {
    pub codec: VideoCodec,
    pub private: Vec<u8>,
    sets: Vec<Vec<u8>>,
    size: (u16, u16),
}
impl RecordingFormat {
    pub fn open(first: &EncodedFrame, size: (u16, u16)) -> Result<Self, RecordingError> {
        if first.payload.is_empty()
            || first.payload.len() > vnilink::MAX_VIDEO_FRAME
            || !first.keyframe
            || picture_size(first)? != size
        {
            return Err(invalid());
        }
        let mut sets = Vec::new();
        let private = match first.codec {
            VideoCodec::H264 => {
                let unit = crate::parse_annex_b(&first.payload).map_err(|_| invalid())?;
                let sps = unit
                    .units()
                    .iter()
                    .find(|u| u.nal_type == 7)
                    .ok_or_else(invalid)?
                    .bytes();
                let pps = unit
                    .units()
                    .iter()
                    .find(|u| u.nal_type == 8)
                    .ok_or_else(invalid)?
                    .bytes();
                if sps.len() < 4 || sps.len() > 65535 || pps.len() > 65535 {
                    return Err(invalid());
                }
                sets = vec![sps.to_vec(), pps.to_vec()];
                let mut data = vec![1, sps[1], sps[2], sps[3], 0xff, 0xe1];
                data.extend((sps.len() as u16).to_be_bytes());
                data.extend(sps);
                data.push(1);
                data.extend((pps.len() as u16).to_be_bytes());
                data.extend(pps);
                data
            }
            VideoCodec::H265 => {
                let units = crate::hevc_units(&first.payload).map_err(|_| invalid())?;
                for kind in [32, 33, 34] {
                    sets.push(
                        units
                            .iter()
                            .find(|n| (n[0] >> 1) & 63 == kind)
                            .ok_or_else(invalid)?
                            .to_vec(),
                    );
                }
                let rbsp = rbsp(&sets[1][2..]);
                if rbsp.len() < 13 {
                    return Err(invalid());
                }
                let mut data = vec![1];
                data.extend_from_slice(&rbsp[1..13]);
                data.extend_from_slice(&[
                    0xf0,
                    0,
                    0xfc,
                    0xfd,
                    0xf8,
                    0xf8,
                    0,
                    0,
                    (((rbsp[0] >> 1) & 7) + 1) << 3 | (rbsp[0] & 1) << 2 | 3,
                    3,
                ]);
                for (kind, set) in [32u8, 33, 34].into_iter().zip(&sets) {
                    if set.len() > 65535 {
                        return Err(invalid());
                    }
                    data.push(0x80 | kind);
                    data.extend(1u16.to_be_bytes());
                    data.extend((set.len() as u16).to_be_bytes());
                    data.extend(set);
                }
                data
            }
            VideoCodec::Av1 => {
                let sequence = av1_sequence(&first.payload)?;
                let info = av1_info(sequence.1)?;
                let mut data = vec![
                    0x81,
                    (info.profile << 5) | info.level,
                    (info.tier << 7)
                        | (info.high << 6)
                        | (info.twelve << 5)
                        | (info.mono << 4)
                        | (info.x << 3)
                        | (info.y << 2)
                        | info.position,
                    0,
                ];
                data.extend(sequence.0);
                sets.push(sequence.0.to_vec());
                data
            }
            VideoCodec::Vp8 | VideoCodec::Vp9 => Vec::new(),
        };
        Ok(Self {
            codec: first.codec,
            private,
            sets,
            size,
        })
    }
    pub fn id(&self) -> &'static [u8] {
        match self.codec {
            VideoCodec::H264 => b"V_MPEG4/ISO/AVC",
            VideoCodec::H265 => b"V_MPEGH/ISO/HEVC",
            VideoCodec::Vp8 => b"V_VP8",
            VideoCodec::Vp9 => b"V_VP9",
            VideoCodec::Av1 => b"V_AV1",
        }
    }
    pub fn picture(&self, frame: &EncodedFrame) -> Result<Vec<u8>, RecordingError> {
        if frame.codec != self.codec {
            return Err(RecordingError::FormatChanged);
        }
        if frame.keyframe {
            let next = Self::open(frame, picture_size(frame)?)?;
            if next.size != self.size || next.private != self.private {
                return Err(RecordingError::FormatChanged);
            }
        }
        match self.codec {
            VideoCodec::H264 => {
                let unit = crate::parse_annex_b(&frame.payload).map_err(|_| invalid())?;
                if unit.has_idr() != frame.keyframe {
                    return Err(invalid());
                }
                let mut picture = Vec::new();
                for nal in unit.units() {
                    if matches!(nal.nal_type, 7 | 8) {
                        let expected = if nal.nal_type == 7 {
                            &self.sets[0]
                        } else {
                            &self.sets[1]
                        };
                        if nal.bytes() != expected {
                            return Err(RecordingError::FormatChanged);
                        }
                        continue;
                    }
                    if nal.nal_type == 9 {
                        continue;
                    }
                    picture.extend((nal.bytes().len() as u32).to_be_bytes());
                    picture.extend(nal.bytes());
                }
                if !unit.units().iter().any(|n| matches!(n.nal_type, 1 | 5)) {
                    return Err(invalid());
                }
                Ok(picture)
            }
            VideoCodec::H265 => {
                let units = crate::hevc_units(&frame.payload).map_err(|_| invalid())?;
                if units
                    .iter()
                    .any(|n| (16..=21).contains(&((n[0] >> 1) & 63)))
                    != frame.keyframe
                {
                    return Err(invalid());
                }
                let mut picture = Vec::new();
                let mut has_picture = false;
                for nal in units {
                    let kind = (nal[0] >> 1) & 63;
                    if (32..=34).contains(&kind) {
                        if nal != self.sets[(kind - 32) as usize] {
                            return Err(RecordingError::FormatChanged);
                        }
                        continue;
                    }
                    if kind == 35 {
                        continue;
                    }
                    has_picture |= kind < 32;
                    picture.extend((nal.len() as u32).to_be_bytes());
                    picture.extend(nal);
                }
                if !has_picture {
                    return Err(invalid());
                }
                Ok(picture)
            }
            _ => {
                if frame.payload.is_empty() || frame.payload.len() > vnilink::MAX_VIDEO_FRAME {
                    return Err(invalid());
                }
                Ok(frame.payload.clone())
            }
        }
    }
}
pub(crate) fn picture_size(frame: &EncodedFrame) -> Result<(u16, u16), RecordingError> {
    if !frame.keyframe {
        return Err(invalid());
    }
    match frame.codec {
        VideoCodec::H264 => {
            let unit = crate::parse_annex_b(&frame.payload).map_err(|_| invalid())?;
            if !unit.has_idr() {
                return Err(invalid());
            }
            let sps = unit
                .units()
                .iter()
                .find(|n| n.nal_type == 7)
                .ok_or_else(invalid)?;
            crate::h264_picture_size(sps.bytes()).map_err(|_| invalid())
        }
        VideoCodec::H265 => {
            let units = crate::hevc_units(&frame.payload).map_err(|_| invalid())?;
            let sps = units
                .iter()
                .find(|n| (n[0] >> 1) & 63 == 33)
                .ok_or_else(invalid)?;
            hevc_size(sps)
        }
        VideoCodec::Vp8 | VideoCodec::Vp9 | VideoCodec::Av1 => {
            let picture = crate::SoftwareDecoder::open(frame.codec)
                .map_err(|_| invalid())?
                .decode(frame)
                .map_err(|_| invalid())?;
            Ok((picture.width(), picture.height()))
        }
    }
}
fn rbsp(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut zeros = 0;
    for b in bytes {
        if zeros == 2 && *b == 3 {
            zeros = 0;
            continue;
        }
        out.push(*b);
        zeros = if *b == 0 { zeros + 1 } else { 0 };
    }
    out
}
struct Bits<'a> {
    bytes: &'a [u8],
    offset: usize,
}
impl<'a> Bits<'a> {
    fn read(&mut self, n: usize) -> Result<u32, RecordingError> {
        if n > 32 || self.offset + n > self.bytes.len() * 8 {
            return Err(invalid());
        }
        let mut v = 0;
        for _ in 0..n {
            v = (v << 1) | ((self.bytes[self.offset / 8] >> (7 - self.offset % 8)) & 1) as u32;
            self.offset += 1;
        }
        Ok(v)
    }
    fn skip(&mut self, n: usize) -> Result<(), RecordingError> {
        if self.offset + n > self.bytes.len() * 8 {
            return Err(invalid());
        }
        self.offset += n;
        Ok(())
    }
    fn ue(&mut self) -> Result<u32, RecordingError> {
        let mut n = 0;
        while self.read(1)? == 0 {
            n += 1;
            if n > 30 {
                return Err(invalid());
            }
        }
        Ok((1 << n) - 1 + self.read(n)?)
    }
}
fn hevc_size(sps: &[u8]) -> Result<(u16, u16), RecordingError> {
    let data = rbsp(&sps[2..]);
    let mut bits = Bits {
        bytes: &data,
        offset: 0,
    };
    bits.skip(4)?;
    let layers = bits.read(3)? as usize;
    bits.skip(1 + 96)?;
    let mut flags = Vec::new();
    for _ in 0..layers {
        flags.push((bits.read(1)?, bits.read(1)?));
    }
    if layers > 0 {
        bits.skip(2 * (8 - layers))?;
    }
    for (p, l) in flags {
        if p != 0 {
            bits.skip(88)?;
        }
        if l != 0 {
            bits.skip(8)?;
        }
    }
    bits.ue()?;
    let chroma = bits.ue()?;
    if chroma == 3 {
        bits.skip(1)?;
    }
    let mut w = bits.ue()?;
    let mut h = bits.ue()?;
    if bits.read(1)? != 0 {
        let left = bits.ue()?;
        let right = bits.ue()?;
        let top = bits.ue()?;
        let bottom = bits.ue()?;
        w = w
            .checked_sub(
                left.checked_add(right)
                    .and_then(|sum| sum.checked_mul(if chroma == 1 || chroma == 2 { 2 } else { 1 }))
                    .ok_or_else(invalid)?,
            )
            .ok_or_else(invalid)?;
        h = h
            .checked_sub(
                top.checked_add(bottom)
                    .and_then(|sum| sum.checked_mul(if chroma == 1 { 2 } else { 1 }))
                    .ok_or_else(invalid)?,
            )
            .ok_or_else(invalid)?;
    }
    if chroma != 1 || bits.ue()? != 0 || bits.ue()? != 0 || w == 0 || h == 0 || w > 4096 || h > 4096
    {
        return Err(invalid());
    }
    Ok((w as u16, h as u16))
}
fn av1_sequence(data: &[u8]) -> Result<(&[u8], &[u8]), RecordingError> {
    let mut offset = 0;
    while offset < data.len() {
        let start = offset;
        let header = data[offset];
        offset += 1;
        if header & 0x80 != 0 || header & 1 != 0 {
            return Err(invalid());
        }
        if header & 4 != 0 {
            offset += 1;
        }
        if header & 2 == 0 {
            return Err(invalid());
        }
        let mut len = 0usize;
        let mut shift = 0;
        loop {
            let b = *data.get(offset).ok_or_else(invalid)?;
            offset += 1;
            len |= ((b & 127) as usize)
                .checked_shl(shift)
                .ok_or_else(invalid)?;
            if b & 128 == 0 {
                break;
            }
            shift += 7;
            if shift > 28 {
                return Err(invalid());
            }
        }
        let end = offset
            .checked_add(len)
            .filter(|end| *end <= data.len())
            .ok_or_else(invalid)?;
        if (header >> 3) & 15 == 1 {
            return Ok((&data[start..end], &data[offset..end]));
        }
        offset = end;
    }
    Err(invalid())
}
struct Av1Info {
    profile: u8,
    level: u8,
    tier: u8,
    high: u8,
    twelve: u8,
    mono: u8,
    x: u8,
    y: u8,
    position: u8,
}
fn av1_info(bytes: &[u8]) -> Result<Av1Info, RecordingError> {
    let mut b = Bits { bytes, offset: 0 };
    let profile = b.read(3)? as u8;
    b.skip(1)?;
    let reduced = b.read(1)? != 0;
    let mut level = 0;
    let mut tier = 0;
    if reduced {
        level = b.read(5)? as u8;
    } else {
        let timing = b.read(1)? != 0;
        let mut model = false;
        let mut delay = 0;
        if timing {
            b.skip(64)?;
            if b.read(1)? != 0 {
                b.ue()?;
            }
            model = b.read(1)? != 0;
            if model {
                delay = b.read(5)? as usize + 1;
                b.skip(32 + 5 + 5)?;
            }
        }
        let initial = b.read(1)? != 0;
        let count = b.read(5)? + 1;
        for i in 0..count {
            b.skip(12)?;
            let l = b.read(5)? as u8;
            let t = if l > 7 { b.read(1)? as u8 } else { 0 };
            if i == 0 {
                level = l;
                tier = t;
            }
            if model && b.read(1)? != 0 {
                b.skip(delay * 2 + 1)?;
            }
            if initial && b.read(1)? != 0 {
                b.skip(4)?;
            }
        }
    }
    let wb = b.read(4)? as usize + 1;
    let hb = b.read(4)? as usize + 1;
    b.skip(wb + hb)?;
    if !reduced && b.read(1)? != 0 {
        b.skip(4 + 3)?;
    }
    b.skip(3)?;
    if !reduced {
        b.skip(4)?;
        let order = b.read(1)? != 0;
        if order {
            b.skip(2)?;
        }
        let screen = if b.read(1)? != 0 { 2 } else { b.read(1)? };
        if screen > 0 && b.read(1)? == 0 {
            b.skip(1)?;
        }
        if order {
            b.skip(3)?;
        }
    }
    b.skip(3)?;
    let high = b.read(1)? as u8;
    let twelve = if profile == 2 && high != 0 {
        b.read(1)? as u8
    } else {
        0
    };
    let mono = if profile == 1 { 0 } else { b.read(1)? as u8 };
    let (primaries, transfer, matrix) = if b.read(1)? != 0 {
        (b.read(8)?, b.read(8)?, b.read(8)?)
    } else {
        (2, 2, 2)
    };
    let (x, y, position) = if mono != 0 {
        b.skip(1)?;
        (1, 1, 0)
    } else if primaries == 1 && transfer == 13 && matrix == 0 {
        (0, 0, 0)
    } else {
        b.skip(1)?;
        let (x, y) = match profile {
            0 => (1, 1),
            1 => (0, 0),
            2 if twelve != 0 => {
                let x = b.read(1)? as u8;
                let y = if x != 0 { b.read(1)? as u8 } else { 0 };
                (x, y)
            }
            2 => (1, 0),
            _ => return Err(invalid()),
        };
        let position = if x != 0 && y != 0 {
            b.read(2)? as u8
        } else {
            0
        };
        (x, y, position)
    };
    Ok(Av1Info {
        profile,
        level,
        tier,
        high,
        twelve,
        mono,
        x,
        y,
        position,
    })
}
