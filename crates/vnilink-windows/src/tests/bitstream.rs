use vnilink_media::{parse_annex_b, Error};

use crate::bitstream::h264_access_unit;

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
