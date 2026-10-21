//! Recovery keyframes and still-screen refinement on Media Foundation's
//! H.264 transform, the twin of the VideoToolbox measurements. Windows only;
//! run with
//! `cargo test -p vnilink-windows --release --lib recovery_tests -- --ignored --nocapture`.

use vnilink::EncodedFrame;
use vnilink_media::measure::{psnr, text_page};
use vnilink_media::{EncodeParams, RecoveryQp, VideoDecoder, VideoEncoder};

use crate::decode::WindowsDecoder;
use crate::encode::WindowsEncoder;

/// Sizes of the forced keyframes in a stream asking for one every `every`
/// pictures after the first, and how many pictures the transform did not
/// return. Every picture it returned is decoded.
fn forced_keyframes(
    width: u16,
    height: u16,
    bitrate_bps: u32,
    recovery: bool,
    every: u64,
    count: u64,
) -> (Vec<usize>, u64) {
    let mut encoder = WindowsEncoder::open(width, height, 30, bitrate_bps).unwrap();
    let mut decoder = WindowsDecoder::new();
    let (mut sizes, mut dropped, mut owed) = (Vec::new(), 0, false);
    for index in 0..every * (count + 1) {
        let force = owed || index % every == 0;
        let params = EncodeParams {
            force_keyframe: force,
            recovery: recovery && force && index > 0,
            bitrate_bps,
            frame_rate: 30,
        };
        match encoder.encode(&text_page(width, height, index), &params) {
            Ok(sample) => {
                owed = false;
                if force && index > 0 {
                    assert!(sample.keyframe(), "picture {index} was not a keyframe");
                    sizes.push(sample.bytes().len());
                }
                let frame =
                    EncodedFrame::h264(sample.keyframe(), index * 33_333, sample.bytes().to_vec())
                        .unwrap();
                decoder.decode(&frame).unwrap();
            }
            Err(_) => {
                owed = force;
                dropped += 1;
            }
        }
    }
    (sizes, dropped)
}

#[test]
#[ignore = "measurement"]
fn recovery_keyframe_sizes() {
    for (width, height) in [(1280, 720), (1920, 1080)] {
        for bitrate_bps in [2_000_000, 8_000_000] {
            let budget = RecoveryQp::budget(&EncodeParams {
                force_keyframe: true,
                recovery: true,
                bitrate_bps,
                frame_rate: 30,
            });
            for recovery in [false, true] {
                let (sizes, dropped) =
                    forced_keyframes(width, height, bitrate_bps, recovery, 30, 8);
                println!(
                    "{width}x{height} {} kb/s {}: budget {budget} B, forced keyframes {sizes:?} B, pictures not returned {dropped}",
                    bitrate_bps / 1_000,
                    if recovery { "recovery" } else { "full" },
                );
            }
        }
    }
}

/// Whether the transform refines a still screen after a full or a recovery
/// keyframe: PSNR of each decoded picture against the page.
#[test]
#[ignore = "measurement"]
fn still_screen_refinement() {
    for recovery in [false, true] {
        let mut encoder = WindowsEncoder::open(1280, 720, 30, 2_000_000).unwrap();
        let mut decoder = WindowsDecoder::new();
        let still = text_page(1280, 720, 0);
        let mut line = Vec::new();
        for index in 0..91u64 {
            let force = index == 0 || index == 30;
            let params = EncodeParams {
                force_keyframe: force,
                recovery: recovery && index == 30,
                bitrate_bps: 2_000_000,
                frame_rate: 30,
            };
            let frame =
                vnilink_media::RawFrame::bgra(1280, 720, index * 33_333, still.pixels().to_vec())
                    .unwrap();
            let Ok(sample) = encoder.encode(&frame, &params) else {
                line.push(format!("{index}:not returned"));
                continue;
            };
            let encoded =
                EncodedFrame::h264(sample.keyframe(), index * 33_333, sample.bytes().to_vec())
                    .unwrap();
            let Ok(picture) = decoder.decode(&encoded) else {
                line.push(format!("{index}:undecoded"));
                continue;
            };
            if index >= 30 && (index - 30) % 10 == 0 || index == 31 {
                line.push(format!(
                    "+{}: {} B {:.1} dB",
                    index - 30,
                    sample.bytes().len(),
                    psnr(&still, &picture)
                ));
            }
        }
        println!(
            "{}: {}",
            if recovery { "recovery" } else { "full" },
            line.join(", ")
        );
    }
}
