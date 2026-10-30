// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! Bounded parsing of SPA cursor metadata. Offsets are checked before reading.
use std::sync::Arc;
use vnilink::{CursorImage, CursorState, MAX_CURSOR_EDGE};

pub(crate) const META_SIZE: usize = 28 + 20 + 512 * 512 * 4;

pub(crate) fn parse(
    bytes: &[u8],
    previous: Option<&CursorState>,
    bgra: u32,
    rgba: u32,
) -> Option<CursorState> {
    let word = |offset: usize| -> Option<u32> {
        Some(u32::from_ne_bytes(
            bytes.get(offset..offset + 4)?.try_into().ok()?,
        ))
    };
    if word(0)? == 0 {
        return None;
    }
    let position = (word(8)? as i32, word(12)? as i32);
    let hotspot = (word(16)? as i32, word(20)? as i32);
    let offset = word(24)? as usize;
    let mut image = previous.and_then(|state| state.image.clone());
    let mut visible = position.0 >= 0 && position.1 >= 0;
    if offset != 0 {
        if offset < 28 || offset.checked_add(20)? > bytes.len() {
            return None;
        }
        let format = word(offset)?;
        if format != 0 {
            if format != bgra && format != rgba {
                return None;
            }
            if word(offset + 16)? == 0 {
                return Some(CursorState {
                    position,
                    visible: false,
                    image: None,
                });
            }
            let width = u16::try_from(word(offset + 4)?).ok()?;
            let height = u16::try_from(word(offset + 8)?).ok()?;
            if width == 0 || height == 0 || width > MAX_CURSOR_EDGE || height > MAX_CURSOR_EDGE {
                return None;
            }
            let stride = word(offset + 12)? as i32;
            let pitch = stride.unsigned_abs() as usize;
            let data_offset = word(offset + 16)? as usize;
            if data_offset == 0 {
                visible = false;
            } else {
                if data_offset < 20 || pitch < usize::from(width) * 4 {
                    return None;
                }
                let start = offset.checked_add(data_offset)?;
                let end = start.checked_add(pitch.checked_mul(usize::from(height))?)?;
                let data = bytes.get(start..end)?;
                let mut pixels = Vec::with_capacity(usize::from(width) * usize::from(height) * 4);
                for y in 0..usize::from(height) {
                    let y = if stride < 0 {
                        usize::from(height) - 1 - y
                    } else {
                        y
                    };
                    pixels.extend_from_slice(&data[y * pitch..y * pitch + usize::from(width) * 4]);
                }
                if format == rgba {
                    for pixel in pixels.chunks_exact_mut(4) {
                        pixel.swap(0, 2);
                    }
                }
                let new_image = CursorImage::new(
                    width,
                    height,
                    (
                        u16::try_from(hotspot.0).ok()?,
                        u16::try_from(hotspot.1).ok()?,
                    ),
                    pixels,
                )
                .ok()?;
                if image.as_deref() != Some(&new_image) {
                    image = Some(Arc::new(new_image));
                }
            }
        }
    }
    Some(CursorState {
        position,
        visible: visible && image.is_some(),
        image,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> Vec<u8> {
        let mut bytes = vec![0; 64];
        for (offset, value) in [
            (0, 1u32),
            (8, 100),
            (12, 200),
            (16, 1),
            (20, 0),
            (24, 28),
            (28, 11),
            (32, 2),
            (36, 2),
            (40, 8),
            (44, 20),
        ] {
            bytes[offset..offset + 4].copy_from_slice(&value.to_ne_bytes());
        }
        bytes[48..64].copy_from_slice(&[1, 2, 3, 255, 4, 5, 6, 255, 7, 8, 9, 255, 10, 11, 12, 255]);
        bytes
    }
    #[test]
    fn cursor_metadata_preserves_shape_across_motion_and_hiding() {
        let original = parse(&fixture(), None, 11, 12).unwrap();
        let mut hidden_bitmap = fixture();
        hidden_bitmap[32..48].fill(0);
        let hidden = parse(&hidden_bitmap, Some(&original), 11, 12).unwrap();
        let mut hidden_motion = fixture();
        hidden_motion[24..28].fill(0);
        assert!(
            !parse(&hidden_motion, Some(&hidden), 11, 12)
                .unwrap()
                .visible
        );
        assert!(parse(&fixture(), Some(&hidden), 11, 12).unwrap().visible);
        assert!(
            !parse(&hidden_bitmap, Some(&original), 11, 12)
                .unwrap()
                .visible
        );
        assert_eq!(original.position, (100, 200));
        assert!(original.visible);
        assert_eq!(original.image.as_ref().unwrap().hotspot(), (1, 0));
        let mut motion = fixture();
        motion[24..28].fill(0);
        motion[8..12].copy_from_slice(&101u32.to_ne_bytes());
        let moved = parse(&motion, Some(&original), 11, 12).unwrap();
        assert_eq!(moved.position, (101, 200));
        assert!(Arc::ptr_eq(
            moved.image.as_ref().unwrap(),
            original.image.as_ref().unwrap()
        ));
        motion[8..12].copy_from_slice(&(-1i32).to_ne_bytes());
        assert!(!parse(&motion, Some(&moved), 11, 12).unwrap().visible);
        let mut rgba = fixture();
        rgba[28..32].copy_from_slice(&12u32.to_ne_bytes());
        assert_eq!(
            &parse(&rgba, None, 11, 12).unwrap().image.unwrap().pixels()[..4],
            &[3, 2, 1, 255]
        );
    }
    #[test]
    fn bad_metadata_offsets_lengths_and_hotspots_never_escape_the_buffer() {
        let bytes = fixture();
        for len in 0..bytes.len() {
            assert!(parse(&bytes[..len], None, 11, 12).is_none());
        }
        for (offset, value) in [
            (24, u32::MAX),
            (44, u32::MAX),
            (40, u32::MAX),
            (32, 513),
            (16, 2),
        ] {
            let mut bytes = fixture();
            bytes[offset..offset + 4].copy_from_slice(&value.to_ne_bytes());
            assert!(parse(&bytes, None, 11, 12).is_none());
        }
    }
}
