use crate::error::Error;
use crate::frame::{encoded_size, scale_bgra, RawFrame};

fn solid(width: u16, height: u16, pixel: [u8; 4]) -> RawFrame {
    let mut bgra = Vec::with_capacity(usize::from(width) * usize::from(height) * 4);
    for _ in 0..usize::from(width) * usize::from(height) {
        bgra.extend_from_slice(&pixel);
    }
    RawFrame::bgra(width, height, 10, bgra).unwrap()
}

#[test]
fn raw_frames_reject_empty_and_short_buffers() {
    assert!(matches!(
        RawFrame::bgra(0, 2, 0, vec![]),
        Err(Error::InvalidFrame(
            "frame width and height must be non-zero"
        ))
    ));
    assert!(matches!(
        RawFrame::bgra(2, 2, 0, vec![0; 8]),
        Err(Error::InvalidFrame("frame is missing pixels"))
    ));
    assert!(matches!(
        RawFrame::with_stride(2, 1, 4, 0, vec![0; 8]),
        Err(Error::InvalidFrame("frame stride is shorter than one row"))
    ));
    let frame = solid(2, 1, [1, 2, 3, 4]);
    let rendered = format!("{frame:?}");
    assert!(rendered.contains("width"));
    assert!(!rendered.contains("[1, 2, 3, 4]"));
}

#[test]
fn encoded_size_keeps_aspect_and_stays_even() {
    assert_eq!(encoded_size(320, 180, 160, 120).unwrap(), (160, 90));
    assert_eq!(encoded_size(1920, 1080, 1280, 720).unwrap(), (1280, 720));
    assert_eq!(encoded_size(100, 100, 1919, 1080).unwrap(), (100, 100));
    assert_eq!(encoded_size(101, 51, 200, 200).unwrap(), (100, 50));
    assert_eq!(encoded_size(3, 3, 3, 3).unwrap(), (2, 2));
    assert!(matches!(
        encoded_size(1, 1, 1, 1),
        Err(Error::InvalidSetting(
            "encoded size must be at least 2 by 2"
        ))
    ));
    assert!(matches!(
        encoded_size(0, 10, 10, 10),
        Err(Error::InvalidSetting(
            "encoded size must be at least 2 by 2"
        ))
    ));
}

#[test]
fn scale_uses_nearest_neighbor_and_ignores_row_padding() {
    let mut bgra = vec![0u8; 2 * 12];
    bgra[0..4].copy_from_slice(&[1, 0, 0, 255]);
    bgra[4..8].copy_from_slice(&[2, 0, 0, 255]);
    bgra[8..12].copy_from_slice(&[9, 9, 9, 9]);
    bgra[12..16].copy_from_slice(&[3, 0, 0, 255]);
    bgra[16..20].copy_from_slice(&[4, 0, 0, 255]);
    let frame = RawFrame::with_stride(2, 2, 12, 7, bgra).unwrap();
    let scaled = scale_bgra(&frame, 1, 1).unwrap();
    assert_eq!(scaled.timestamp_us(), 7);
    assert_eq!(scaled.pixels(), &[1, 0, 0, 255]);
    assert_eq!(
        scale_bgra(&frame, 2, 2).unwrap().pixels()[0..8],
        frame.row(0)[0..8]
    );
    assert!(matches!(
        scale_bgra(&frame, 0, 2),
        Err(Error::InvalidFrame(
            "frame width and height must be non-zero"
        ))
    ));
}
