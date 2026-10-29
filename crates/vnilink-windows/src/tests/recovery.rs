//! Recovery keyframes and still-screen refinement on each of Media
//! Foundation's H.264 encoder transforms on this machine, the twin of the
//! VideoToolbox measurements. Windows only; run with
//! `cargo test -p vnilink-windows --release --lib recovery_tests -- --ignored --nocapture`.

use vnilink::EncodedFrame;
use vnilink_media::measure::{psnr, text_page};
use vnilink_media::{EncodeParams, RecoveryQp, VideoDecoder, VideoEncoder};

use crate::decode::WindowsDecoder;
use crate::encode::WindowsEncoder;
use crate::transform::{h264_encoders, H264Transform};

/// Each transform with the encoder it opened, or a line on why it did not.
fn each_transform(mut measure: impl FnMut(&H264Transform)) {
    for candidate in unsafe { h264_encoders() } {
        println!(
            "== {} ({:?}, {})",
            candidate.name,
            candidate.vendor,
            if candidate.hardware() {
                "hardware"
            } else {
                "software"
            }
        );
        measure(&candidate);
    }
}

fn open(
    candidate: &H264Transform,
    width: u16,
    height: u16,
    bitrate_bps: u32,
) -> Option<WindowsEncoder> {
    WindowsEncoder::open_candidate(
        candidate,
        width,
        height,
        30,
        bitrate_bps,
        vnilink::YuvMatrix::Bt709,
    )
    .inspect_err(|err| println!("{width}x{height}: did not open: {err}"))
    .ok()
}

/// Sizes of the forced keyframes in a stream asking for one every `every`
/// pictures after the first, how many pictures the transform did not
/// return, and how many of those it returned the decoder gave no picture
/// for (Media Foundation's decoder may hold one back).
fn forced_keyframes(
    candidate: &H264Transform,
    width: u16,
    height: u16,
    bitrate_bps: u32,
    recovery: bool,
    every: u64,
    count: u64,
) -> Option<(Vec<usize>, u64, u64)> {
    let mut encoder = open(candidate, width, height, bitrate_bps)?;
    let mut decoder = WindowsDecoder::new();
    let (mut sizes, mut dropped, mut undecoded, mut owed) = (Vec::new(), 0, 0, false);
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
                undecoded += u64::from(decoder.decode(&frame).is_err());
            }
            Err(_) => {
                owed = force;
                dropped += 1;
            }
        }
    }
    Some((sizes, dropped, undecoded))
}

#[test]
#[ignore = "measurement"]
fn recovery_keyframe_sizes() {
    each_transform(recovery_keyframe_sizes_of);
}

fn recovery_keyframe_sizes_of(candidate: &H264Transform) {
    use windows::Win32::Media::MediaFoundation::{
        CODECAPI_AVEncCommonBufferSize, CODECAPI_AVEncCommonMeanBitRate,
        CODECAPI_AVEncCommonRateControlMode, CODECAPI_AVEncVideoMaxQP, CODECAPI_AVEncVideoMinQP,
        CODECAPI_AVScenarioInfo,
    };
    if let Some(encoder) = open(candidate, 1280, 720, 2_000_000) {
        let settings = [
            ("rate control mode", CODECAPI_AVEncCommonRateControlMode),
            ("mean bitrate", CODECAPI_AVEncCommonMeanBitRate),
            ("buffer size", CODECAPI_AVEncCommonBufferSize),
            ("min QP", CODECAPI_AVEncVideoMinQP),
            ("max QP", CODECAPI_AVEncVideoMaxQP),
            ("scenario", CODECAPI_AVScenarioInfo),
        ];
        let taken: Vec<_> = settings
            .iter()
            .map(|(name, key)| format!("{name} {}", encoder.supports(key)))
            .collect();
        println!("ICodecAPI: {}", taken.join(", "));
    }
    for (width, height) in [(1280, 720), (1920, 1080)] {
        for bitrate_bps in [2_000_000, 8_000_000] {
            let budget = RecoveryQp::budget(&EncodeParams {
                force_keyframe: true,
                recovery: true,
                bitrate_bps,
                frame_rate: 30,
            });
            for recovery in [false, true] {
                let Some((sizes, dropped, undecoded)) =
                    forced_keyframes(candidate, width, height, bitrate_bps, recovery, 30, 8)
                else {
                    return;
                };
                println!(
                    "{width}x{height} {} kb/s {}: budget {budget} B, forced keyframes {sizes:?} B, pictures not returned {dropped}, undecoded {undecoded}",
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
    each_transform(still_screen_refinement_of);
}

fn still_screen_refinement_of(candidate: &H264Transform) {
    for recovery in [false, true] {
        let Some(mut encoder) = open(candidate, 1280, 720, 2_000_000) else {
            return;
        };
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

/// Which `ICodecAPI` change around one forced keyframe shrinks it: forced
/// keyframe sizes at 720p 2 Mb/s, each change applied before the keyframe
/// and undone after it.
#[test]
#[ignore = "measurement"]
fn keyframe_caps() {
    use windows::Win32::Media::MediaFoundation::{
        CODECAPI_AVEncCommonBufferSize, CODECAPI_AVEncCommonMeanBitRate, CODECAPI_AVEncVideoMaxQP,
        CODECAPI_AVEncVideoMinQP,
    };
    let bitrate_bps = 2_000_000u32;
    let frame = bitrate_bps / 30;
    // What the encoder opens with, restored after each change.
    let buffer = vnilink::picture_buffer_bits(bitrate_bps);
    type Change = (&'static str, Vec<(windows::core::GUID, u32, u32)>);
    use windows::Win32::Media::MediaFoundation::CODECAPI_AVEncVideoEncodeQP;
    let changes: Vec<Change> = vec![
        ("none", vec![]),
        (
            "min QP 36 + same bitrate",
            vec![
                (CODECAPI_AVEncVideoMinQP, 36, 0),
                (CODECAPI_AVEncCommonMeanBitRate, bitrate_bps, bitrate_bps),
            ],
        ),
        (
            "min QP 40 + same bitrate",
            vec![
                (CODECAPI_AVEncVideoMinQP, 40, 0),
                (CODECAPI_AVEncCommonMeanBitRate, bitrate_bps, bitrate_bps),
            ],
        ),
        (
            "min QP 46 + same bitrate",
            vec![
                (CODECAPI_AVEncVideoMinQP, 46, 0),
                (CODECAPI_AVEncCommonMeanBitRate, bitrate_bps, bitrate_bps),
            ],
        ),
        ("encode QP 40", vec![(CODECAPI_AVEncVideoEncodeQP, 40, 26)]),
        ("min QP 40", vec![(CODECAPI_AVEncVideoMinQP, 40, 0)]),
        ("min QP 46", vec![(CODECAPI_AVEncVideoMinQP, 46, 0)]),
        (
            "min QP 40, max QP 51",
            vec![
                (CODECAPI_AVEncVideoMinQP, 40, 0),
                (CODECAPI_AVEncVideoMaxQP, 51, 51),
            ],
        ),
        (
            "buffer 1/4 picture",
            vec![(CODECAPI_AVEncCommonBufferSize, frame / 4, buffer)],
        ),
        (
            "bitrate 1/4",
            vec![
                (
                    CODECAPI_AVEncCommonMeanBitRate,
                    bitrate_bps / 4,
                    bitrate_bps,
                ),
                (CODECAPI_AVEncCommonBufferSize, frame / 4, buffer),
            ],
        ),
        (
            "bitrate 1/8",
            vec![
                (
                    CODECAPI_AVEncCommonMeanBitRate,
                    bitrate_bps / 8,
                    bitrate_bps,
                ),
                (CODECAPI_AVEncCommonBufferSize, frame / 8, buffer),
            ],
        ),
    ];
    each_transform(|candidate| {
        for (name, change) in &changes {
            let Some(mut encoder) = open(candidate, 1280, 720, bitrate_bps) else {
                return;
            };
            let (mut sizes, mut after) = (Vec::new(), Vec::new());
            let mut taken = true;
            for index in 0..30 * 5u64 {
                let force = index % 30 == 0;
                if force && index > 0 {
                    for (key, value, _) in change {
                        taken &= encoder.set_codec(key, *value);
                    }
                }
                let params = EncodeParams {
                    force_keyframe: force,
                    recovery: false,
                    bitrate_bps,
                    frame_rate: 30,
                };
                let sample = encoder.encode(&text_page(1280, 720, index), &params);
                if force && index > 0 {
                    for (key, _, undo) in change {
                        encoder.set_codec(key, *undo);
                    }
                    sizes.push(sample.map_or(0, |sample| sample.bytes().len()));
                } else if index > 30 && index % 30 <= 3 {
                    after.push(sample.map_or(0, |sample| sample.bytes().len()));
                }
            }
            println!("{name}: taken {taken}, forced keyframes {sizes:?} B, next three {after:?} B");
        }
    });
}

/// Forced keyframe sizes under a mean bitrate cut to `1/divisor` for that
/// picture, the two pictures after it, unforced keyframes in the stream,
/// and the bytes of a second of pictures after a retune to half the rate.
#[test]
#[ignore = "measurement"]
fn bitrate_cut_and_retune() {
    use windows::Win32::Media::MediaFoundation::{
        CODECAPI_AVEncCommonBufferSize, CODECAPI_AVEncCommonMeanBitRate,
    };
    each_transform(|candidate| {
        for (width, height) in [(1280u16, 720u16), (1920, 1080)] {
            for bitrate_bps in [2_000_000u32, 8_000_000] {
                let budget = RecoveryQp::budget(&EncodeParams {
                    force_keyframe: true,
                    recovery: true,
                    bitrate_bps,
                    frame_rate: 30,
                });
                for divisor in [1u32, 2, 3, 4, 6, 8, 16] {
                    let Some(mut encoder) = open(candidate, width, height, bitrate_bps) else {
                        return;
                    };
                    let (mut sizes, mut after, mut unforced) = (Vec::new(), Vec::new(), 0);
                    for index in 0..30 * 5u64 {
                        let force = index % 30 == 0;
                        let cut = force && index > 0 && divisor > 1;
                        if cut {
                            encoder
                                .set_codec(&CODECAPI_AVEncCommonMeanBitRate, bitrate_bps / divisor);
                            encoder.set_codec(
                                &CODECAPI_AVEncCommonBufferSize,
                                bitrate_bps / 30 / divisor,
                            );
                        }
                        let params = EncodeParams {
                            force_keyframe: force,
                            recovery: false,
                            bitrate_bps,
                            frame_rate: 30,
                        };
                        let sample = encoder
                            .encode(&text_page(width, height, index), &params)
                            .ok();
                        if cut {
                            encoder.set_codec(&CODECAPI_AVEncCommonMeanBitRate, bitrate_bps);
                            encoder.set_codec(&CODECAPI_AVEncCommonBufferSize, bitrate_bps / 30);
                        }
                        let bytes = sample.as_ref().map_or(0, |sample| sample.bytes().len());
                        unforced +=
                            u32::from(!force && sample.as_ref().is_some_and(|s| s.keyframe()));
                        if force && index > 0 {
                            sizes.push(bytes);
                        } else if index > 30 && index % 30 <= 2 {
                            after.push(bytes);
                        }
                    }
                    println!(
                        "{width}x{height} {} kb/s 1/{divisor}: budget {budget} B, forced keyframes {sizes:?} B, next two {after:?} B, unforced keyframes {unforced}",
                        bitrate_bps / 1_000
                    );
                }
            }
        }
        // A retune to half the rate: bytes of the second before and after.
        let Some(mut encoder) = open(candidate, 1280, 720, 4_000_000) else {
            return;
        };
        let mut seconds = [0usize; 4];
        for index in 0..120u64 {
            if index == 60 {
                let taken = encoder.retune(2_000_000, 30);
                println!("retune to 2 Mb/s: {taken:?}");
            }
            let bitrate_bps = if index < 60 { 4_000_000 } else { 2_000_000 };
            let params = EncodeParams {
                force_keyframe: index == 0,
                recovery: false,
                bitrate_bps,
                frame_rate: 30,
            };
            if let Ok(sample) = encoder.encode(&text_page(1280, 720, index), &params) {
                seconds[index as usize / 30] += sample.bytes().len();
            }
        }
        println!("720p 4 then 2 Mb/s, bytes per second: {seconds:?}");
    });
}
