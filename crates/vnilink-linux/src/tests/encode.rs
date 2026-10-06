use vnilink_media::{parse_annex_b, Error, VideoEncoder};

use crate::encode::LinuxEncoder;

#[test]
fn an_odd_encoder_size_is_rejected() {
    assert!(matches!(
        LinuxEncoder::open(63, 64, 30, 500_000, false),
        Err(Error::InvalidFrame("H.264 width and height must be even"))
    ));
    assert!(matches!(
        LinuxEncoder::open(64, 63, 30, 500_000, true),
        Err(Error::InvalidFrame("H.264 width and height must be even"))
    ));
}

#[cfg(not(target_os = "linux"))]
#[test]
fn the_software_encoder_is_not_linked() {
    assert!(matches!(
        LinuxEncoder::open(64, 64, 30, 500_000, false),
        Err(Error::Unsupported("VA-API H.264"))
    ));
    assert!(!crate::h264_encode_available());
    assert!(!crate::h264_high_available());
    assert!(!crate::h264_decode_available());
}

#[cfg(target_os = "linux")]
#[test]
fn vaapi_encodes_a_synthetic_picture_when_a_render_node_is_open() {
    let mut encoder = match LinuxEncoder::open(64, 64, 30, 500_000, false) {
        Ok(encoder) => encoder,
        Err(Error::Unsupported("VA-API H.264")) => return,
        Err(err) => panic!("VA-API failed to open: {err}"),
    };
    let params = vnilink_media::EncodeParams {
        force_keyframe: true,
        bitrate_bps: 500_000,
        frame_rate: 30,
    };
    let sample = encoder
        .encode(&solid(64, 64, 0, 0, 0, 255), &params)
        .unwrap();
    assert!(sample.keyframe());
    let parsed = parse_annex_b(sample.bytes()).unwrap();
    assert!(parsed.has_sps() && parsed.has_pps() && parsed.has_idr());

    let delta = encoder
        .encode(
            &solid(64, 64, 33_333, 0, 0, 255),
            &vnilink_media::EncodeParams {
                force_keyframe: false,
                bitrate_bps: 500_000,
                frame_rate: 30,
            },
        )
        .unwrap();
    let mut stream = sample.bytes().to_vec();
    stream.extend_from_slice(delta.bytes());
    assert_red_picture(&stream, 64, 64);

    let quality = vnilink_media::Quality::new(500, 30, 64, 64).unwrap();
    let mut video = vnilink_media::HostVideo::new(quality, encoder);
    let output = video.push(&solid(64, 64, 66_666, 0, 255, 0)).unwrap();
    assert!(matches!(output, vnilink_media::PumpOutput::Encoded(_)));
}

#[cfg(target_os = "linux")]
fn solid(
    width: u16,
    height: u16,
    timestamp_us: u64,
    b: u8,
    g: u8,
    r: u8,
) -> vnilink_media::RawFrame {
    let mut pixels = Vec::with_capacity(usize::from(width) * usize::from(height) * 4);
    for _ in 0..usize::from(width) * usize::from(height) {
        pixels.extend_from_slice(&[b, g, r, 255]);
    }
    vnilink_media::RawFrame::bgra(width, height, timestamp_us, pixels).unwrap()
}

#[cfg(target_os = "linux")]
fn assert_red_picture(annex_b: &[u8], width: u16, height: u16) {
    let path = std::env::temp_dir().join(format!("vnilink-h264-{}.h264", std::process::id()));
    std::fs::write(&path, annex_b).unwrap();
    let output = match std::process::Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-f", "h264", "-i"])
        .arg(&path)
        .args(["-frames:v", "1", "-f", "rawvideo", "-pix_fmt", "bgra", "-"])
        .output()
    {
        Ok(output) => output,
        Err(_) => {
            let _ = std::fs::remove_file(&path);
            return;
        }
    };
    let _ = std::fs::remove_file(&path);
    assert!(
        output.status.success(),
        "ffmpeg rejected the bitstream: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let row = usize::from(width) * 4;
    let needed = row * usize::from(height);
    assert!(output.stdout.len() >= needed);
    let pixel = &output.stdout[32 * row + 32 * 4..32 * row + 32 * 4 + 4];
    assert!(
        pixel[2] > pixel[0] && pixel[2] > pixel[1] && pixel[2] > 128,
        "decoded picture is not the encoded color"
    );
}

#[cfg(target_os = "linux")]
#[test]
fn higher_bitrate_changes_the_hardware_output_budget() {
    fn encoded_bytes(bitrate: u32) -> Option<usize> {
        let (width, height) = (1280u16, 720u16);
        let mut encoder = match LinuxEncoder::open(width, height, 60, bitrate, true) {
            Ok(encoder) => encoder,
            Err(Error::Unsupported("VA-API H.264")) => return None,
            Err(error) => panic!("encoder failed: {error}"),
        };
        let mut total = 0;
        for index in 0..30u64 {
            let mut pixels = vec![255; usize::from(width) * usize::from(height) * 4];
            for y in 0..usize::from(height) {
                for x in 0..usize::from(width) {
                    let value = if ((x + index as usize) % 19 < 2 && y % 23 < 17)
                        || (y % 23 < 2 && x % 19 < 13)
                    {
                        32
                    } else {
                        224
                    };
                    let offset = (y * usize::from(width) + x) * 4;
                    pixels[offset..offset + 3].fill(value);
                }
            }
            let frame =
                vnilink_media::RawFrame::bgra(width, height, index * 16_667, pixels).unwrap();
            let sample = encoder
                .encode(
                    &frame,
                    &vnilink_media::EncodeParams {
                        force_keyframe: index == 0,
                        bitrate_bps: bitrate,
                        frame_rate: 60,
                    },
                )
                .unwrap();
            total += sample.bytes().len();
        }
        Some(total)
    }
    let Some(low) = encoded_bytes(300_000) else {
        return;
    };
    let high = encoded_bytes(12_000_000).unwrap();
    assert!(
        high > low,
        "bitrate control had no effect on detailed pictures: low={low}, high={high}"
    );
}
