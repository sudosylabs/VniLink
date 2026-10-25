//! The reference pictures of `src/workbench/services/camera/decoderColour.ts`:
//! one flat 256x256 keyframe per codec family and matrix, BGR 200, 120, 40
//! converted and declared in that matrix by VniLink's own encoders
//! (VideoToolbox for H.264, the software encoders for the others), as lines of
//! codec label and hex, which the TypeScript source holds as base64. Synthetic; never captures a screen.
#[cfg(target_os = "macos")]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    use vnilink::{VideoCodec, YuvMatrix};
    use vnilink_media::{EncodeParams, RawFrame, SoftwareEncoder, VideoEncoder};

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

#[cfg(not(target_os = "macos"))]
fn main() {}
