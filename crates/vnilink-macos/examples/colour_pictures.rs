//! The reference pictures of `src/workbench/services/camera/decoderColour.ts`:
//! one flat 256x256 keyframe per codec family and matrix, BGR 200, 120, 40
//! converted and declared in that matrix by VniLink's own encoders
//! (VideoToolbox for H.264, the software encoders for the others), as lines of
//! codec label and hex, which the TypeScript source holds as base64. Synthetic; never captures a screen.
#[cfg(target_os = "macos")]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    use vnilink::{VideoCodec, YuvMatrix};
    use vnilink_media::{EncodeParams, RawFrame, SoftwareEncoder, VideoEncoder};

    if std::env::args().nth(1).as_deref() == Some("chart") {
        return chart();
    }
    let frame = RawFrame::bgra(256, 256, 0, [200u8, 120, 40, 255].repeat(256 * 256))?;
    let params = EncodeParams {
        force_keyframe: true,
        recovery: false,
        bitrate_bps: 4_000_000,
        frame_rate: 30,
    };
    for matrix in [YuvMatrix::Bt709, YuvMatrix::Bt601] {
        println!("// {matrix:?}");
        for codec in [
            VideoCodec::H264,
            VideoCodec::H265,
            VideoCodec::Vp8,
            VideoCodec::Vp9,
            VideoCodec::Av1,
        ] {
            let sample = if codec == VideoCodec::H264 {
                vnilink_macos::MacEncoder::open(256, 256, 30, 4_000_000, true, matrix)?
                    .encode(&frame, &params)?
            } else {
                SoftwareEncoder::open(codec, matrix, 256, 256, 30, 4_000_000)?
                    .encode(&frame, &params)?
            };
            let hex: String = sample
                .bytes()
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect();
            println!("{} {hex}", codec.label());
        }
    }
    Ok(())
}

/// `chart`: a 256x256 picture of 4x4 flat colour blocks encoded by the
/// platform's own encoders (VideoToolbox H.264 and H.265) in both matrices, to
/// compare with what a window shows. Prints `<matrix> <codec> <hex>` lines.
#[cfg(target_os = "macos")]
fn chart() -> Result<(), Box<dyn std::error::Error>> {
    use vnilink::YuvMatrix;
    use vnilink_media::{EncodeParams, RawFrame, VideoEncoder};

    const BLOCKS: [[u8; 3]; 16] = [
        [200, 120, 40],
        [40, 120, 200],
        [30, 200, 60],
        [220, 60, 160],
        [255, 0, 0],
        [0, 255, 0],
        [0, 0, 255],
        [255, 255, 0],
        [0, 255, 255],
        [255, 0, 255],
        [128, 128, 128],
        [235, 235, 235],
        [20, 20, 20],
        [90, 160, 220],
        [180, 90, 30],
        [60, 60, 140],
    ];
    let mut pixels = Vec::with_capacity(256 * 256 * 4);
    for y in 0..256 {
        for x in 0..256 {
            let [r, g, b] = BLOCKS[(y / 64) * 4 + x / 64];
            pixels.extend_from_slice(&[b, g, r, 255]);
        }
    }
    let frame = RawFrame::bgra(256, 256, 0, pixels)?;
    let params = EncodeParams {
        force_keyframe: true,
        recovery: false,
        bitrate_bps: 8_000_000,
        frame_rate: 30,
    };
    println!("blocks {BLOCKS:?}");
    for matrix in [YuvMatrix::Bt709, YuvMatrix::Bt601] {
        for (label, wire) in [("h264", 2u8), ("h265", 3)] {
            let sample =
                vnilink_macos::MacVideoEncoder::open(256, 256, 30, 8_000_000, wire, matrix)?
                    .encode(&frame, &params)?;
            let hex: String = sample
                .bytes()
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect();
            println!("{matrix:?} {label} {hex}");
        }
    }
    Ok(())
}

#[cfg(not(target_os = "macos"))]
fn main() {}
