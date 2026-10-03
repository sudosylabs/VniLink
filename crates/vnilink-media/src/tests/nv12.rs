use crate::error::Error;
use crate::frame::RawFrame;
use crate::nv12::bgra_to_nv12;

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
