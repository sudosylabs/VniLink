// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

use vnilink_media::{parse_annex_b, Error, VideoEncoder};

use crate::encode::LinuxEncoder;

#[test]
fn an_odd_encoder_size_is_rejected() {
    assert!(matches!(
        LinuxEncoder::open(
            vnilink::VideoCodec::H264,
            63,
            64,
            30,
            500_000,
            false,
            vnilink::YuvMatrix::Bt709
        ),
        Err(Error::InvalidFrame("width and height must be even"))
    ));
    assert!(matches!(
        LinuxEncoder::open(
            vnilink::VideoCodec::H264,
            64,
            63,
            30,
            500_000,
            true,
            vnilink::YuvMatrix::Bt709
        ),
        Err(Error::InvalidFrame("width and height must be even"))
    ));
}

#[cfg(not(target_os = "linux"))]
#[test]
fn the_software_encoder_is_not_linked() {
    assert!(matches!(
        LinuxEncoder::open(
            vnilink::VideoCodec::H264,
            64,
            64,
            30,
            500_000,
            false,
            vnilink::YuvMatrix::Bt709
        ),
        Err(Error::Unsupported("VA-API"))
    ));
    let caps = crate::video_codec_caps();
    assert!(!caps.can_encode() && !caps.can_decode());
}

#[cfg(target_os = "linux")]
#[test]
fn vaapi_encodes_a_synthetic_picture_when_a_render_node_is_open() {
    let mut encoder = match LinuxEncoder::open(
        vnilink::VideoCodec::H264,
        64,
        64,
        30,
        500_000,
        false,
        vnilink::YuvMatrix::Bt709,
    ) {
        Ok(encoder) => encoder,
        Err(Error::Unsupported(why)) => {
            vnilink_media::needs::needs("a VA-API encoder", Err(why.to_owned()));
            return;
        }
        Err(err) => panic!("VA-API failed to open: {err}"),
    };
    let params = vnilink_media::EncodeParams {
        force_keyframe: true,
        recovery: false,
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
                recovery: false,
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
        Err(error) => {
            let _ = std::fs::remove_file(&path);
            vnilink_media::needs::needs("ffmpeg", Err(error.to_string()));
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
        let mut encoder = match LinuxEncoder::open(
            vnilink::VideoCodec::H264,
            width,
            height,
            60,
            bitrate,
            true,
            vnilink::YuvMatrix::Bt709,
        ) {
            Ok(encoder) => encoder,
            Err(Error::Unsupported(why)) => {
                vnilink_media::needs::needs("a VA-API encoder", Err(why.to_owned()));
                return None;
            }
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
                        recovery: false,
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

/// Which GPU this machine's sessions encode on, for confirming
/// [`crate::gpu`]'s rule on other hardware. Linux only; run with
/// `cargo test -p vnilink-linux --lib encoder_choice_on_this_machine -- --ignored --nocapture`.
#[cfg(target_os = "linux")]
#[test]
#[ignore = "measurement"]
fn encoder_choice_on_this_machine() {
    let gpus = crate::gpu::inventory();
    for gpu in &gpus {
        println!("{gpu:?}");
    }
    println!(
        "compositing: {:?}",
        crate::gpu::compositing(&gpus).map(|gpu| &gpu.render_node)
    );
    for choice in crate::gpu::encoders(&gpus) {
        println!("candidate: {choice:?}");
    }
    for codec in [
        vnilink::VideoCodec::H264,
        vnilink::VideoCodec::H265,
        vnilink::VideoCodec::Av1,
    ] {
        match LinuxEncoder::open(
            codec,
            1280,
            720,
            30,
            4_000_000,
            false,
            vnilink::YuvMatrix::Bt709,
        ) {
            Ok(encoder) => println!("{codec:?}: {}", encoder.implementation().name),
            Err(error) => println!("{codec:?}: no hardware encoder ({error})"),
        }
    }
    println!("caps: {:?}", crate::video_codec_caps());
}
