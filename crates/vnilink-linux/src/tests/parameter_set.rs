// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

use vnilink_media::parse_annex_b;

use crate::parameter_set::{parameter_sets, round_up_16, EncodeProfile};

#[test]
fn parameter_sets_are_annex_b_with_crop_for_a_non_macroblock_size() {
    assert_eq!(round_up_16(66), 80);
    let bytes = parameter_sets(EncodeProfile::Baseline, 66, 48, 30).unwrap();
    let parsed = parse_annex_b(&bytes).unwrap();
    assert!(parsed.has_sps() && parsed.has_pps());
    assert_eq!(parsed.units()[0].bytes()[0] & 0x1F, 7);
    assert_eq!(parsed.units()[0].bytes()[1], 66);
    assert!(parameter_sets(EncodeProfile::Baseline, 0, 48, 30).is_err());
}

#[test]
fn high_parameter_sets_carry_profile_100_and_parse() {
    let bytes = parameter_sets(EncodeProfile::High, 64, 64, 30).unwrap();
    let parsed = parse_annex_b(&bytes).unwrap();
    assert!(parsed.has_sps() && parsed.has_pps());
    let sps_nal = parsed
        .units()
        .iter()
        .find(|unit| unit.bytes()[0] & 0x1F == 7)
        .expect("SPS")
        .bytes();
    assert_eq!(sps_nal[1], 100);
    // The hand-written High sets must survive our own VA-API SPS/PPS parser
    // with the right profile and visible size.
    let sps = crate::syntax::Sps::parse(sps_nal).unwrap();
    assert_eq!(sps.profile, 100);
    assert_eq!(sps.visible, (64, 64));
    let pps_nal = parsed
        .units()
        .iter()
        .find(|unit| unit.bytes()[0] & 0x1F == 8)
        .expect("PPS")
        .bytes();
    let pps = crate::syntax::Pps::parse(pps_nal, &sps).unwrap();
    assert!(pps.entropy_cabac);
}
