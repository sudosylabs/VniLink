use crate::error::Error;
use crate::frame::RawFrame;
use crate::nv12::{bgra_to_nv12, nv12_to_bgra};

fn solid(width: u16, height: u16, pixel: [u8; 4]) -> RawFrame {
    let mut bgra = Vec::with_capacity(usize::from(width) * usize::from(height) * 4);
    for _ in 0..usize::from(width) * usize::from(height) {
        bgra.extend_from_slice(&pixel);
    }
    RawFrame::bgra(width, height, 10, bgra).unwrap()
}

#[test]
fn nv12_uses_limited_range_and_rejects_odd_sizes() {
    let black = solid(2, 2, [0, 0, 0, 255]);
    let nv12 = bgra_to_nv12(&black).unwrap();
    assert!(nv12.y_plane().iter().all(|sample| *sample == 16));
    assert!(nv12.uv_plane().iter().all(|sample| *sample == 128));
    let white = solid(2, 2, [255, 255, 255, 255]);
    let nv12 = bgra_to_nv12(&white).unwrap();
    assert!(nv12.y_plane().iter().all(|sample| *sample == 235));
    assert_eq!(nv12.uv_plane().len(), 2);
    let rendered = format!("{nv12:?}");
    assert!(!rendered.contains("235"));
    assert!(matches!(
        bgra_to_nv12(&solid(3, 2, [0, 0, 0, 255])),
        Err(Error::InvalidFrame("NV12 width and height must be even"))
    ));
}

#[test]
fn nv12_returns_within_rounding_of_the_original_picture() {
    for pixel in [
        [255, 0, 0, 255],
        [0, 255, 0, 255],
        [0, 0, 255, 255],
        [13, 77, 200, 255],
    ] {
        let frame = solid(4, 4, pixel);
        let nv12 = bgra_to_nv12(&frame).unwrap();
        let back = nv12_to_bgra(4, 4, 10, nv12.y_plane(), nv12.uv_plane()).unwrap();
        assert_eq!((back.width(), back.height()), (4, 4));
        for y in 0..4 {
            let row = back.row(y);
            for chunk in row.chunks_exact(4) {
                for channel in 0..3 {
                    let drift = (i16::from(chunk[channel]) - i16::from(pixel[channel])).abs();
                    assert!(drift <= 2, "channel drifted by {drift}");
                }
                assert_eq!(chunk[3], 255);
            }
        }
    }
    assert!(matches!(
        nv12_to_bgra(2, 2, 0, &[0; 3], &[0; 2]),
        Err(Error::InvalidFrame("frame is missing pixels"))
    ));
    assert!(matches!(
        nv12_to_bgra(3, 2, 0, &[0; 12], &[0; 6]),
        Err(Error::InvalidFrame("NV12 width and height must be even"))
    ));
}
