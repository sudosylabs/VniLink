// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

use vnilink::MAX_VIDEO_FRAME;

use crate::annexb::{parse_annex_b, to_annex_b, AvccAccessUnit};
use crate::error::Error;

fn avcc(length_size: u8, nals: &[&[u8]]) -> Vec<u8> {
    let mut out = Vec::new();
    for nal in nals {
        match length_size {
            1 => out.push(nal.len() as u8),
            2 => out.extend_from_slice(&(nal.len() as u16).to_be_bytes()),
            _ => out.extend_from_slice(&(nal.len() as u32).to_be_bytes()),
        }
        out.extend_from_slice(nal);
    }
    out
}

fn unit<'a>(
    length_size: u8,
    avcc: &'a [u8],
    sets: &'a [Vec<u8>],
    keyframe: bool,
) -> AvccAccessUnit<'a> {
    AvccAccessUnit {
        nal_length_size: length_size,
        avcc,
        parameter_sets: sets,
        keyframe,
    }
}

#[test]
fn length_prefixed_nals_become_annex_b() {
    let slice = [0x41, 0x9A];
    let idr = [0x65, 0x88];
    let packed = avcc(4, &[&slice, &idr]);
    let annex = to_annex_b(&unit(4, &packed, &[], false)).unwrap();
    assert_eq!(&annex[..4], &[0, 0, 0, 1]);
    let parsed = parse_annex_b(&annex).unwrap();
    assert_eq!(parsed.units().len(), 2);
    assert_eq!(parsed.units()[0].nal_type, 1);
    assert_eq!(parsed.units()[0].bytes(), &slice);
    assert_eq!(parsed.units()[1].nal_type, 5);
    assert!(parsed.has_idr());
    assert!(!parsed.has_sps());
}

#[test]
fn one_and_two_byte_lengths_round_trip() {
    let nal = [0x41, 0x01, 0x02];
    for size in [1u8, 2] {
        let packed = avcc(size, &[&nal]);
        let annex = to_annex_b(&unit(size, &packed, &[], false)).unwrap();
        let parsed = parse_annex_b(&annex).unwrap();
        assert_eq!(parsed.units()[0].bytes(), &nal);
    }
}

#[test]
fn a_keyframe_prefixes_parameter_sets_in_order() {
    let sps = vec![0x67, 0x42];
    let pps = vec![0x68, 0xCE];
    let idr = [0x65, 0x11];
    let packed = avcc(4, &[&idr]);
    let sets = [sps.clone(), pps.clone()];
    let annex = to_annex_b(&unit(4, &packed, &sets, true)).unwrap();
    let parsed = parse_annex_b(&annex).unwrap();
    assert!(parsed.has_sps());
    assert!(parsed.has_pps());
    assert!(parsed.has_idr());
    assert_eq!(parsed.units()[0].bytes(), sps.as_slice());
    assert_eq!(parsed.units()[1].bytes(), pps.as_slice());
    assert_eq!(parsed.units()[2].bytes(), &idr);
}

#[test]
fn a_predicted_frame_omits_parameter_sets() {
    let sps = vec![0x67, 0x42];
    let slice = [0x41, 0x22];
    let packed = avcc(4, &[&slice]);
    let sets = [sps];
    let annex = to_annex_b(&unit(4, &packed, &sets, false)).unwrap();
    let parsed = parse_annex_b(&annex).unwrap();
    assert_eq!(parsed.units().len(), 1);
    assert!(!parsed.has_sps());
    assert_eq!(parsed.units()[0].bytes(), &slice);
}

#[test]
fn three_byte_and_four_byte_start_codes_both_parse() {
    let three = [0, 0, 1, 0x41, 0x01, 0, 0, 0, 1, 0x41, 0x02];
    let parsed = parse_annex_b(&three).unwrap();
    assert_eq!(parsed.units().len(), 2);
    assert_eq!(parsed.units()[1].bytes(), &[0x41, 0x02]);

    let leading_zero = [0, 0, 0, 0, 1, 0x65, 0x01];
    let parsed = parse_annex_b(&leading_zero).unwrap();
    assert!(parsed.has_idr());
    assert_eq!(parsed.units()[0].bytes(), &[0x65, 0x01]);
}

#[test]
fn malformed_samples_are_rejected() {
    let nal = [0x41, 0x01];
    let packed = avcc(4, &[&nal]);
    assert!(matches!(
        to_annex_b(&unit(3, &packed, &[], false)),
        Err(Error::InvalidFrame(
            "H.264 length prefix must be 1, 2, or 4 bytes"
        ))
    ));
    assert!(matches!(
        to_annex_b(&unit(4, &[], &[], false)),
        Err(Error::InvalidFrame("encoded frame is empty"))
    ));
    assert!(matches!(
        to_annex_b(&unit(4, &packed, &[], true)),
        Err(Error::InvalidFrame("a keyframe needs its parameter sets"))
    ));
    assert!(matches!(
        to_annex_b(&unit(4, &[0, 0, 0], &[], false)),
        Err(Error::InvalidFrame("H.264 length prefix is truncated"))
    ));
    assert!(matches!(
        to_annex_b(&unit(4, &[0, 0, 0, 2, 0x41], &[], false)),
        Err(Error::InvalidFrame("H.264 NAL unit is truncated"))
    ));
    assert!(matches!(
        to_annex_b(&unit(4, &[0, 0, 0, 0], &[], false)),
        Err(Error::InvalidFrame("an H.264 NAL unit is empty"))
    ));
    let empty_set = vec![];
    assert!(matches!(
        to_annex_b(&unit(4, &packed, &[empty_set], true)),
        Err(Error::InvalidFrame("an H.264 NAL unit is empty"))
    ));
    let started = vec![0, 0, 0, 1, 0x67];
    assert!(matches!(
        to_annex_b(&unit(4, &packed, &[started], true)),
        Err(Error::InvalidFrame(
            "an H.264 NAL unit already has a start code"
        ))
    ));
    assert!(matches!(
        parse_annex_b(&[]),
        Err(Error::InvalidFrame("encoded frame is empty"))
    ));
    assert!(matches!(
        parse_annex_b(&[0x41, 0x01, 0x02]),
        Err(Error::InvalidFrame("H.264 frame is not Annex-B"))
    ));
    assert!(matches!(
        parse_annex_b(&[0, 0, 0, 1, 0, 0, 0, 1]),
        Err(Error::InvalidFrame("an H.264 NAL unit is empty"))
    ));
}

#[test]
fn an_oversized_sample_is_rejected() {
    let mut huge = vec![0u8; MAX_VIDEO_FRAME + 1];
    huge[0] = 0x41;
    assert!(matches!(
        to_annex_b(&unit(4, &huge, &[], false)),
        Err(Error::InvalidFrame("encoded frame is larger than 1 MiB"))
    ));
    assert!(matches!(
        parse_annex_b(&huge),
        Err(Error::InvalidFrame("encoded frame is larger than 1 MiB"))
    ));

    let nal_len = (MAX_VIDEO_FRAME - 8) as u32;
    let mut packed = nal_len.to_be_bytes().to_vec();
    packed.extend(std::iter::repeat(0x41).take(nal_len as usize));
    let sets = [vec![0x67, 0x42], vec![0x68, 0xCE]];
    assert!(matches!(
        to_annex_b(&unit(4, &packed, &sets, true)),
        Err(Error::InvalidFrame("encoded frame is larger than 1 MiB"))
    ));
}

#[test]
fn nal_unit_debug_hides_bytes() {
    let packed = avcc(4, &[&[0xAB, 0xCD]]);
    let annex = to_annex_b(&unit(4, &packed, &[], false)).unwrap();
    let parsed = parse_annex_b(&annex).unwrap();
    let rendered = format!("{:?}", parsed.units()[0]);
    assert!(!rendered.contains("171"));
    assert!(!rendered.contains("205"));
    assert!(rendered.contains("len"));
}
