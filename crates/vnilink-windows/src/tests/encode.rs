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
