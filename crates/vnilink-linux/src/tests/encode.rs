use vnilink_media::{parse_annex_b, Error, VideoEncoder};

use crate::encode::LinuxEncoder;

#[test]
fn an_odd_encoder_size_is_rejected() {
    assert!(matches!(
        LinuxEncoder::open(63, 64, 30, 500_000),
        Err(Error::InvalidFrame("H.264 width and height must be even"))
    ));
}

#[cfg(not(target_os = "linux"))]
#[test]
fn the_software_encoder_is_not_linked() {
    assert!(matches!(
        LinuxEncoder::open(64, 64, 30, 500_000),
        Err(Error::Unsupported("VA-API H.264"))
    ));
}

#[cfg(target_os = "linux")]
#[test]
fn vaapi_encodes_a_synthetic_picture_when_a_render_node_is_open() {
    let mut encoder = match LinuxEncoder::open(64, 64, 30, 500_000) {
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
