use vnilink_media::{parse_annex_b, CapturePlan, Error, Quality};

use crate::bitstream::h264_access_unit;
use crate::plan::{frame_size, WindowsPlan, SECURE_DESKTOP_SUPPORTED};

fn plan() -> CapturePlan {
    let quality = Quality::new(2_500, 30, 1920, 1080).unwrap();
    CapturePlan::one_monitor(quality).unwrap()
}

#[test]
fn the_plan_is_one_monitor_and_not_the_secure_desktop() {
    assert!(!SECURE_DESKTOP_SUPPORTED);
    let windows = WindowsPlan::from_capture(&plan(), 2_500).unwrap();
    assert_eq!(windows.monitor_index(), 0);
    assert_eq!((windows.width(), windows.height()), (1920, 1080));
    assert_eq!(windows.frame_rate(), 30);
    assert_eq!(windows.bitrate_bps(), 2_500_000);
    assert!(!windows.secure_desktop_supported());
}

#[test]
fn a_desktop_larger_than_one_frame_is_rejected() {
    assert!(matches!(
        frame_size(u32::from(u16::MAX) + 1, 1080),
        Err(Error::Unsupported("the desktop is larger than one frame"))
    ));
    assert!(matches!(
        frame_size(1920, 0),
        Err(Error::CaptureRejected(
            "Desktop Duplication returned an empty frame"
        ))
    ));
}

#[test]
fn a_length_prefixed_idr_gains_parameter_sets() {
    let sps = [0x67, 0x42];
    let pps = [0x68, 0xce];
    let header = avc_decoder_config(&sps, &pps);
    let sample = h264_access_unit(&length_prefixed(&[0x65, 0x88]), &header).unwrap();
    assert!(sample.keyframe());
    let parsed = parse_annex_b(sample.bytes()).unwrap();
    let types: Vec<u8> = parsed.units().iter().map(|unit| unit.nal_type).collect();
    assert_eq!(types, vec![7, 8, 5]);
}

#[test]
fn an_annex_b_sequence_header_is_prefixed_onto_an_idr() {
    let header = [0, 0, 0, 1, 0x67, 0x42, 0, 0, 0, 1, 0x68, 0xce];
    let sample = h264_access_unit(&length_prefixed(&[0x65, 0x10]), &header).unwrap();
    assert!(sample.keyframe());
    let parsed = parse_annex_b(sample.bytes()).unwrap();
    assert!(parsed.has_sps() && parsed.has_pps() && parsed.has_idr());
}

#[test]
fn a_predicted_frame_does_not_repeat_parameter_sets() {
    let sample = h264_access_unit(&length_prefixed(&[0x41, 0x9a]), &[]).unwrap();
    assert!(!sample.keyframe());
    let parsed = parse_annex_b(sample.bytes()).unwrap();
    assert_eq!(parsed.units().len(), 1);
    assert_eq!(parsed.units()[0].nal_type, 1);
}

#[test]
fn an_idr_without_parameter_sets_is_rejected() {
    assert!(matches!(
        h264_access_unit(&length_prefixed(&[0x65, 0x88]), &[]),
        Err(Error::EncodeRejected(
            "a keyframe needs SPS, PPS, and an IDR"
        ))
    ));
}

#[test]
fn an_annex_b_keyframe_is_kept() {
    let bytes = [
        0, 0, 0, 1, 0x67, 0x42, 0, 0, 0, 1, 0x68, 0xce, 0, 0, 0, 1, 0x65, 0x88,
    ];
    let sample = h264_access_unit(&bytes, &[]).unwrap();
    assert!(sample.keyframe());
}

fn length_prefixed(nal: &[u8]) -> Vec<u8> {
    let mut out = Vec::from((nal.len() as u32).to_be_bytes());
    out.extend_from_slice(nal);
    out
}

fn avc_decoder_config(sps: &[u8], pps: &[u8]) -> Vec<u8> {
    let mut out = vec![1, 0x42, 0, 0x1e, 0xff, 0xe1];
    out.extend_from_slice(&(sps.len() as u16).to_be_bytes());
    out.extend_from_slice(sps);
    out.push(1);
    out.extend_from_slice(&(pps.len() as u16).to_be_bytes());
    out.extend_from_slice(pps);
    out
}

#[test]
fn an_absurd_bitrate_is_rejected() {
    assert!(matches!(
        WindowsPlan::from_capture(&plan(), u32::MAX),
        Err(Error::InvalidSetting(
            "bitrate does not fit in bits per second"
        ))
    ));
}
