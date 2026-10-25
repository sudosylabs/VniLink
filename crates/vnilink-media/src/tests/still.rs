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

/// The browser scenario's still: a page of coloured text as `StillEncoder`
/// makes it, and the crop of its source where 4:2:0 loses most.
const FIXTURE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../test/browser/fixtures/still.json"
);
const FIXTURE_SIZE: (u16, u16) = (640, 360);
const CROP: (usize, usize) = (64, 32);

/// Straight RGBA of `frame` within `crop` (x, y, width, height), as a
/// canvas reads it back.
fn rgba_crop(frame: &RawFrame, (x, y, width, height): (usize, usize, usize, usize)) -> Vec<u8> {
    let mut out = Vec::with_capacity(width * height * 4);
    for row in y..y + height {
        for pixel in frame.row(row as u16)[x * 4..(x + width) * 4].chunks_exact(4) {
            out.extend_from_slice(&[pixel[2], pixel[1], pixel[0], 255]);
        }
    }
    out
}

/// PSNR of two RGBA crops over their colour channels.
fn rgba_psnr(left: &[u8], right: &[u8]) -> f64 {
    let (mut sum, mut count) = (0.0, 0.0);
    for (a, b) in left.chunks_exact(4).zip(right.chunks_exact(4)) {
        for channel in 0..3 {
            let error = f64::from(a[channel]) - f64::from(b[channel]);
            sum += error * error;
            count += 1.0;
        }
    }
    10.0 * (255.0 * 255.0 / (sum / count).max(1e-9)).log10()
}

/// The crop where a 4:2:0 round trip in the stream's colour loses most,
/// and that loss.
fn colourful_crop(page: &RawFrame) -> ((usize, usize, usize, usize), f64) {
    let nv12 = crate::bgra_to_nv12(page, crate::YuvMatrix::Bt709).unwrap();
    let subsampled = crate::nv12_to_bgra(
        page.width(),
        page.height(),
        0,
        nv12.y_plane(),
        nv12.uv_plane(),
        crate::VideoSignal::stream(crate::YuvMatrix::Bt709),
    )
    .unwrap();
    let (width, height) = (usize::from(page.width()), usize::from(page.height()));
    let mut worst = ((0, 0, CROP.0, CROP.1), f64::MAX);
    for y in (0..=height - CROP.1).step_by(8) {
        for x in (0..=width - CROP.0).step_by(8) {
            let crop = (x, y, CROP.0, CROP.1);
            let loss = rgba_psnr(&rgba_crop(page, crop), &rgba_crop(&subsampled, crop));
            if loss < worst.1 {
                worst = (crop, loss);
            }
        }
    }
    worst
}

fn base64(bytes: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

fn unbase64(text: &str) -> Vec<u8> {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD
        .decode(text)
        .expect("base64")
}

/// Writes the fixture: `cargo test -p vnilink-media --lib write_still_fixture -- --ignored`.
#[test]
#[ignore = "writes test/browser/fixtures/still.json"]
fn write_still_fixture() {
    let page = colour_text_page(FIXTURE_SIZE.0, FIXTURE_SIZE.1, 0);
    let still = StillEncoder::new()
        .encode(&page, vnilink::MAX_VIDEO_FRAME)
        .unwrap()
        .expect("it fits");
    let (crop, subsampled) = colourful_crop(&page);
    let meta = serde_json::json!({
        "width": FIXTURE_SIZE.0,
        "height": FIXTURE_SIZE.1,
        "crop": { "x": crop.0, "y": crop.1, "width": crop.2, "height": crop.3 },
        "subsampledPsnr": (subsampled * 10.0).round() / 10.0,
        "still": base64(&still),
        "glyph": base64(&rgba_crop(&page, crop)),
    });
    std::fs::write(FIXTURE, serde_json::to_string_pretty(&meta).unwrap() + "\n").unwrap();
}

/// The committed fixture is a still of its page: it decodes, through
/// libaom, to the source crop far closer than 4:2:0 keeps it.
#[test]
fn the_still_fixture_keeps_its_coloured_glyph() {
    let meta: serde_json::Value = serde_json::from_slice(&std::fs::read(FIXTURE).unwrap()).unwrap();
    let bytes = |key: &str| unbase64(meta[key].as_str().unwrap());
    let crop = |key: &str| meta["crop"][key].as_u64().unwrap() as usize;
    let crop = (crop("x"), crop("y"), crop("width"), crop("height"));
    let shown = decode(&bytes("still"));
    assert_eq!((shown.width(), shown.height()), FIXTURE_SIZE);
    let source = bytes("glyph");
    assert_eq!(
        source,
        rgba_crop(&colour_text_page(FIXTURE_SIZE.0, FIXTURE_SIZE.1, 0), crop),
        "the page changed: write the fixture again"
    );
    let kept = rgba_psnr(&source, &rgba_crop(&shown, crop));
    let subsampled = meta["subsampledPsnr"].as_f64().unwrap();
    assert!(
        kept > subsampled + 6.0,
        "{kept:.1} dB against {subsampled:.1} at 4:2:0"
    );
}

/// A still of grey text is not sent: 4:2:0 loses almost nothing there, and
/// the stream's own picture is as good.
#[test]
fn a_still_of_grey_text_is_not_worth_sending() {
    let coloured = colour_text_page(640, 360, 0);
    let grey: Vec<u8> = coloured
        .pixels()
        .chunks_exact(4)
        .flat_map(|pixel| {
            let luma =
                ((u32::from(pixel[2]) * 54 + u32::from(pixel[1]) * 183 + u32::from(pixel[0]) * 19)
                    >> 8) as u8;
            [luma, luma, luma, 255]
        })
        .collect();
    let grey = RawFrame::bgra(640, 360, 0, grey).unwrap();
    let encoder = StillEncoder::new();
    assert!(encoder.encode(&grey, 1_048_576).unwrap().is_none());
    assert!(
        encoder.encode(&coloured, 1_048_576).unwrap().is_some(),
        "coloured text is"
    );
}

/// A still is scaled to the stream's size by averaging: each destination
/// pixel is the mean of the source pixels it covers.
#[test]
fn a_still_is_scaled_down_by_averaging() {
    // 4 x 2 pixels: two 2 x 2 boxes, one of black and white, one of grey.
    let pixel = |value: u8| [value, value, value, 255];
    let rows = [
        [pixel(0), pixel(255), pixel(100), pixel(100)],
        [pixel(255), pixel(0), pixel(100), pixel(100)],
    ];
    let bgra: Vec<u8> = rows.iter().flatten().flatten().copied().collect();
    let frame = RawFrame::bgra(4, 2, 7, bgra).unwrap();
    let scaled = crate::still::area_scale(&frame, (2, 1)).unwrap();
    assert_eq!(
        (scaled.width(), scaled.height(), scaled.timestamp_us()),
        (2, 1, 7)
    );
    assert_eq!(scaled.row(0), &[128, 128, 128, 255, 100, 100, 100, 255]);
    assert!(
        crate::still::area_scale(&frame, (8, 4)).is_err(),
        "it only scales down"
    );
}
