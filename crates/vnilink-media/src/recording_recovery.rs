// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! Bounded structural scan of our append-only recordings. No payload decoding
//! or whole-file read: damaged tails are discarded only by an explicit caller.
use crate::RecordingError;
use std::io::{Read, Seek, SeekFrom};
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RecordingRecovery {
    pub complete_bytes: u64,
    pub clusters: u64,
    pub last_timestamp_ms: u64,
    pub duration_position: u64,
}
/// Scan a locally selected VniLink partial Matroska recording. This reports a
/// recoverable prefix; it never labels interrupted output as a finished file.
pub fn scan_recording_prefix<R: Read + Seek>(
    reader: &mut R,
) -> Result<RecordingRecovery, RecordingError> {
    let length = reader.seek(SeekFrom::End(0))?;
    reader.seek(SeekFrom::Start(0))?;
    let (id, size) = header(reader)?.ok_or(RecordingError::InvalidMedia)?;
    if id != 0x1a45dfa3 || size > 4096 {
        return Err(RecordingError::InvalidMedia);
    }
    let mut ebml = vec![0; size as usize];
    reader.read_exact(&mut ebml)?;
    if !ebml.windows(8).any(|v| v == b"matroska") {
        return Err(RecordingError::InvalidMedia);
    }
    let (id, size) = header(reader)?.ok_or(RecordingError::InvalidMedia)?;
    if id != 0x18538067 || size != u64::MAX {
        return Err(RecordingError::InvalidMedia);
    }
    let mut info = false;
    let mut app = false;
    let mut tracks = false;
    let mut result = RecordingRecovery {
        complete_bytes: 0,
        clusters: 0,
        last_timestamp_ms: 0,
        duration_position: 0,
    };
    for _ in 0..5_500_000 {
        let at = reader.stream_position()?;
        if at == length {
            break;
        }
        let Some((id, size)) = header(reader)? else {
            break;
        };
        let data = reader.stream_position()?;
        let Some(end) = data.checked_add(size).filter(|v| *v <= length) else {
            break;
        };
        match id {
            0x1549a966 if !info && !tracks => {
                if size > 4096 {
                    return Err(RecordingError::InvalidMedia);
                }
                let mut bytes = vec![0; size as usize];
                reader.read_exact(&mut bytes)?;
                let mut children = std::io::Cursor::new(&bytes);
                while children.position() < size {
                    let (child, len) =
                        header(&mut children)?.ok_or(RecordingError::InvalidMedia)?;
                    let offset = children.position();
                    let next = offset
                        .checked_add(len)
                        .filter(|v| *v <= size)
                        .ok_or(RecordingError::InvalidMedia)?;
                    if child == 0x4489 && len == 8 {
                        result.duration_position = data + offset;
                    }
                    if child == 0x5741 {
                        if len != 7 || &bytes[offset as usize..next as usize] != b"VniLink" {
                            return Err(RecordingError::InvalidMedia);
                        }
                        app = true;
                    }
                    children.set_position(next);
                }
                if result.duration_position == 0 || !app {
                    return Err(RecordingError::InvalidMedia);
                }
                info = true;
            }
            0x1654ae6b if info && !tracks => {
                if size > 131_200 {
                    return Err(RecordingError::InvalidMedia);
                }
                let mut bytes = vec![0; size as usize];
                reader.read_exact(&mut bytes)?;
                if !bytes
                    .windows(b"V_MPEG4/ISO/AVC".len())
                    .any(|b| b == b"V_MPEG4/ISO/AVC")
                {
                    return Err(RecordingError::InvalidMedia);
                }
                tracks = true;
            }
            0x1f43b675 if info && tracks => {
                if size > vnilink::MAX_VIDEO_FRAME as u64 + 4096 {
                    return Err(RecordingError::InvalidMedia);
                }
                let (child, len) = header(reader)?.ok_or(RecordingError::InvalidMedia)?;
                if child != 0xe7 || len == 0 || len > 8 {
                    return Err(RecordingError::InvalidMedia);
                }
                let mut bytes = [0; 8];
                reader.read_exact(&mut bytes[8 - len as usize..])?;
                let time = u64::from_be_bytes(bytes);
                if time < result.last_timestamp_ms || time > 86_400_000 {
                    return Err(RecordingError::InvalidMedia);
                }
                let (child, len) = header(reader)?.ok_or(RecordingError::InvalidMedia)?;
                if child != 0xa3
                    || len < 5
                    || reader.stream_position()?.checked_add(len) != Some(end)
                {
                    return Err(RecordingError::InvalidMedia);
                }
                let mut block = [0; 4];
                reader.read_exact(&mut block)?;
                if block[0] != 0x81
                    || block[1] != 0
                    || block[2] != 0
                    || !matches!(block[3], 0 | 0x80)
                    || result.clusters == 0 && block[3] != 0x80
                {
                    return Err(RecordingError::InvalidMedia);
                }
                result.last_timestamp_ms = time;
                result.clusters += 1;
                result.complete_bytes = end;
            }
            // Cues are discarded on explicit recovery; do not trust an
            // interrupted finalization's duration/cue update as finished output.
            0x1c53bb6b if result.clusters > 0 => break,
            _ => return Err(RecordingError::InvalidMedia),
        }
        reader.seek(SeekFrom::Start(end))?;
    }
    if result.clusters == 0 {
        return Err(RecordingError::InvalidMedia);
    }
    Ok(result)
}
fn header<R: Read>(reader: &mut R) -> Result<Option<(u32, u64)>, RecordingError> {
    fn first<R: Read>(reader: &mut R) -> Result<Option<u8>, RecordingError> {
        let mut byte = [0; 1];
        match reader.read(&mut byte) {
            Ok(0) => Ok(None),
            Ok(_) => Ok(Some(byte[0])),
            Err(e) => Err(e.into()),
        }
    }
    let Some(byte) = first(reader)? else {
        return Ok(None);
    };
    let width = byte.leading_zeros() as usize + 1;
    if width > 4 {
        return Err(RecordingError::InvalidMedia);
    }
    let mut id = u32::from(byte);
    for _ in 1..width {
        let Some(b) = first(reader)? else {
            return Ok(None);
        };
        id = (id << 8) | u32::from(b);
    }
    let Some(byte) = first(reader)? else {
        return Ok(None);
    };
    let width = byte.leading_zeros() as usize + 1;
    if width > 8 {
        return Err(RecordingError::InvalidMedia);
    }
    let mut size = u64::from(byte & (0xffu16 >> width) as u8);
    for _ in 1..width {
        let Some(b) = first(reader)? else {
            return Ok(None);
        };
        size = (size << 8) | u64::from(b);
    }
    if size == (1u64 << (7 * width)) - 1 {
        size = u64::MAX;
    }
    Ok(Some((id, size)))
}
