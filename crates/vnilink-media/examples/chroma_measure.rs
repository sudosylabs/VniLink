//! What 4:4:4 buys coloured text, and what it costs, with the codec held
//! fixed: libaom AV1 at 4:2:0 (Main) and at 4:4:4 (High), at the same target
//! bitrates, on `measure::colour_text_page`, plus 4:2:0 with no codec at all.
//! Colour follows the stream contract (BT.709, limited range) both ways.
//!
//! `cargo run -p vnilink-media --release --example chroma_measure -- <output dir>`
//!
//! Prints one table row per size, format and bitrate: bytes per second sent
//! over two seconds of typing, then PSNR, chroma PSNR and edge PSNR of the
//! keyframe and of the last picture. Writes PPM crops of the last picture
//! at 2 Mb/s for judging by eye.

use shiguredo_aom as aom;
use vnilink_media::measure::{chroma_psnr, colour_text_page, edge_psnr, psnr};
use vnilink_media::{bgra_to_nv12, nv12_to_bgra, RawFrame, VideoSignal, YuvMatrix};

const PICTURES: u64 = 60;
const FPS: u32 = 30;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Chroma {
    Yuv420,
    Yuv444,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let out = std::path::PathBuf::from(std::env::args().nth(1).ok_or("output directory required")?);
    std::fs::create_dir_all(&out)?;
    println!("| Size | Chroma | Target Mb/s | Sent kB/s | Key PSNR | Key chroma | Key edge | Last PSNR | Last chroma | Last edge |");
    println!("|---|---|---|---|---|---|---|---|---|---|");
    for (width, height) in [(1280u16, 720u16), (1920, 1080)] {
        let page = colour_text_page(width, height, PICTURES - 1);
        let nv12 = bgra_to_nv12(&page, YuvMatrix::Bt709)?;
        let subsampled = nv12_to_bgra(
            width,
            height,
            0,
            nv12.y_plane(),
            nv12.uv_plane(),
            VideoSignal::stream(YuvMatrix::Bt709),
        )?;
        println!(
            "| {width}x{height} | 4:2:0, no codec | - | - | - | - | - | {:.1} | {:.1} | {:.1} |",
            psnr(&page, &subsampled),
            chroma_psnr(&page, &subsampled),
            edge_psnr(&page, &subsampled)
        );
        if width == 1920 {
            crop(&page, &out.join("source.ppm"))?;
            crop(&subsampled, &out.join("subsampled-420.ppm"))?;
        }
        for chroma in [Chroma::Yuv420, Chroma::Yuv444] {
            for megabits in [1u32, 2, 4, 8] {
                let (bytes, key, last) = run(width, height, chroma, megabits * 1_000_000)?;
                let source_key = colour_text_page(width, height, 0);
                let source_last = colour_text_page(width, height, PICTURES - 1);
                println!(
                    "| {width}x{height} | {} | {megabits} | {:.0} | {:.1} | {:.1} | {:.1} | {:.1} | {:.1} | {:.1} |",
                    if chroma == Chroma::Yuv420 { "AV1 4:2:0" } else { "AV1 4:4:4" },
                    bytes as f64 / 1000.0 / (PICTURES as f64 / f64::from(FPS)),
                    psnr(&source_key, &key),
                    chroma_psnr(&source_key, &key),
                    edge_psnr(&source_key, &key),
                    psnr(&source_last, &last),
                    chroma_psnr(&source_last, &last),
                    edge_psnr(&source_last, &last),
                );
                if width == 1920 && megabits == 2 {
                    let name = if chroma == Chroma::Yuv420 {
                        "av1-420-2mbps.ppm"
                    } else {
                        "av1-444-2mbps.ppm"
                    };
                    crop(&last, &out.join(name))?;
                }
            }
        }
    }
    Ok(())
}

/// Encodes two seconds of the page being typed on and decodes each picture:
/// total bytes, the decoded keyframe and the decoded last picture.
fn run(
    width: u16,
    height: u16,
    chroma: Chroma,
    bitrate: u32,
) -> Result<(usize, RawFrame, RawFrame), Box<dyn std::error::Error>> {
    let format = if chroma == Chroma::Yuv420 {
        aom::ImageFormat::I420
    } else {
        aom::ImageFormat::I444
    };
    let mut config = aom::EncoderConfig::new(width.into(), height.into(), format);
    // AV1 Main is 4:2:0; High adds 4:4:4.
    config.g_profile = if chroma == Chroma::Yuv420 { 0 } else { 1 };
    config.g_usage = aom::Usage::Realtime;
    config.g_timebase = aom::AomRational {
        num: 1,
        den: FPS as i32,
    };
    config.g_threads = Some(8);
    config.g_lag_in_frames = Some(0);
    config.cpu_used = Some(10);
    config.rc_end_usage = aom::RateControlMode::Cbr;
    config.rc_target_bitrate = bitrate / 1000;
    config.tune_content = Some(aom::ContentType::Screen);
    let mut encoder = aom::Encoder::new(config)?;
    let mut decoder = aom::Decoder::new(aom::DecoderConfig::new())?;
    let (mut bytes, mut key, mut last) = (0usize, None, None);
    for index in 0..PICTURES {
        let page = colour_text_page(width, height, index);
        let (y, u, v) = planes(&page, chroma);
        let image = match chroma {
            Chroma::Yuv420 => aom::ImageData::I420 {
                y: &y,
                u: &u,
                v: &v,
            },
            Chroma::Yuv444 => aom::ImageData::I444 {
                y: &y,
                u: &u,
                v: &v,
            },
        };
        encoder.encode(
            &image,
            &aom::EncodeOptions {
                force_keyframe: index == 0,
            },
        )?;
        while let Some(packet) = encoder.next_frame() {
            let data = packet.data()?.to_vec();
            bytes += data.len();
            decoder.decode(&data)?;
            while let Some(picture) = decoder.next_frame() {
                let shown = picture_bgra(&picture, chroma)?;
                if key.is_none() {
                    key = Some(shown.clone());
                }
                last = Some(shown);
            }
        }
    }
    Ok((bytes, key.ok_or("no keyframe")?, last.ok_or("no picture")?))
}

/// BT.709 limited planes: 4:2:0 through the product's own conversion, 4:4:4
/// with the same coefficients per pixel.
fn planes(frame: &RawFrame, chroma: Chroma) -> (Vec<u8>, Vec<u8>, Vec<u8>) {
    if chroma == Chroma::Yuv420 {
        let nv12 = bgra_to_nv12(frame, YuvMatrix::Bt709).expect("an even page");
        let (u, v) = nv12
            .uv_plane()
            .chunks_exact(2)
            .map(|pair| (pair[0], pair[1]))
            .unzip();
        return (nv12.y_plane().to_vec(), u, v);
    }
    let (mut y, mut u, mut v) = (Vec::new(), Vec::new(), Vec::new());
    for row in 0..frame.height() {
        for pixel in frame.row(row).chunks_exact(4) {
            let (b, g, r) = (
                i32::from(pixel[0]),
                i32::from(pixel[1]),
                i32::from(pixel[2]),
            );
            y.push((((47 * r + 157 * g + 16 * b + 128) >> 8) + 16) as u8);
            u.push((((-26 * r - 86 * g + 112 * b + 128) >> 8) + 128) as u8);
            v.push((((112 * r - 102 * g - 10 * b + 128) >> 8) + 128) as u8);
        }
    }
    (y, u, v)
}

fn picture_bgra(
    picture: &aom::DecodedFrame<'_>,
    chroma: Chroma,
) -> Result<RawFrame, Box<dyn std::error::Error>> {
    let (w, h) = (picture.width(), picture.height());
    let (y, u, v) = (picture.y_plane()?, picture.u_plane()?, picture.v_plane()?);
    let (ys, us, vs) = (
        picture.y_stride()?,
        picture.u_stride()?,
        picture.v_stride()?,
    );
    if chroma == Chroma::Yuv420 {
        let mut luma = Vec::with_capacity(w * h);
        let mut uv = Vec::with_capacity(w * h / 2);
        for row in 0..h {
            luma.extend_from_slice(&y[row * ys..row * ys + w]);
        }
        for row in 0..h / 2 {
            for col in 0..w / 2 {
                uv.push(u[row * us + col]);
                uv.push(v[row * vs + col]);
            }
        }
        return Ok(nv12_to_bgra(
            w as u16,
            h as u16,
            0,
            &luma,
            &uv,
            VideoSignal::stream(YuvMatrix::Bt709),
        )?);
    }
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
    Ok(RawFrame::bgra(w as u16, h as u16, 0, bgra)?)
}

/// The top-left 480x270 of `frame` as binary PPM.
fn crop(frame: &RawFrame, path: &std::path::Path) -> std::io::Result<()> {
    let (w, h) = (
        480usize.min(usize::from(frame.width())),
        270u16.min(frame.height()),
    );
    let mut out = format!("P6 {w} {h} 255\n").into_bytes();
    for y in 0..h {
        for pixel in frame.row(y)[..w * 4].chunks_exact(4) {
            out.extend_from_slice(&[pixel[2], pixel[1], pixel[0]]);
        }
    }
    std::fs::write(path, out)
}
