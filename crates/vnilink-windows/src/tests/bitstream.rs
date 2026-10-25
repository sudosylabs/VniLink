use vnilink_media::{parse_annex_b, Error};

use crate::bitstream::{h264_access_unit, picture_size};

#[test]
fn a_length_prefixed_idr_gains_parameter_sets() {
    let sps = [0x67, 0x42, 0x00, 0x1e, 0xda, 0x10, 0x99];
    let pps = [0x68, 0xce];
    let header = avc_decoder_config(&sps, &pps);
    let sample = h264_access_unit(
        &length_prefixed(&[0x65, 0x88]),
        &header,
        vnilink::YuvMatrix::Bt709,
    )
    .unwrap();
    assert!(sample.keyframe());
    let parsed = parse_annex_b(sample.bytes()).unwrap();
    let types: Vec<u8> = parsed.units().iter().map(|unit| unit.nal_type).collect();
    assert_eq!(types, vec![7, 8, 5]);
}

#[test]
fn an_annex_b_sequence_header_is_prefixed_onto_an_idr() {
    let header = [
        0, 0, 0, 1, 0x67, 0x42, 0x00, 0x1e, 0xda, 0x10, 0x99, 0, 0, 0, 1, 0x68, 0xce,
    ];
    let sample = h264_access_unit(
        &length_prefixed(&[0x65, 0x10]),
        &header,
        vnilink::YuvMatrix::Bt709,
    )
    .unwrap();
    assert!(sample.keyframe());
    let parsed = parse_annex_b(sample.bytes()).unwrap();
    assert!(parsed.has_sps() && parsed.has_pps() && parsed.has_idr());
}

#[test]
fn a_predicted_frame_does_not_repeat_parameter_sets() {
    let sample = h264_access_unit(
        &length_prefixed(&[0x41, 0x9a]),
        &[],
        vnilink::YuvMatrix::Bt709,
    )
    .unwrap();
    assert!(!sample.keyframe());
    let parsed = parse_annex_b(sample.bytes()).unwrap();
    assert_eq!(parsed.units().len(), 1);
    assert_eq!(parsed.units()[0].nal_type, 1);
}

#[test]
fn an_idr_without_parameter_sets_is_rejected() {
    assert!(matches!(
        h264_access_unit(
            &length_prefixed(&[0x65, 0x88]),
            &[],
            vnilink::YuvMatrix::Bt709
        ),
        Err(Error::EncodeRejected(
            "a keyframe needs SPS, PPS, and an IDR"
        ))
    ));
}

#[test]
fn an_annex_b_keyframe_is_kept() {
    let bytes = [
        0, 0, 0, 1, 0x67, 0x42, 0x00, 0x1e, 0xda, 0x10, 0x99, 0, 0, 0, 1, 0x68, 0xce, 0, 0, 0, 1,
        0x65, 0x88,
    ];
    let sample = h264_access_unit(&bytes, &[], vnilink::YuvMatrix::Bt709).unwrap();
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

/// Sequence parameter sets written by the Linux VA-API encoder writer, read
/// back here. Writer and parser are independent implementations.
#[test]
fn sps_vectors_recover_the_visible_picture_size() {
    // 64x64, no crop.
    let even = [
        0x67, 0x42, 0xc0, 0x0a, 0x96, 0x54, 0x21, 0x34, 0x20, 0x00, 0x00, 0x03, 0x00, 0x20, 0x00,
        0x00, 0x07, 0x91, 0xe1, 0x10, 0x8d, 0x40,
    ];
    assert_eq!(picture_size(&even).unwrap(), (64, 64));
    // 62x30, coded 64x32 with one macroblock cropped right and bottom.
    let cropped = [
        0x67, 0x42, 0xc0, 0x0a, 0x96, 0x54, 0x22, 0xf5, 0x50, 0x80, 0x00, 0x00, 0x03, 0x00, 0x80,
        0x00, 0x00, 0x1e, 0x47, 0x84, 0x42, 0x35,
    ];
    assert_eq!(picture_size(&cropped).unwrap(), (62, 30));
    // 1280x720, no crop.
    let hd = [
        0x67, 0x42, 0xc0, 0x1f, 0x96, 0x54, 0x02, 0x80, 0x2d, 0xd0, 0x80, 0x00, 0x00, 0x03, 0x00,
        0x80, 0x00, 0x00, 0x1e, 0x47, 0x84, 0x42, 0x35,
    ];
    assert_eq!(picture_size(&hd).unwrap(), (1280, 720));
}

#[test]
fn a_truncated_or_foreign_sps_is_refused() {
    let even = [
        0x67, 0x42, 0xc0, 0x0a, 0x96, 0x54, 0x21, 0x34, 0x20, 0x00, 0x00, 0x03, 0x00, 0x20, 0x00,
        0x00, 0x07, 0x91, 0xe1, 0x10, 0x8d, 0x40,
    ];
    assert!(matches!(
        picture_size(&even[..4]),
        Err(Error::InvalidFrame("H.264 SPS is truncated"))
    ));
    assert!(matches!(
        picture_size(&[]),
        Err(Error::InvalidFrame("H.264 SPS is empty"))
    ));
    // NAL type 8 is a PPS, not an SPS.
    let mut pps = even;
    pps[0] = 0x68;
    assert!(matches!(
        picture_size(&pps),
        Err(Error::InvalidFrame("H.264 NAL unit is not an SPS"))
    ));
}

/// A High-profile SPS parses in spec order: the sequence id precedes the
/// chroma block. Reading chroma first consumes the seq id as
/// `chroma_format_idc` and rejects a valid stream.
#[test]
fn a_high_profile_sps_parses_in_spec_order() {
    let mut sps = vec![0x67, 0x64, 0x00, 0x1f];
    let mut bits = BitWriter::default();
    bits.ue(0); // seq_parameter_set_id
    bits.ue(1); // chroma_format_idc = 4:2:0
    bits.ue(0); // bit_depth_luma_minus8
    bits.ue(0); // bit_depth_chroma_minus8
    bits.bit(0); // qpprime_y_zero_transform_bypass_flag
    bits.bit(0); // seq_scaling_matrix_present_flag
    bits.ue(0); // log2_max_frame_num_minus4
    bits.ue(0); // pic_order_cnt_type
    bits.ue(0); // log2_max_pic_order_cnt_lsb_minus4
    bits.ue(1); // max_num_ref_frames
    bits.bit(0); // gaps_in_frame_num_value_allowed_flag
    bits.ue(3); // pic_width_in_mbs_minus1: 64 / 16 - 1
    bits.ue(3); // pic_height_in_map_units_minus1
    bits.bit(1); // frame_mbs_only_flag
    bits.bit(1); // direct_8x8_inference_flag
    bits.bit(0); // frame_cropping_flag
    sps.extend_from_slice(&bits.finish());
    assert_eq!(picture_size(&sps).unwrap(), (64, 64));
}

#[derive(Default)]
struct BitWriter {
    bytes: Vec<u8>,
    pending: u8,
    filled: u8,
}

impl BitWriter {
    fn bit(&mut self, value: u8) {
        self.pending = (self.pending << 1) | (value & 1);
        self.filled += 1;
        if self.filled == 8 {
            self.bytes.push(self.pending);
            self.pending = 0;
            self.filled = 0;
        }
    }

    fn ue(&mut self, value: u32) {
        let code = value + 1;
        let zeros = 31 - code.leading_zeros();
        for _ in 0..zeros {
            self.bit(0);
        }
        for shift in (0..=zeros).rev() {
            self.bit(((code >> shift) & 1) as u8);
        }
    }

    fn finish(mut self) -> Vec<u8> {
        // RBSP stop bit, then align with zeros.
        self.bit(1);
        while self.filled != 0 {
            self.bit(0);
        }
        self.bytes
    }
}

#[test]
fn camera_keyframes_require_actual_constrained_baseline_parameter_sets() {
    use crate::bitstream::camera_access_unit;
    let valid = [
        0, 0, 0, 1, 0x67, 66, 0x40, 31, 0, 0, 0, 1, 0x68, 1, 0, 0, 0, 1, 0x65, 1,
    ];
    camera_access_unit(&valid).unwrap();
    let mut high = valid;
    high[5] = 100;
    assert!(camera_access_unit(&high).is_err());
    let mut unconstrained = valid;
    unconstrained[6] = 0;
    assert!(camera_access_unit(&unconstrained).is_err());
    assert!(camera_access_unit(&valid[..14]).is_err());
    assert!(camera_access_unit(&valid[8..]).is_err());
}
