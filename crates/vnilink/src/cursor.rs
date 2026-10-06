//! Bounded, lossless cursor shapes, independent of the video codec.
use crate::Error;
use std::sync::Arc;

pub const MAX_CURSOR_EDGE: u16 = 512;
pub(crate) const MAGIC: [u8; 4] = *b"CUR3";
pub(crate) const MAX_BODY: usize = 18 + 512 * 512 * 4;

/// Premultiplied BGRA pixels and the point in the image under the mouse.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CursorImage {
    width: u16,
    height: u16,
    hotspot: (u16, u16),
    pixels: Vec<u8>,
}
impl CursorImage {
    pub fn new(
        width: u16,
        height: u16,
        hotspot: (u16, u16),
        pixels: Vec<u8>,
    ) -> Result<Self, Error> {
        if width == 0
            || height == 0
            || width > MAX_CURSOR_EDGE
            || height > MAX_CURSOR_EDGE
            || hotspot.0 >= width
            || hotspot.1 >= height
            || pixels.len() != usize::from(width) * usize::from(height) * 4
        {
            return Err(Error::Malformed);
        }
        Ok(Self {
            width,
            height,
            hotspot,
            pixels,
        })
    }
    pub fn width(&self) -> u16 {
        self.width
    }
    pub fn height(&self) -> u16 {
        self.height
    }
    pub fn hotspot(&self) -> (u16, u16) {
        self.hotspot
    }
    pub fn pixels(&self) -> &[u8] {
        &self.pixels
    }
}

/// Position in unscaled capture pixels. A hidden cursor retains its last shape.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CursorState {
    pub position: (i32, i32),
    pub visible: bool,
    pub image: Option<Arc<CursorImage>>,
}

pub(crate) fn encode(state: &CursorState, include_image: bool) -> Vec<u8> {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&state.position.0.to_be_bytes());
    bytes.extend_from_slice(&state.position.1.to_be_bytes());
    bytes.push(u8::from(state.visible));
    if let Some(image) = state.image.as_ref().filter(|_| include_image) {
        bytes.push(1);
        bytes.extend_from_slice(&image.width.to_be_bytes());
        bytes.extend_from_slice(&image.height.to_be_bytes());
        bytes.extend_from_slice(&image.hotspot.0.to_be_bytes());
        bytes.extend_from_slice(&image.hotspot.1.to_be_bytes());
        bytes.extend_from_slice(image.pixels());
    } else {
        bytes.push(0);
    }
    bytes
}
pub(crate) fn decode(bytes: &[u8]) -> Result<CursorState, Error> {
    if bytes.len() < 10 || bytes.len() > MAX_BODY || bytes[8] > 1 {
        return Err(Error::Malformed);
    }
    let image = match bytes[9] {
        0 if bytes.len() == 10 => None,
        1 if bytes.len() >= 18 => {
            let short = |offset| u16::from_be_bytes([bytes[offset], bytes[offset + 1]]);
            Some(Arc::new(CursorImage::new(
                short(10),
                short(12),
                (short(14), short(16)),
                bytes[18..].to_vec(),
            )?))
        }
        _ => return Err(Error::Malformed),
    };
    Ok(CursorState {
        position: (
            i32::from_be_bytes(bytes[..4].try_into().unwrap()),
            i32::from_be_bytes(bytes[4..8].try_into().unwrap()),
        ),
        visible: bytes[8] == 1,
        image,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cursor_shapes_and_position_only_updates_round_trip_without_video() {
        let state = CursorState {
            position: (-1, 1080),
            visible: true,
            image: Some(Arc::new(
                CursorImage::new(2, 2, (1, 1), vec![64; 16]).unwrap(),
            )),
        };
        assert_eq!(decode(&encode(&state, true)).unwrap(), state);
        let position_only = decode(&encode(&state, false)).unwrap();
        assert_eq!(position_only.position, state.position);
        assert!(position_only.visible);
        assert!(position_only.image.is_none());
        let mut hidden = state;
        hidden.visible = false;
        assert!(!decode(&encode(&hidden, false)).unwrap().visible);
    }
    #[test]
    fn invalid_cursor_dimensions_hotspots_and_payloads_are_refused() {
        assert!(CursorImage::new(0, 1, (0, 0), vec![]).is_err());
        assert!(CursorImage::new(513, 1, (0, 0), vec![0; 513 * 4]).is_err());
        assert!(CursorImage::new(2, 2, (2, 0), vec![0; 16]).is_err());
        assert!(CursorImage::new(2, 2, (0, 0), vec![0; 15]).is_err());
        assert!(decode(&[0; 9]).is_err());
        let mut bytes = vec![0; 10];
        bytes[8] = 2;
        assert!(decode(&bytes).is_err());
        bytes[8] = 0;
        bytes.push(0);
        assert!(decode(&bytes).is_err());
        let image = CursorImage::new(512, 512, (511, 511), vec![0; 512 * 512 * 4]).unwrap();
        let encoded = encode(
            &CursorState {
                position: (0, 0),
                visible: true,
                image: Some(Arc::new(image)),
            },
            true,
        );
        assert!(decode(&encoded).is_ok());
        for len in [0, 9, 17, encoded.len() - 1] {
            assert!(decode(&encoded[..len]).is_err());
        }
    }
}
