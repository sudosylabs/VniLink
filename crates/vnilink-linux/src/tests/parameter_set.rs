use vnilink_media::parse_annex_b;

use crate::parameter_set::{parameter_sets, round_up_16};

#[test]
fn parameter_sets_are_annex_b_with_crop_for_a_non_macroblock_size() {
    assert_eq!(round_up_16(66), 80);
    let bytes = parameter_sets(66, 48, 30).unwrap();
    let parsed = parse_annex_b(&bytes).unwrap();
    assert!(parsed.has_sps() && parsed.has_pps());
    assert_eq!(parsed.units()[0].bytes()[0] & 0x1F, 7);
    assert_eq!(parsed.units()[0].bytes()[1], 66);
    assert!(parameter_sets(0, 48, 30).is_err());
}
