// H.264 syntax vectors are pure parsing and run on every platform. The
// hardware round trip at the end runs only where a VA-API render node exists.

use crate::syntax::{Pps, Slice, Sps};

/// Bit writer for synthetic parameter sets and slice headers. NAL payload
/// only: the caller prepends the real header bytes.
#[derive(Default)]
struct Writer {
    bytes: Vec<u8>,
    acc: u64,
    pending: u32,
}

impl Writer {
    fn bit(&mut self, value: u32) {
        self.bits(value, 1);
    }

    fn bits(&mut self, value: u32, width: u32) {
        self.acc = (self.acc << width) | u64::from(value);
        self.pending += width;
        while self.pending >= 8 {
            let shift = self.pending - 8;
            self.bytes.push((self.acc >> shift) as u8);
            if shift == 0 {
                self.acc = 0;
            } else {
                self.acc &= (1u64 << shift) - 1;
            }
            self.pending -= 8;
        }
    }

    fn ue(&mut self, value: u32) {
        let code = value + 1;
        let width = 32 - code.leading_zeros();
        self.bits(0, width - 1);
        self.bits(code, width);
    }

    fn se(&mut self, value: i32) {
        self.ue(if value > 0 {
            (value as u32) * 2 - 1
        } else {
            (-value as u32) * 2
        });
    }

    /// RBSP stop bit, byte alignment, and emulation prevention.
    fn payload(&mut self) -> Vec<u8> {
        self.bit(1);
        if self.pending > 0 {
            self.bits(0, 8 - self.pending);
        }
        let mut out = Vec::with_capacity(self.bytes.len());
        let mut zeros = 0u8;
        for &byte in &self.bytes {
            if zeros == 2 && byte <= 3 {
                out.push(3);
                zeros = 0;
            }
            out.push(byte);
            zeros = if byte == 0 { zeros + 1 } else { 0 };
        }
        self.bytes.clear();
        self.acc = 0;
        self.pending = 0;
        out
    }
}

/// Baseline SPS the way the VA-API encoder writer lays it out: 64x64, one
/// reference, progressive, no crop.
fn baseline_sps() -> Vec<u8> {
    let mut fields = Writer::default();
    fields.ue(0); // seq_parameter_set_id
    fields.ue(4); // log2_max_frame_num_minus4
    fields.ue(0); // pic_order_cnt_type
    fields.ue(4); // log2_max_pic_order_cnt_lsb_minus4
    fields.ue(1); // num_ref_frames
    fields.bit(0); // gaps_in_frame_num_value_allowed_flag
    fields.ue(3); // pic_width_in_mbs_minus1
    fields.ue(3); // pic_height_in_map_units_minus1
    fields.bit(1); // frame_mbs_only_flag
    fields.bit(1); // direct_8x8_inference_flag
    fields.bit(0); // frame_cropping_flag
    let mut nal = vec![0x67, 0x42, 0xc0, 0x0a];
    nal.extend_from_slice(&fields.payload());
    nal
}

fn baseline_pps() -> Vec<u8> {
    let mut fields = Writer::default();
    fields.ue(0); // pic_parameter_set_id
    fields.ue(0); // seq_parameter_set_id
    fields.bit(0); // entropy_coding_mode_flag
    fields.bit(0); // bottom_field_pic_order_in_frame_present_flag
    fields.ue(0); // num_slice_groups_minus1
    fields.ue(0); // num_ref_idx_l0_default_active_minus1
    fields.ue(0); // num_ref_idx_l1_default_active_minus1
    fields.bit(0); // weighted_pred_flag
    fields.bits(0, 2); // weighted_bipred_idc
    fields.se(0); // pic_init_qp_minus26
    fields.se(0); // pic_init_qs_minus26
    fields.se(0); // chroma_qp_index_offset
    fields.bit(1); // deblocking_filter_control_present_flag
    fields.bit(0); // constrained_intra_pred_flag
    fields.bit(0); // redundant_pic_cnt_present_flag
    let mut nal = vec![0x68];
    nal.extend_from_slice(&fields.payload());
    nal
}

/// P slice header for a 64x64 Baseline picture.
fn p_slice(frame_num: u32, poc_lsb: u32) -> Vec<u8> {
    let mut fields = Writer::default();
    fields.ue(0); // first_mb_in_slice
    fields.ue(0); // slice_type P
    fields.ue(0); // pic_parameter_set_id
    fields.bits(frame_num, 8); // frame_num, log2_max_frame_num = 8
    fields.bits(poc_lsb, 8); // pic_order_cnt_lsb
    fields.bit(0); // num_ref_idx_active_override_flag
    fields.bit(0); // ref_pic_list_modification_flag_l0
    fields.bit(0); // adaptive_ref_pic_marking_mode_flag
    fields.se(0); // slice_qp_delta
    fields.ue(0); // disable_deblocking_filter_idc
    fields.se(0); // slice_alpha_c0_offset_div2
    fields.se(0); // slice_beta_offset_div2
    let mut nal = vec![0x41];
    nal.extend_from_slice(&fields.payload());
    nal
}

#[test]
fn baseline_parameter_sets_parse() {
    let sps = Sps::parse(&baseline_sps()).unwrap();
    assert_eq!(sps.profile, 66);
    assert_eq!((sps.coded.0, sps.coded.1), (64, 64));
    assert_eq!((sps.visible.0, sps.visible.1), (64, 64));
    assert_eq!(sps.num_ref_frames, 1);
    assert_eq!(sps.log2_max_frame_num_minus4, 4);
    let pps = Pps::parse(&baseline_pps(), &sps).unwrap();
    assert_eq!(pps.id, 0);
    assert!(!pps.entropy_cabac);
    assert_eq!(pps.pic_init_qp, 26);
    assert_eq!(pps.chroma_qp_offset, [0, 0]);
    // Absent scaling lists fall back to the specification defaults.
    assert_eq!(pps.scaling4[0][0], 6);
    assert_eq!(pps.scaling4[3][0], 10);
}

#[test]
fn p_slice_header_parses() {
    let sps = Sps::parse(&baseline_sps()).unwrap();
    let pps = Pps::parse(&baseline_pps(), &sps).unwrap();
    let slice = Slice::parse(&p_slice(7, 14), &sps, &pps, false, 2).unwrap();
    assert_eq!(slice.first_mb, 0);
    assert_eq!(slice.frame_num, 7);
    assert_eq!(slice.poc_lsb, 14);
    assert_eq!(slice.num_ref_l0, 1);
    assert_eq!(slice.qp_delta, 0);
    // NAL header byte plus the header fields above.
    assert_eq!(
        slice.header_bits,
        8 + 1 + 1 + 1 + 8 + 8 + 1 + 1 + 1 + 1 + 1 + 1 + 1
    );
}

#[test]
fn default_scaling_tables_match_the_published_values() {
    // Provenance guard: these raster-order tables are the specification
    // defaults reproduced by FFmpeg. The hardware round trip arbitrates
    // them; this test pins them against silent edits.
    use crate::syntax::{DEFAULT_INTER_8X8, DEFAULT_INTRA_8X8};
    assert_eq!(DEFAULT_INTRA_8X8.len(), 64);
    assert_eq!(DEFAULT_INTER_8X8.len(), 64);
    assert_eq!(&DEFAULT_INTRA_8X8[..8], &[6, 10, 13, 16, 18, 23, 25, 27]);
    assert_eq!(&DEFAULT_INTRA_8X8[56..], &[27, 29, 31, 33, 36, 38, 40, 42]);
    assert_eq!(&DEFAULT_INTER_8X8[..8], &[9, 13, 15, 17, 19, 21, 22, 24]);
    assert_eq!(&DEFAULT_INTER_8X8[56..], &[24, 25, 27, 28, 30, 32, 33, 35]);
}

#[test]
fn high_profile_sps_parses() {
    // 64x64 High profile without custom scaling lists.
    let mut fields = Writer::default();
    fields.ue(0); // seq_parameter_set_id
    fields.ue(1); // chroma_format_idc = 4:2:0
    fields.ue(0); // bit_depth_luma_minus8
    fields.ue(0); // bit_depth_chroma_minus8
    fields.bit(0); // qpprime_y_zero_transform_bypass_flag
    fields.bit(0); // seq_scaling_matrix_present_flag
    fields.ue(0); // log2_max_frame_num_minus4
    fields.ue(0); // pic_order_cnt_type
    fields.ue(0); // log2_max_pic_order_cnt_lsb_minus4
    fields.ue(1); // num_ref_frames
    fields.bit(0); // gaps
    fields.ue(3); // width mbs
    fields.ue(3); // height maps
    fields.bit(1); // frame_mbs_only_flag
    fields.bit(1); // direct_8x8_inference_flag
    fields.bit(0); // frame_cropping_flag
    let mut nal = vec![0x67, 0x64, 0x00, 0x1f];
    nal.extend_from_slice(&fields.payload());
    let parsed = Sps::parse(&nal).unwrap();
    assert_eq!(parsed.profile, 100);
    assert_eq!((parsed.visible.0, parsed.visible.1), (64, 64));
}

#[test]
fn unsupported_streams_are_refused_explicitly() {
    let sps = Sps::parse(&baseline_sps()).unwrap();
    let pps = Pps::parse(&baseline_pps(), &sps).unwrap();
    // B slice type.
    let mut b = Writer::default();
    b.ue(0);
    b.ue(1);
    let mut b_nal = vec![0x41];
    b_nal.extend_from_slice(&b.payload());
    assert!(matches!(
        Slice::parse(&b_nal, &sps, &pps, false, 2),
        Err(vnilink_media::Error::Unsupported(_))
    ));
    // Interlaced SPS.
    let mut field = Writer::default();
    field.ue(0);
    field.ue(4);
    field.ue(0);
    field.ue(4);
    field.ue(1);
    field.bit(0);
    field.ue(3);
    field.ue(3);
    field.bit(0); // frame_mbs_only_flag = 0
    let mut field_nal = vec![0x67, 0x42, 0xc0, 0x0a];
    field_nal.extend_from_slice(&field.payload());
    assert!(matches!(
        Sps::parse(&field_nal),
        Err(vnilink_media::Error::Unsupported(_))
    ));
    // Slice groups in the PPS.
    let mut fmo = Writer::default();
    fmo.ue(0);
    fmo.ue(0);
    fmo.bit(0);
    fmo.bit(0);
    fmo.ue(1); // num_slice_groups_minus1 = 1
    let mut fmo_nal = vec![0x68];
    fmo_nal.extend_from_slice(&fmo.payload());
    assert!(matches!(
        Pps::parse(&fmo_nal, &sps),
        Err(vnilink_media::Error::Unsupported(_))
    ));
    // Long-term marking operation 6.
    let mut mmco = Writer::default();
    mmco.ue(0); // first_mb
    mmco.ue(0); // slice_type P
    mmco.ue(0); // pps id
    mmco.bits(0, 8); // frame_num
    mmco.bits(0, 8); // poc_lsb
    mmco.bit(0); // num_ref override
    mmco.bit(0); // ref_pic_list_modification
    mmco.bit(1); // adaptive_ref_pic_marking_mode_flag
    mmco.ue(6); // memory_management_control_operation 6
    mmco.ue(3); // max_long_term_frame_idx_plus1
    let mut mmco_nal = vec![0x41];
    mmco_nal.extend_from_slice(&mmco.payload());
    assert!(matches!(
        Slice::parse(&mmco_nal, &sps, &pps, false, 2),
        Err(vnilink_media::Error::Unsupported(_))
    ));
}

#[cfg(target_os = "linux")]
#[test]
fn vaapi_decodes_what_it_encoded() {
    use vnilink::EncodedFrame;
    use vnilink_media::{EncodeParams, VideoDecoder, VideoEncoder};

    use crate::encode::LinuxEncoder;

    let mut encoder = match LinuxEncoder::open(64, 64, 30, 500_000) {
        Ok(encoder) => encoder,
        Err(vnilink_media::Error::Unsupported(_)) => return,
        Err(error) => panic!("VA-API failed to open: {error}"),
    };
    let mut decoder = crate::decode::LinuxDecoder::new();
    let key_params = EncodeParams {
        force_keyframe: true,
        bitrate_bps: 500_000,
        frame_rate: 30,
    };
    // Solid red keyframe, then a solid green delta: the delta proves the
    // reference picture survived in the decoder.
    let keyframe = encoder
        .encode(&solid(64, 64, 0, 0, 0, 255), &key_params)
        .unwrap();
    assert!(keyframe.keyframe());
    let picture = decoder
        .decode(&EncodedFrame::h264(true, 0, keyframe.bytes().to_vec()).unwrap())
        .unwrap();
    assert_red(&picture);
    let delta = encoder
        .encode(
            &solid(64, 64, 33_333, 0, 255, 0),
            &EncodeParams {
                force_keyframe: false,
                ..key_params
            },
        )
        .unwrap();
    let picture = decoder
        .decode(&EncodedFrame::h264(false, 33_333, delta.bytes().to_vec()).unwrap())
        .unwrap();
    assert_green(&picture);
    // A cropped size decodes at the visible size, not the coded size.
    let mut small_encoder = match LinuxEncoder::open(62, 30, 30, 500_000) {
        Ok(encoder) => encoder,
        Err(vnilink_media::Error::Unsupported(_)) => return,
        Err(error) => panic!("VA-API failed to open: {error}"),
    };
    let small = small_encoder
        .encode(&solid(62, 30, 66_666, 0, 0, 255), &key_params)
        .unwrap();
    let mut small_decoder = crate::decode::LinuxDecoder::new();
    let picture = small_decoder
        .decode(&EncodedFrame::h264(true, 66_666, small.bytes().to_vec()).unwrap())
        .unwrap();
    assert_eq!((picture.width(), picture.height()), (62, 30));
    assert_red(&picture);
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
fn channel_mean(picture: &vnilink_media::RawFrame, channel: usize) -> f64 {
    let mut sum = 0u64;
    let mut count = 0u64;
    for row in 0..picture.height() {
        for pixel in picture.row(row).chunks_exact(4) {
            sum += u64::from(pixel[channel]);
            count += 1;
        }
    }
    sum as f64 / count.max(1) as f64
}

#[cfg(target_os = "linux")]
fn assert_red(picture: &vnilink_media::RawFrame) {
    assert!(
        channel_mean(picture, 2) > 128.0
            && channel_mean(picture, 2) > channel_mean(picture, 0)
            && channel_mean(picture, 2) > channel_mean(picture, 1),
        "decoded picture is not the encoded color"
    );
}

#[cfg(target_os = "linux")]
fn assert_green(picture: &vnilink_media::RawFrame) {
    assert!(
        channel_mean(picture, 1) > 128.0
            && channel_mean(picture, 1) > channel_mean(picture, 0)
            && channel_mean(picture, 1) > channel_mean(picture, 2),
        "decoded picture is not the encoded color"
    );
}
