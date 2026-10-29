//! The chroma measurement's page and metrics measure what they say.

use crate::measure::{chroma_psnr, colour_text_page, edge_psnr, psnr, psnr_channels};
use crate::{bgra_to_nv12, nv12_to_bgra, RawFrame, VideoSignal, YuvMatrix};

fn changed(frame: &RawFrame, at: impl Fn(u16, u16) -> bool, edit: impl Fn(&mut [u8])) -> RawFrame {
    let mut pixels = Vec::new();
    for y in 0..frame.height() {
        for (x, pixel) in frame.row(y).chunks_exact(4).enumerate() {
            let mut pixel = [pixel[0], pixel[1], pixel[2], pixel[3]];
            if at(x as u16, y) {
                edit(&mut pixel);
            }
            pixels.extend_from_slice(&pixel);
        }
    }
    RawFrame::bgra(frame.width(), frame.height(), 0, pixels).unwrap()
}

#[test]
fn each_metric_answers_only_its_own_question() {
    let page = colour_text_page(320, 180, 0);
    assert_eq!(page.pixels(), colour_text_page(320, 180, 0).pixels());
    // Typing changes only its own line (y 54..63): the rest of the page holds.
    let typed = colour_text_page(320, 180, 20);
    for y in (0..180u16).filter(|y| !(54..63).contains(y)) {
        assert_eq!(page.row(y), typed.row(y), "row {y} changed while typing");
    }
    assert_ne!(page.pixels(), typed.pixels());
    assert!(psnr(&page, &page) > 80.0);
    // Red alone changed: red's PSNR drops, green's and blue's do not.
    let red = changed(
        &page,
        |_, _| true,
        |pixel| pixel[2] = pixel[2].saturating_sub(20),
    );
    let [r, g, b] = psnr_channels(&page, &red);
    assert!(r < 35.0 && g > 80.0 && b > 80.0, "{r} {g} {b}");
    // The same brightness change in every channel moves luma, not chroma.
    let brighter = changed(
        &page,
        |_, _| true,
        |pixel| {
            for channel in &mut pixel[..3] {
                *channel = channel.saturating_add(12).min(250);
            }
        },
    );
    assert!(chroma_psnr(&page, &brighter) > psnr(&page, &brighter) + 10.0);
    // An error in the empty margin counts in PSNR but not at the edges.
    let margin = changed(&page, |x, y| x < 4 && y < 4, |pixel| pixel[1] = 0);
    assert!(psnr(&page, &margin) < 60.0);
    assert!(edge_psnr(&page, &margin) > 80.0);
}

/// 4:2:0 alone, with no codec, already costs the coloured page most of its
/// chroma: that is the loss 4:4:4 is about.
#[test]
fn the_colour_page_loses_chroma_to_subsampling_alone() {
    let page = colour_text_page(640, 360, 0);
    let nv12 = bgra_to_nv12(&page, YuvMatrix::Bt709).unwrap();
    let back = nv12_to_bgra(
        640,
        360,
        0,
        nv12.y_plane(),
        nv12.uv_plane(),
        VideoSignal::stream(YuvMatrix::Bt709),
    )
    .unwrap();
    let (chroma, edges) = (chroma_psnr(&page, &back), edge_psnr(&page, &back));
    assert!(chroma < 32.0, "chroma {chroma}");
    assert!(edges < 26.0, "edges {edges}");
    // A grey page keeps its chroma: there is none to lose.
    let grey = changed(
        &page,
        |_, _| true,
        |pixel| {
            let mean =
                ((u16::from(pixel[0]) + u16::from(pixel[1]) + u16::from(pixel[2])) / 3) as u8;
            pixel[..3].fill(mean);
        },
    );
    let nv12 = bgra_to_nv12(&grey, YuvMatrix::Bt709).unwrap();
    let back = nv12_to_bgra(
        640,
        360,
        0,
        nv12.y_plane(),
        nv12.uv_plane(),
        VideoSignal::stream(YuvMatrix::Bt709),
    )
    .unwrap();
    assert!(chroma_psnr(&grey, &back) > 45.0);
}
