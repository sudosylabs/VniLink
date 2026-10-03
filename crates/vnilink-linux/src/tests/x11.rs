use vnilink_media::Error;

use crate::x11::{raw_frame_from_ximage, X11Image, X11Plan};

#[test]
fn x11_images_must_be_little_endian_bgra() {
    let pixels = [1u8, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16];
    let frame = raw_frame_from_ximage(
        X11Image {
            width: 2,
            height: 2,
            stride: 8,
            depth: 24,
            bits_per_pixel: 32,
            lsb_first: true,
            bytes: &pixels,
        },
        5,
    )
    .unwrap();
    assert_eq!(frame.timestamp_us(), 5);
    assert_eq!(frame.row(1), &pixels[8..16]);

    assert!(matches!(
        raw_frame_from_ximage(
            X11Image {
                width: 2,
                height: 2,
                stride: 8,
                depth: 16,
                bits_per_pixel: 16,
                lsb_first: true,
                bytes: &pixels,
            },
            0,
        ),
        Err(Error::Unsupported("this X11 pixel format"))
    ));
    assert!(matches!(
        raw_frame_from_ximage(
            X11Image {
                width: 2,
                height: 2,
                stride: 8,
                depth: 24,
                bits_per_pixel: 32,
                lsb_first: false,
                bytes: &pixels,
            },
            0,
        ),
        Err(Error::Unsupported("this X11 pixel format"))
    ));
    assert!(matches!(
        raw_frame_from_ximage(
            X11Image {
                width: 2,
                height: 2,
                stride: 8,
                depth: 24,
                bits_per_pixel: 32,
                lsb_first: true,
                bytes: &[0, 1, 2, 3],
            },
            0,
        ),
        Err(Error::InvalidFrame("frame is missing pixels"))
    ));
    assert_eq!(X11Plan::current_session().screen, 0);
    assert!(X11Plan::current_session().display_name.is_none());
}
