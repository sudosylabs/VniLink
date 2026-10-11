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

#[test]
fn displays_from_crtcs_skips_disabled_outputs_and_rejects_a_bad_list() {
    use crate::x11::{displays_from_crtcs, CrtcGeometry};

    let crtc = |id, x, y, width, height, rotation, mode| CrtcGeometry {
        id,
        x,
        y,
        width,
        height,
        rotation,
        mode,
    };
    let displays = displays_from_crtcs(
        3,
        &[
            crtc(0, 0, 0, 10, 10, 1, 1),
            crtc(7, 0, 0, 10, 10, 1, 0),
            crtc(8, 0, 0, 0, 10, 1, 3),
            crtc(4, -1920, 0, 1920, 1080, 2, 1),
            crtc(5, 0, 10, 1280, 800, 4, 9),
        ],
    )
    .unwrap();
    assert_eq!(displays.len(), 2);
    assert_eq!(displays[0].id, 4);
    assert_eq!(displays[0].origin_x, -1920);
    assert_eq!(displays[0].origin_y, 0);
    assert_eq!(displays[0].width, 1920);
    assert_eq!(displays[0].rotation_deg, 90);
    assert_eq!(displays[0].scale_percent, 100);
    assert_eq!(displays[0].revision, 3);
    assert_eq!(displays[1].id, 5);
    assert_eq!(displays[1].rotation_deg, 180);
    assert_eq!(
        displays_from_crtcs(1, &[crtc(6, 0, 0, 100, 100, 8, 1)]).unwrap()[0].rotation_deg,
        270
    );
    assert_eq!(
        displays_from_crtcs(1, &[crtc(6, 0, 0, 100, 100, 1, 1)]).unwrap()[0].rotation_deg,
        0
    );
    assert!(matches!(
        displays_from_crtcs(
            1,
            &[crtc(7, 0, 0, 10, 10, 1, 0), crtc(0, 0, 0, 10, 10, 1, 1)]
        ),
        Err(Error::CaptureRejected("no display is available"))
    ));
    assert!(matches!(
        displays_from_crtcs(
            2,
            &[
                crtc(4, -1920, 0, 1920, 1080, 1, 1),
                crtc(9, 0, 0, 20_000, 100, 1, 1),
            ],
        ),
        Err(Error::InvalidSetting("a display size is out of range"))
    ));
}

#[test]
fn twin_crtc_crops_read_exact_rects_and_refuse_unreadable_ones() {
    use crate::x11::{crop_rect, CrtcGeometry};

    let crtc = |id, x, y, width, height| CrtcGeometry {
        id,
        x,
        y,
        width,
        height,
        rotation: 1,
        mode: 1,
    };
    // Same size, distinct origins: each twin reads its own rectangle.
    assert_eq!(
        crop_rect(&crtc(63, 0, 0, 1920, 1080)),
        Some((0, 0, 1920, 1080))
    );
    assert_eq!(
        crop_rect(&crtc(64, 1920, 0, 1920, 1080)),
        Some((1920, 0, 1920, 1080))
    );
    // Left/above-primary monitors keep their negative origins.
    assert_eq!(
        crop_rect(&crtc(65, -1920, -1080, 1920, 1080)),
        Some((-1920, -1080, 1920, 1080))
    );
    // Disabled, empty, oversized and id-less CRTCs are refused.
    assert_eq!(
        crop_rect(&CrtcGeometry {
            mode: 0,
            ..crtc(66, 0, 0, 1920, 1080)
        }),
        None
    );
    assert_eq!(crop_rect(&crtc(67, 0, 0, 0, 1080)), None);
    assert_eq!(crop_rect(&crtc(0, 0, 0, 1920, 1080)), None);
    assert_eq!(
        crop_rect(&crtc(68, 0, 0, vnilink::MAX_DISPLAY_EDGE + 1, 1080)),
        None
    );
    assert_eq!(
        crop_rect(&crtc(
            68,
            0,
            0,
            vnilink::MAX_DISPLAY_EDGE,
            vnilink::MAX_DISPLAY_EDGE
        )),
        Some((0, 0, vnilink::MAX_DISPLAY_EDGE, vnilink::MAX_DISPLAY_EDGE))
    );
}

#[test]
fn stride_and_cursor_offset_follow_the_crop() {
    use crate::x11::{offset_cursor, stride_for};

    assert_eq!(stride_for(1920, 32, 4), 1920 * 4);
    assert_eq!(stride_for(1366, 32, 4), 1366 * 4);
    // A 24-bit row that misses the pad rounds up.
    assert_eq!(stride_for(5, 24, 4), 16);
    assert_eq!(stride_for(64, 32, 4), 256);

    assert_eq!(offset_cursor((2000, 100), (1920, 0)), (80, 100));
    assert_eq!(offset_cursor((0, 0), (0, 0)), (0, 0));
    // Outside the crop keeps its offset; viewers clip it.
    assert_eq!(offset_cursor((10, 10), (1920, 0)), (-1910, 10));
    assert_eq!(offset_cursor((100, 100), (-1920, 0)), (2020, 100));
}

#[test]
fn xfixes_cursor_preserves_argb_alpha_hotspot_and_bounds() {
    let mut reply = x11rb::protocol::xfixes::GetCursorImageReply::default();
    reply.width = 2;
    reply.height = 1;
    reply.xhot = 1;
    reply.yhot = 0;
    reply.cursor_image = vec![0xff112233, 0x80402010];
    let image = crate::x11::cursor_image(&reply).unwrap();
    assert_eq!(image.hotspot(), (1, 0));
    assert_eq!(
        image.pixels(),
        &[0x33, 0x22, 0x11, 0xff, 0x10, 0x20, 0x40, 0x80]
    );
    reply.xhot = 2;
    assert!(crate::x11::cursor_image(&reply).is_none());
    reply.xhot = 1;
    reply.cursor_image.pop();
    assert!(crate::x11::cursor_image(&reply).is_none());
    reply.width = 513;
    assert!(crate::x11::cursor_image(&reply).is_none());
}

#[test]
#[ignore = "requires an isolated Xvfb server and VNILINK_TEST_X11=private"]
fn real_xfixes_cursor_is_independent_from_capture_and_uses_shared_shape_storage() {
    assert_eq!(std::env::var("VNILINK_TEST_X11").as_deref(), Ok("private"));
    let capture = crate::X11Capture::open(&X11Plan::current_session()).unwrap();
    assert!(capture.separate_cursor());
    let first = capture.cursor().unwrap();
    let next = capture.cursor().unwrap();
    assert_eq!(first, next);
    assert!(std::sync::Arc::ptr_eq(
        first.image.as_ref().unwrap(),
        next.image.as_ref().unwrap()
    ));
    let frame = capture.next_frame(1).unwrap();
    assert_eq!((frame.width(), frame.height()), capture.size());
}
