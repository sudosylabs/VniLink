//! The cursor for viewers that draw it themselves, in the picture's pixels,
//! and compatibility rendering for viewers that cannot.
use std::sync::Arc;

use crate::{Error, RawFrame};
use vnilink::{CursorImage, CursorState};

/// `cursor`, read in capture pixels, in the pixels of a picture `capture`
/// scaled to `picture`: its position, and its image averaged down (or
/// repeated up) so it keeps its size relative to the screen.
pub(crate) fn for_picture(
    cursor: &CursorState,
    capture: (u16, u16),
    picture: (u16, u16),
) -> Result<CursorState, Error> {
    if capture == picture {
        return Ok(cursor.clone());
    }
    let sx = f64::from(picture.0) / f64::from(capture.0.max(1));
    let sy = f64::from(picture.1) / f64::from(capture.1.max(1));
    let image = cursor
        .image
        .as_ref()
        .map(|image| scale_image(image, sx, sy).map(Arc::new))
        .transpose()?;
    Ok(CursorState {
        position: (
            (f64::from(cursor.position.0) * sx).round() as i32,
            (f64::from(cursor.position.1) * sy).round() as i32,
        ),
        visible: cursor.visible,
        image,
    })
}

/// Each target pixel is the average of the premultiplied source pixels it
/// covers, so a shrunk cursor keeps smooth edges instead of dropping rows.
fn scale_image(image: &CursorImage, sx: f64, sy: f64) -> Result<CursorImage, Error> {
    let (width, height) = (usize::from(image.width()), usize::from(image.height()));
    let edge = f64::from(vnilink::MAX_CURSOR_EDGE);
    let w = (width as f64 * sx).round().clamp(1.0, edge) as usize;
    let h = (height as f64 * sy).round().clamp(1.0, edge) as usize;
    let source = image.pixels();
    let mut pixels = vec![0u8; w * h * 4];
    for y in 0..h {
        let top = y * height / h;
        let bottom = ((y + 1) * height).div_ceil(h).max(top + 1);
        for x in 0..w {
            let left = x * width / w;
            let right = ((x + 1) * width).div_ceil(w).max(left + 1);
            let mut sum = [0u32; 4];
            for row in top..bottom {
                for pixel in
                    source[(row * width + left) * 4..(row * width + right) * 4].chunks_exact(4)
                {
                    for (total, value) in sum.iter_mut().zip(pixel) {
                        *total += u32::from(*value);
                    }
                }
            }
            let count = ((bottom - top) * (right - left)) as u32;
            for (channel, total) in pixels[(y * w + x) * 4..][..4].iter_mut().zip(sum) {
                *channel = ((total + count / 2) / count) as u8;
            }
        }
    }
    let hotspot = |value: u16, scale: f64, size: usize| {
        (f64::from(value) * scale)
            .round()
            .clamp(0.0, (size - 1) as f64) as u16
    };
    CursorImage::new(
        w as u16,
        h as u16,
        (
            hotspot(image.hotspot().0, sx, w),
            hotspot(image.hotspot().1, sy, h),
        ),
        pixels,
    )
    .map_err(|_| Error::InvalidFrame("cursor image was rejected"))
}

pub(crate) fn composite(frame: &RawFrame, cursor: &CursorState) -> Result<RawFrame, Error> {
    let Some(image) = cursor.image.as_ref().filter(|_| cursor.visible) else {
        return Ok(frame.clone());
    };
    let mut pixels = frame.pixels().to_vec();
    let left = i64::from(cursor.position.0) - i64::from(image.hotspot().0);
    let top = i64::from(cursor.position.1) - i64::from(image.hotspot().1);
    for y in 0..usize::from(image.height()) {
        let screen_y = top + y as i64;
        if screen_y < 0 || screen_y >= i64::from(frame.height()) {
            continue;
        }
        for x in 0..usize::from(image.width()) {
            let screen_x = left + x as i64;
            if screen_x < 0 || screen_x >= i64::from(frame.width()) {
                continue;
            }
            let source = (y * usize::from(image.width()) + x) * 4;
            let target = screen_y as usize * frame.stride() + screen_x as usize * 4;
            let alpha = u16::from(image.pixels()[source + 3]);
            for channel in 0..3 {
                pixels[target + channel] = (u16::from(image.pixels()[source + channel])
                    + (u16::from(pixels[target + channel]) * (255 - alpha) + 127) / 255)
                    .min(255) as u8;
            }
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
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn a_separate_cursor_is_placed_and_averaged_in_the_pictures_pixels() {
        // A 2x2 cursor read on a 4K screen, sent in a 1080p picture.
        let image = Arc::new(
            CursorImage::new(
                2,
                2,
                (1, 1),
                vec![
                    0, 0, 0, 0, 200, 200, 200, 255, 100, 100, 100, 255, 100, 100, 100, 255,
                ],
            )
            .unwrap(),
        );
        let cursor = CursorState {
            position: (1001, 600),
            visible: true,
            image: Some(image),
        };
        let shown = for_picture(&cursor, (3840, 2160), (1920, 1080)).unwrap();
        assert_eq!(shown.position, (501, 300));
        let image = shown.image.unwrap();
        assert_eq!(
            (image.width(), image.height(), image.hotspot()),
            (1, 1, (0, 0))
        );
        assert_eq!(image.pixels(), &[100, 100, 100, 191]);
        let same = for_picture(&cursor, (1920, 1080), (1920, 1080)).unwrap();
        assert_eq!(same, cursor);
    }
    #[test]
    fn legacy_viewers_get_one_composited_cursor_with_clipped_hotspot_and_alpha() {
        let frame = RawFrame::bgra(2, 2, 42, vec![100; 16]).unwrap();
        let image = Arc::new(
            vnilink::CursorImage::new(
                2,
                2,
                (1, 1),
                vec![
                    20, 30, 40, 255, 20, 30, 40, 255, 20, 30, 40, 255, 25, 25, 25, 128,
                ],
            )
            .unwrap(),
        );
        let cursor = CursorState {
            position: (0, 0),
            visible: true,
            image: Some(image),
        };
        let output = composite(&frame, &cursor).unwrap();
        assert_eq!(&output.pixels()[..3], &[75, 75, 75]);
        assert_eq!(&output.pixels()[4..], &frame.pixels()[4..]);
        assert_eq!(output.timestamp_us(), 42);
        let mut hidden = cursor;
        hidden.visible = false;
        assert_eq!(composite(&frame, &hidden).unwrap().pixels(), frame.pixels());
    }
}
