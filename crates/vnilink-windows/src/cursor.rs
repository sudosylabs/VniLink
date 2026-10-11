//! Bounded DXGI pointer shapes composited against original desktop pixels.
//! XOR cursors cannot be represented as one ordinary transparent RGBA image.
use vnilink_media::{Error, RawFrame};
const EDGE: usize = 512;
pub(crate) const MAX_SHAPE_BYTES: usize = EDGE * EDGE * 4;
#[derive(Clone, Copy)]
pub(crate) enum Kind {
    Monochrome,
    Color,
    MaskedColor,
}
pub(crate) struct Shape {
    kind: Kind,
    width: usize,
    height: usize,
    pitch: usize,
    bytes: Vec<u8>,
}
impl Shape {
    pub(crate) fn new(
        kind: Kind,
        width: u32,
        height: u32,
        pitch: u32,
        bytes: Vec<u8>,
    ) -> Result<Self, Error> {
        let width = width as usize;
        let stored_height = height as usize;
        let height = match kind {
            Kind::Monochrome if stored_height.is_multiple_of(2) => stored_height / 2,
            Kind::Monochrome => {
                return Err(Error::InvalidFrame("invalid monochrome cursor height"))
            }
            _ => stored_height,
        };
        let row = match kind {
            Kind::Monochrome => width.div_ceil(8),
            _ => width.saturating_mul(4),
        };
        let pitch = pitch as usize;
        if width == 0
            || height == 0
            || width > EDGE
            || height > EDGE
            || pitch < row
            || bytes.len() > MAX_SHAPE_BYTES
            || pitch.checked_mul(stored_height) != Some(bytes.len())
        {
            return Err(Error::InvalidFrame("invalid DXGI cursor dimensions"));
        }
        if matches!(kind, Kind::MaskedColor)
            && bytes.chunks(pitch).any(|row| {
                row[..width * 4]
                    .chunks_exact(4)
                    .any(|p| p[3] != 0 && p[3] != 255)
            })
        {
            return Err(Error::InvalidFrame("invalid masked cursor alpha"));
        }
        Ok(Self {
            kind,
            width,
            height,
            pitch,
            bytes,
        })
    }
    pub(crate) fn composite(
        &self,
        frame: &RawFrame,
        position: (i32, i32),
    ) -> Result<RawFrame, Error> {
        let left = i64::from(position.0);
        let top = i64::from(position.1);
        let x0 = left.max(0);
        let y0 = top.max(0);
        let x1 = (left + self.width as i64).min(i64::from(frame.width()));
        let y1 = (top + self.height as i64).min(i64::from(frame.height()));
        if x0 >= x1 || y0 >= y1 {
            return Ok(frame.clone());
        }
        let mut pixels = frame.pixels().to_vec();
        for y in y0..y1 {
            for x in x0..x1 {
                let sx = (x - left) as usize;
                let sy = (y - top) as usize;
                let offset = y as usize * frame.stride() + x as usize * 4;
                let target = &mut pixels[offset..offset + 4];
                match self.kind {
                    Kind::Monochrome => {
                        let bit = 0x80 >> (sx % 8);
                        let and = if self.bytes[sy * self.pitch + sx / 8] & bit != 0 {
                            255
                        } else {
                            0
                        };
                        let xor = if self.bytes[(sy + self.height) * self.pitch + sx / 8] & bit != 0
                        {
                            255
                        } else {
                            0
                        };
                        for channel in &mut target[..3] {
                            *channel = (*channel & and) ^ xor;
                        }
                    }
                    Kind::MaskedColor => {
                        let source = &self.bytes[sy * self.pitch + sx * 4..][..4];
                        for i in 0..3 {
                            target[i] = if source[3] == 255 {
                                target[i] ^ source[i]
                            } else {
                                source[i]
                            };
                        }
                    }
                    Kind::Color => {
                        let source = &self.bytes[sy * self.pitch + sx * 4..][..4];
                        let alpha = u32::from(source[3]);
                        for i in 0..3 {
                            target[i] = ((u32::from(source[i]) * alpha
                                + u32::from(target[i]) * (255 - alpha)
                                + 127)
                                / 255) as u8;
                        }
                    }
                }
                target[3] = 255;
            }
        }
        RawFrame::with_stride(
            frame.width(),
            frame.height(),
            frame.stride(),
            frame.timestamp_us(),
            pixels,
        )
    }
}
#[derive(Default)]
pub(crate) struct Pointer {
    pub(crate) visible: bool,
    pub(crate) position: (i32, i32),
    pub(crate) shape: Option<Shape>,
}
impl Pointer {
    pub(crate) fn composite(&self, frame: RawFrame) -> Result<RawFrame, Error> {
        match self.shape.as_ref().filter(|_| self.visible) {
            Some(shape) => shape.composite(&frame, self.position),
            None => Ok(frame),
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn frame() -> RawFrame {
        RawFrame::bgra(4, 1, 9, [10, 20, 30, 255].repeat(4)).unwrap()
    }
    #[test]
    fn monochrome_and_xor_cover_black_white_transparent_and_inversion() {
        // AND: 0011, XOR: 0101 -> black, white, unchanged, inverse.
        let shape = Shape::new(Kind::Monochrome, 4, 2, 1, vec![0x30, 0x50]).unwrap();
        let base = frame();
        let rendered = shape.composite(&base, (0, 0)).unwrap();
        assert_eq!(
            rendered.pixels(),
            &[0, 0, 0, 255, 255, 255, 255, 255, 10, 20, 30, 255, 245, 235, 225, 255]
        );
        assert_eq!(base.pixels(), &[10, 20, 30, 255].repeat(4));
        assert_eq!(rendered.timestamp_us(), 9);
    }
    #[test]
    fn masked_color_copies_or_inverts_and_color_blends_with_clipping() {
        let masked = Shape::new(
            Kind::MaskedColor,
            2,
            1,
            8,
            vec![100, 110, 120, 0, 255, 15, 0, 255],
        )
        .unwrap();
        assert_eq!(
            &masked.composite(&frame(), (0, 0)).unwrap().pixels()[..8],
            &[100, 110, 120, 255, 245, 27, 30, 255]
        );
        let color = Shape::new(
            Kind::Color,
            2,
            1,
            12,
            vec![9, 8, 7, 0, 110, 120, 130, 128, 77, 77, 77, 77],
        )
        .unwrap();
        let rendered = color.composite(&frame(), (-1, 0)).unwrap();
        assert_eq!(&rendered.pixels()[..4], &[60, 70, 80, 255]);
        assert_eq!(&rendered.pixels()[4..], &frame().pixels()[4..]);
        assert_eq!(
            color
                .composite(&frame(), (i32::MAX, i32::MIN))
                .unwrap()
                .pixels(),
            frame().pixels()
        );
    }
    #[test]
    fn moving_and_hiding_cursor_never_burns_trails_into_cached_desktop() {
        let base = frame();
        let mut pointer = Pointer {
            visible: true,
            position: (0, 0),
            shape: Some(Shape::new(Kind::Color, 1, 1, 4, vec![255; 4]).unwrap()),
        };
        let first = pointer.composite(base.clone()).unwrap();
        assert_eq!(&first.pixels()[..4], &[255; 4]);
        pointer.position = (1, 0);
        let second = pointer.composite(base.clone()).unwrap();
        assert_eq!(&second.pixels()[..4], &base.pixels()[..4]);
        assert_eq!(&second.pixels()[4..8], &[255; 4]);
        pointer.visible = false;
        assert_eq!(
            pointer.composite(base.clone()).unwrap().pixels(),
            base.pixels()
        );
    }
    #[test]
    fn shapes_bound_allocation_and_mask_pitch_and_hidden_cursor_keeps_original_pixels() {
        assert!(Shape::new(Kind::Monochrome, 1, 3, 1, vec![0; 3]).is_err());
        assert!(Shape::new(Kind::Color, 513, 1, 2052, vec![0; 2052]).is_err());
        assert!(Shape::new(Kind::Color, 1, 1, 3, vec![0; 3]).is_err());
        assert!(Shape::new(Kind::MaskedColor, 1, 1, 4, vec![0, 0, 0, 128]).is_err());
        let pointer = Pointer {
            visible: false,
            position: (0, 0),
            shape: Some(Shape::new(Kind::Color, 1, 1, 4, vec![255; 4]).unwrap()),
        };
        assert_eq!(
            pointer.composite(frame()).unwrap().pixels(),
            frame().pixels()
        );
    }
}
