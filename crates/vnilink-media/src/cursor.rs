//! Compatibility rendering for viewers that cannot render a separate cursor.
use crate::{Error, RawFrame};
use vnilink::CursorState;

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
    use std::sync::Arc;
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
