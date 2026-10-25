//! Generate synthetic VideoToolbox access units for another platform's decoder.
use vnilink_macos::MacEncoder;
use vnilink_media::{EncodeParams, RawFrame, VideoEncoder};
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let dir = std::path::PathBuf::from(std::env::args().nth(1).ok_or("output directory required")?);
    std::fs::create_dir_all(&dir)?;
    for (width, height) in [(64u16, 64u16), (1920, 1080)] {
        let mut encoder = MacEncoder::open(
            width,
            height,
            30,
            8_000_000,
            true,
            vnilink::YuvMatrix::Bt709,
        )?;
        for index in 0..3u64 {
            let mut pixels = Vec::new();
            for y in 0..height {
                for x in 0..width {
                    pixels.extend_from_slice(&[
                        (x / 4 + index as u16 * 9) as u8,
                        (y / 4) as u8,
                        ((x + y) / 8) as u8,
                        255,
                    ]);
                }
            }
            let frame = RawFrame::bgra(width, height, index * 33_333, pixels)?;
            let sample = encoder.encode(
                &frame,
                &EncodeParams {
                    force_keyframe: index == 0,
                    recovery: false,
                    bitrate_bps: 8_000_000,
                    frame_rate: 30,
                },
            )?;
            std::fs::write(
                dir.join(format!("mac-high-{width}-{index}.h264")),
                sample.bytes(),
            )?;
        }
    }
    Ok(())
}
