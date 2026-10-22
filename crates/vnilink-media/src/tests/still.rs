use shiguredo_aom as aom;

use crate::measure::{chroma_psnr, colour_text_page, psnr};
use crate::{RawFrame, StillEncoder};

/// Decodes one AV1 4:4:4 picture to BGRA, BT.709 limited range.
fn decode(picture: &[u8]) -> RawFrame {
    let mut decoder = aom::Decoder::new(aom::DecoderConfig::new()).unwrap();
    decoder.decode(picture).unwrap();
    let decoded = decoder.next_frame().expect("a picture");
    let (w, h) = (decoded.width(), decoded.height());
    let (y, u, v) = (
        decoded.y_plane().unwrap(),
        decoded.u_plane().unwrap(),
        decoded.v_plane().unwrap(),
    );
    let (ys, us, vs) = (
        decoded.y_stride().unwrap(),
        decoded.u_stride().unwrap(),
        decoded.v_stride().unwrap(),
    );
    let mut bgra = Vec::with_capacity(w * h * 4);
    for row in 0..h {
        for col in 0..w {
            let c = 298 * (i32::from(y[row * ys + col]) - 16);
            let d = i32::from(u[row * us + col]) - 128;
            let e = i32::from(v[row * vs + col]) - 128;
            let clamp = |value: i32| (value >> 8).clamp(0, 255) as u8;
            bgra.extend_from_slice(&[
                clamp(c + 541 * d + 128),
                clamp(c - 55 * d - 136 * e + 128),
                clamp(c + 459 * e + 128),
                255,
            ]);
        }
    }
    RawFrame::bgra(w as u16, h as u16, 0, bgra).unwrap()
}

#[test]
fn a_still_keeps_coloured_text_sharp_within_its_budget() {
    let page = colour_text_page(1280, 720, 0);
    let encoder = StillEncoder::new();
    let picture = encoder.encode(&page, 1_048_576).unwrap().expect("it fits");
    let shown = decode(&picture);
    assert_eq!((shown.width(), shown.height()), (1280, 720));
    let (whole, chroma) = (psnr(&page, &shown), chroma_psnr(&page, &shown));
    assert!(
        whole > 35.0 && chroma > 30.0,
        "{whole:.1} dB, chroma {chroma:.1} dB"
    );
    // A tight budget takes a coarser picture; one nothing fits takes none.
    let fine = picture.len();
    let coarse = encoder
        .encode(&page, fine - 1)
        .unwrap()
        .expect("a coarser one fits");
    assert!(coarse.len() < fine);
    assert!(encoder.encode(&page, 1_000).unwrap().is_none());
}
