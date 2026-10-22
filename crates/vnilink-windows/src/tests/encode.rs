//! Which H.264 transform the desktop encoder opens. Windows only.

use vnilink::EncodedFrame;
use vnilink_media::measure::text_page;
use vnilink_media::{EncodeParams, VideoDecoder, VideoEncoder};

use crate::decode::WindowsDecoder;
use crate::encode::WindowsEncoder;
use crate::transform::h264_encoders;

/// A hardware transform that opens is preferred to the software one, which
/// is listed last so a machine without hardware still encodes, and the
/// probe says which kind a peer will get.
#[test]
fn prefers_a_hardware_transform_that_opens() {
    let encoders = unsafe { h264_encoders() };
    assert!(!encoders.last().unwrap().hardware());
    let hardware_opens = encoders
        .iter()
        .filter(|candidate| candidate.hardware())
        .any(|candidate| {
            WindowsEncoder::open_candidate(candidate, 1280, 720, 30, 4_000_000).is_ok()
        });
    let encoder = WindowsEncoder::open(1280, 720, 30, 4_000_000).unwrap();
    assert_eq!(encoder.hardware(), hardware_opens);
    assert_eq!(crate::probe_video_codecs().hardware, hardware_opens);
}

/// `profile_idc` of the SPS in an Annex-B keyframe.
fn sps_profile(annex_b: &[u8]) -> Option<u8> {
    annex_b
        .windows(5)
        .find(|bytes| bytes[..3] == [0, 0, 1] && bytes[3] & 0x1f == 7)
        .map(|bytes| bytes[4])
}

/// Every transform that opens, hardware or not, returns a picture for each
/// picture it is given, a forced keyframe as a keyframe; a hardware one in
/// Baseline. Microsoft's software transform writes Main on the desktop path
/// (measured 2026-10-22), which is left as it was.
#[test]
fn every_transform_encodes_each_picture() {
    for candidate in unsafe { h264_encoders() } {
        let Ok(mut encoder) = WindowsEncoder::open_candidate(&candidate, 1280, 720, 30, 4_000_000)
        else {
            continue;
        };
        let mut decoder = WindowsDecoder::new();
        for index in 0..20u64 {
            let force = index == 0 || index == 10;
            let params = EncodeParams {
                force_keyframe: force,
                recovery: index == 10,
                bitrate_bps: 4_000_000,
                frame_rate: 30,
            };
            let sample = encoder
                .encode(&text_page(1280, 720, index), &params)
                .unwrap_or_else(|err| panic!("{}: picture {index}: {err}", candidate.name));
            assert!(
                !force || sample.keyframe(),
                "{}: picture {index}",
                candidate.name
            );
            if force && candidate.hardware() {
                assert_eq!(sps_profile(sample.bytes()), Some(66), "{}", candidate.name);
            }
            let frame =
                EncodedFrame::h264(sample.keyframe(), index * 33_333, sample.bytes().to_vec())
                    .unwrap();
            // Media Foundation's decoder may hold the first picture back.
            let _ = decoder.decode(&frame);
        }
    }
}

/// Every transform's stream says BT.709 limited range, and pictures
/// converted with that matrix come back through Media Foundation's decoder
/// in the colours they went in with.
#[test]
fn every_transform_keeps_the_stream_colour() {
    // (b, g, r)
    let colours = [
        [0u8, 0, 255],
        [0, 255, 0],
        [255, 0, 0],
        [0, 255, 255],
        [255, 0, 255],
        [0, 128, 255],
        [128, 128, 128],
    ];
    for candidate in unsafe { h264_encoders() } {
        for colour in colours {
            let Ok(mut encoder) =
                WindowsEncoder::open_candidate(&candidate, 640, 480, 30, 8_000_000)
            else {
                break;
            };
            let mut decoder = WindowsDecoder::new();
            let mut pixels = Vec::with_capacity(640 * 480 * 4);
            for _ in 0..640 * 480 {
                pixels.extend_from_slice(&[colour[0], colour[1], colour[2], 255]);
            }
            let mut shown = None;
            // Media Foundation's decoder may hold a picture back: give it a few.
            for index in 0..6u64 {
                let picture =
                    vnilink_media::RawFrame::bgra(640, 480, index * 33_333, pixels.clone())
                        .unwrap();
                let params = EncodeParams {
                    force_keyframe: index == 0,
                    recovery: false,
                    bitrate_bps: 8_000_000,
                    frame_rate: 30,
                };
                let sample = encoder.encode(&picture, &params).unwrap();
                if index == 0 {
                    let parsed = vnilink_media::parse_annex_b(sample.bytes()).unwrap();
                    let sps = parsed
                        .units()
                        .iter()
                        .find(|unit| unit.nal_type == 7)
                        .unwrap();
                    assert_eq!(
                        vnilink_media::h264_video_signal(sps.bytes()).unwrap(),
                        Some(vnilink_media::STREAM_SIGNAL),
                        "{}",
                        candidate.name
                    );
                }
                let frame =
                    EncodedFrame::h264(sample.keyframe(), index * 33_333, sample.bytes().to_vec())
                        .unwrap();
                if let Ok(decoded) = decoder.decode(&frame) {
                    shown = Some(decoded);
                }
            }
            let decoded = shown.unwrap_or_else(|| panic!("{}: no picture", candidate.name));
            let pixel = &decoded.row(240)[320 * 4..320 * 4 + 3];
            for channel in 0..3 {
                let drift = (i16::from(pixel[channel]) - i16::from(colour[channel])).abs();
                assert!(
                    drift <= 6,
                    "{}: {colour:?} came back as {pixel:?}",
                    candidate.name
                );
            }
        }
    }
}
