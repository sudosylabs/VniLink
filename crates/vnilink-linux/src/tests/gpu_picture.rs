// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

use std::path::PathBuf;

use crate::gpu::EncoderChoice;
use crate::gpu_picture::{
    choose_modifier, damage_is_empty, first_vaapi_node, MODIFIER_I915_X_TILED,
    MODIFIER_I915_Y_TILED, MODIFIER_LINEAR,
};

#[test]
fn the_first_layout_this_side_prefers_that_the_compositor_offers_is_chosen() {
    assert_eq!(
        choose_modifier(&[
            MODIFIER_LINEAR,
            MODIFIER_I915_X_TILED,
            MODIFIER_I915_Y_TILED
        ]),
        Some(MODIFIER_I915_Y_TILED)
    );
    assert_eq!(
        choose_modifier(&[MODIFIER_LINEAR, MODIFIER_I915_X_TILED]),
        Some(MODIFIER_I915_X_TILED)
    );
    assert_eq!(choose_modifier(&[MODIFIER_LINEAR]), Some(MODIFIER_LINEAR));
}

#[test]
fn a_compositor_whose_layouts_are_all_unknown_gets_no_gpu_path() {
    // An AMD compressed layout, say.
    assert_eq!(choose_modifier(&[(0x02 << 56) | 0x1234]), None);
    assert_eq!(choose_modifier(&[]), None);
}

#[test]
fn damage_names_a_change_only_with_a_region_that_has_an_area() {
    assert!(damage_is_empty(&[]));
    assert!(damage_is_empty(&[[0, 0, 0, 0]]));
    assert!(damage_is_empty(&[[10, 10, 0, 5]]));
    assert!(!damage_is_empty(&[[0, 0, 1920, 1080], [0, 0, 0, 0]]));
    assert!(!damage_is_empty(&[[559, 276, 802, 602]]));
}

#[test]
fn pictures_stay_on_the_gpu_only_for_a_vaapi_first_encoder() {
    let vaapi = EncoderChoice::Vaapi {
        render_node: PathBuf::from("/dev/dri/renderD128"),
    };
    let nvenc = EncoderChoice::Nvenc {
        pci_address: Some("0000:01:00.0".into()),
    };
    assert_eq!(
        first_vaapi_node(&[vaapi.clone(), nvenc.clone()]),
        Some(PathBuf::from("/dev/dri/renderD128"))
    );
    // NVENC on the compositor's GPU, VA-API behind it: NVENC wins, and it
    // reads memory.
    assert_eq!(first_vaapi_node(&[nvenc, vaapi]), None);
    assert_eq!(first_vaapi_node(&[]), None);
}

/// Four flat quadrants, BGRX, to stand in for a compositor's buffer.
fn quadrant_colour(x: usize, y: usize, width: usize, height: usize) -> [u8; 4] {
    match (x < width / 2, y < height / 2) {
        (true, true) => [40, 120, 200, 255],
        (false, true) => [255, 0, 0, 255],
        (true, false) => [0, 255, 0, 255],
        (false, false) => [128, 128, 128, 255],
    }
}

/// A screen picture imported into VA-API, copied into the capture's pool,
/// read back as it was drawn, and scaled into an encoder picture whose
/// height is not a multiple of 16, then decoded: the colours and the layout
/// survive, with nothing mapped through GBM. It runs on a render node with
/// a video processor and H.265 encode.
#[test]
#[ignore = "needs a VA-API render node with a video processor and H.265 encode"]
fn a_picture_on_the_gpu_is_scaled_into_an_encoder_picture() {
    use libva::{Display, Image, UsageHint};
    use vnilink_media::{NativePicture, RawFrame, SoftwareDecoder, VideoDecoder};

    use crate::gpu_picture::{Importer, Prime};
    use crate::vaapi::{Input, Session, VaCodec};

    let Some(node) = crate::vaapi::offering(VaCodec::H265) else {
        eprintln!("no render node encodes H.265");
        return;
    };
    let (width, height) = (1280usize, 720usize);
    // The compositor's buffer, drawn by another display on the same node.
    let compositor = Display::open_drm_display(&node).unwrap();
    let source = compositor
        .create_surfaces(
            libva::VA_RT_FORMAT_RGB32,
            Some(libva::VA_FOURCC_BGRX),
            width as u32,
            height as u32,
            Some(UsageHint::USAGE_HINT_VPP_READ),
            vec![()],
        )
        .unwrap()
        .pop()
        .unwrap();
    {
        let format = compositor
            .query_image_formats()
            .unwrap()
            .into_iter()
            .find(|format| format.fourcc == libva::VA_FOURCC_BGRX)
            .unwrap();
        let size = (width as u32, height as u32);
        let mut image = match Image::derive_from(&source, size) {
            Ok(image) => image,
            Err(_) => Image::create_from(&source, format, size, size).unwrap(),
        };
        let descriptor = *image.image();
        let (offset, pitch) = (
            descriptor.offsets[0] as usize,
            descriptor.pitches[0] as usize,
        );
        let bytes = image.as_mut();
        for y in 0..height {
            for x in 0..width {
                let at = offset + y * pitch + x * 4;
                bytes[at..at + 4].copy_from_slice(&quadrant_colour(x, y, width, height));
            }
        }
    }
    let buffer = Prime::exported(&source).unwrap();

    let mut importer = Importer::open(&node, (width as u16, height as u16)).unwrap();
    let picture = importer
        .copy(1, || buffer.duplicate())
        .unwrap()
        .expect("a free picture");
    assert_eq!(picture.size(), (width as u16, height as u16));
    let bytes = picture.read_back().unwrap();
    for (x, y) in [(10, 10), (900, 10), (10, 600), (900, 600)] {
        let at = (y * width + x) * 4;
        assert_eq!(
            bytes[at..at + 4],
            quadrant_colour(x, y, width, height),
            "pixel {x},{y}"
        );
    }

    // The pool is small and a picture stays taken while a frame holds it.
    let mut held = vec![picture];
    while let Some(next) = importer.copy(1, || buffer.duplicate()).unwrap() {
        held.push(next);
        assert!(held.len() <= 8, "the pool never runs out");
    }
    let taken = held.len();
    held.pop();
    assert!(importer.copy(1, || buffer.duplicate()).unwrap().is_some());
    assert!(taken >= 2);
    eprintln!("{taken} pictures taken before the pool ran out");
    held.truncate(1);
    let picture = held.pop().unwrap();

    let session = Session::open(
        &node,
        VaCodec::H265,
        640,
        360,
        30,
        4_000_000,
        vnilink::YuvMatrix::Bt709,
    )
    .unwrap();
    if !session.converts() {
        eprintln!("{} has no video processor for the encoder", node.display());
        return;
    }
    let frame = RawFrame::native(0, Box::new(picture)).unwrap();
    let sample = session.encode(Input::Bgra(frame), true).unwrap();
    let encoded =
        vnilink::EncodedFrame::new(vnilink::VideoCodec::H265, true, 0, sample.bytes().to_vec())
            .unwrap();
    let decoded = SoftwareDecoder::open(vnilink::VideoCodec::H265)
        .unwrap()
        .decode(&encoded)
        .unwrap();
    assert_eq!((decoded.width(), decoded.height()), (640, 360));
    for (x, y) in [(80, 45), (560, 45), (80, 315), (560, 315)] {
        let at = y * decoded.stride() + x * 4;
        let got = &decoded.pixels()[at..at + 3];
        let want = quadrant_colour(x, y, 640, 360);
        assert!(
            got.iter()
                .zip(&want[..3])
                .all(|(got, want)| got.abs_diff(*want) <= 8),
            "pixel {x},{y} decoded as {got:?}, drawn as {want:?}"
        );
    }
    // The picture reaches the bottom row of the screen, not a stretched one.
    let last = 359 * decoded.stride() + 80 * 4;
    let want = quadrant_colour(80, 359, 640, 360);
    assert!(decoded.pixels()[last..last + 3]
        .iter()
        .zip(&want[..3])
        .all(|(got, want)| got.abs_diff(*want) <= 8));
}
