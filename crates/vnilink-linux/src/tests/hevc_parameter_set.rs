use vnilink_media::hevc_units;

use crate::hevc_parameter_set::{parameter_sets, slice_header, Layout};
use crate::syntax::{rbsp, Bits};

/// Intel's low-power encoder on a Raptor Lake GPU (iHD 26.1): 64x64 trees,
/// asymmetric partitions and per-unit quantizers required, the rest allowed.
const IHD_FEATURES: u32 = 0x0190_0464;
const IHD_BLOCK_SIZES: u32 = 0x88cf;

fn layout() -> Layout {
    Layout::from_attributes(Some(IHD_FEATURES), Some(IHD_BLOCK_SIZES))
}

#[test]
fn the_layout_follows_the_drivers_block_sizes_and_turns_on_what_it_allows() {
    let layout = layout();
    assert_eq!(
        (
            layout.log2_ctb,
            layout.log2_min_cb,
            layout.log2_max_tb,
            layout.log2_min_tb
        ),
        (6, 3, 5, 2)
    );
    assert_eq!((layout.depth_inter, layout.depth_intra), (2, 2));
    assert!(layout.amp && layout.sao && layout.temporal_mvp && layout.transform_skip);
    assert!(
        !layout.strong_intra_smoothing,
        "the driver does not allow it"
    );
    assert!(layout.cu_qp_delta);
    assert_eq!(layout.cu_qp_delta_depth(), 3);
    assert_eq!(layout.ctbs((1920, 1088)), 30 * 17);

    let unknown = Layout::from_attributes(None, None);
    assert_eq!((unknown.log2_ctb, unknown.log2_min_cb), (5, 3));
}

#[test]
fn the_sps_names_the_coded_size_and_crops_it_to_the_visible_one() {
    let sets = parameter_sets(&layout(), 1920, 1080).unwrap();
    let units = hevc_units(&sets).unwrap();
    let kinds: Vec<u8> = units.iter().map(|nal| (nal[0] >> 1) & 63).collect();
    assert_eq!(kinds, [32, 33, 34]);
    let sps = rbsp(&units[1][2..]);
    let mut bits = Bits::new(&sps);
    // Parameter set ids, sub-layers and the 96-bit profile, tier and level.
    bits.bits(8).unwrap();
    for _ in 0..3 {
        bits.bits(32).unwrap();
    }
    assert_eq!(bits.ue(), Some(0));
    assert_eq!(bits.ue(), Some(1), "4:2:0");
    assert_eq!((bits.ue(), bits.ue()), (Some(1920), Some(1088)));
    assert_eq!(bits.bit(), Some(1), "conformance window");
    let window: Vec<_> = (0..4).map(|_| bits.ue().unwrap()).collect();
    assert_eq!(window, [0, 0, 0, 4], "8 luma rows off the bottom");
    assert!(parameter_sets(&layout(), 1921, 1080).is_err());
}

/// `slice_type` and the bits after it, from a slice header NAL.
fn slice_start(header: &[u8]) -> (u8, u8, u32) {
    let kind = (header[4] >> 1) & 63;
    let payload = rbsp(&header[6..]);
    let mut bits = Bits::new(&payload);
    let first = bits.bit().unwrap();
    if kind == 19 {
        bits.bit().unwrap();
    }
    bits.ue().unwrap();
    (kind, first, bits.ue().unwrap())
}

#[test]
fn slice_headers_mark_keyframes_and_predicted_pictures_by_the_drivers_rules() {
    let mut layout = layout();
    let idr = slice_header(&layout, true, 0);
    assert_eq!(&idr[..4], &[0, 0, 0, 1]);
    assert_eq!(slice_start(&idr), (19, 1, 2));
    assert_eq!(slice_start(&slice_header(&layout, false, 1)), (1, 1, 1));
    layout.generalized_b = true;
    assert_eq!(slice_start(&slice_header(&layout, false, 1)), (1, 1, 0));
    // Byte aligned: the last byte ends with the stop bit or later zeros.
    let last = *slice_header(&layout, false, 4095).last().unwrap();
    assert_ne!(last, 0);
}
