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

#[test]
fn clones_and_shared_frames_hold_the_same_pixels() {
    let frame = solid(64, 32, [1, 2, 3, 255]);
    let clone = frame.clone().with_timestamp(99);
    assert_eq!(clone.pixels().as_ptr(), frame.pixels().as_ptr());
    let shared = RawFrame::shared(64, 32, 64 * 4, 7, frame.shared_pixels()).unwrap();
    assert_eq!(shared.pixels().as_ptr(), frame.pixels().as_ptr());
    assert_eq!(shared.timestamp_us(), 7);
    // The same checks as a frame that owns its pixels.
    assert!(RawFrame::shared(64, 33, 64 * 4, 0, frame.shared_pixels()).is_err());
    assert!(RawFrame::shared(64, 32, 63 * 4, 0, frame.shared_pixels()).is_err());
}

/// The share of a picture rectangles cover, clipped to it and never above all of it.
#[test]
fn rectangles_give_the_share_of_the_picture_they_cover() {
    use crate::changed_share;
    assert_eq!(changed_share([[0, 0, 50, 100]], 100, 100), 0.5);
    // Clipped at the edges.
    assert_eq!(changed_share([[-50, 0, 100, 100]], 100, 100), 0.5);
    assert_eq!(
        changed_share([[0, 0, 100, 100], [0, 0, 100, 100]], 100, 100),
        1.0
    );
    assert_eq!(changed_share(std::iter::empty(), 100, 100), 0.0);
}

/// Tiles compare their pixels and not the rows' padding; a picture too
/// short for its rows cannot be compared.
#[test]
fn tiles_that_differ_give_the_share_of_the_picture_that_changed() {
    use crate::changed_tiles;
    // Two pictures of 128 by 64 pixels, two tiles, one padded by 16 bytes a row.
    let left = vec![9u8; 128 * 4 * 64];
    let mut right = vec![0u8; (128 * 4 + 16) * 64];
    for y in 0..64 {
        right[y * (128 * 4 + 16)..][..128 * 4].fill(9);
    }
    assert_eq!(
        changed_tiles((&left, 512), (&right, 528), 128, 64),
        Some(0.0)
    );
    right[10 * 528 + 70 * 4] = 1;
    assert_eq!(
        changed_tiles((&left, 512), (&right, 528), 128, 64),
        Some(0.5)
    );
    assert_eq!(
        changed_tiles((&left, 512), (&right[..1000], 528), 128, 64),
        None
    );
}
