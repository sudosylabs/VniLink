use crate::video::{
    decode_video_body, encode_video_body, fragment, EncodedFrame, Losses, Reassembler, FRAG_HEADER,
};

fn body(keyframe: bool, stamp: u64, payload: &[u8]) -> Vec<u8> {
    encode_video_body(&EncodedFrame::h264(keyframe, stamp, payload.to_vec()).unwrap())
}

#[test]
fn fragments_round_trip_in_order() {
    let body = body(true, 7, &[9; 3000]);
    let frags = fragment(&body, 12, 1200);
    assert!(frags.len() > 1);
    for frag in &frags {
        assert!(frag.len() <= 1200);
    }
    let mut reasm = Reassembler::new();
    let mut done = None;
    for frag in &frags {
        done = reasm.push(frag);
    }
    let done = done.expect("last fragment completes the frame");
    assert_eq!(done, body);
    let frame = decode_video_body(&done).unwrap();
    assert!(frame.keyframe && frame.timestamp_us == 7);
}

#[test]
fn a_single_small_frame_is_one_fragment() {
    let body = body(false, 3, &[1, 2, 3]);
    let frags = fragment(&body, 1, 1200);
    assert_eq!(frags.len(), 1);
    let mut reasm = Reassembler::new();
    assert_eq!(reasm.push(&frags[0]).unwrap(), body);
}

#[test]
fn shuffled_fragments_still_complete() {
    let body = body(true, 9, &[5; 5000]);
    let mut frags = fragment(&body, 4, 700);
    frags.reverse();
    let mut reasm = Reassembler::new();
    let mut done = None;
    for frag in &frags {
        if let Some(body) = reasm.push(frag) {
            done = Some(body);
        }
    }
    assert_eq!(done.unwrap(), body);
}

#[test]
fn loss_never_completes_and_stragglers_are_ignored() {
    let original = body(false, 11, &[7; 4000]);
    let frags = fragment(&original, 21, 800);
    assert!(frags.len() > 2);
    let mut reasm = Reassembler::new();
    // Drop the middle fragment: nothing may complete.
    for (index, frag) in frags.iter().enumerate() {
        if index != frags.len() / 2 {
            assert!(reasm.push(frag).is_none());
        }
    }
    // The late fragment arrives after a newer frame completed: the old frame
    // would reach the decoder out of order, so it is lost, and said so.
    let other = body(true, 12, &[8; 100]);
    for frag in fragment(&other, 22, 1200) {
        let _ = reasm.push(&frag);
    }
    assert!(
        !reasm.take_loss().any(),
        "the first complete frame is no loss"
    );
    assert_eq!(reasm.push(&frags[frags.len() / 2]), None);
    assert_eq!(
        reasm.take_loss(),
        Losses {
            stale: 1,
            ..Losses::default()
        },
        "an out-of-order frame was not reported lost"
    );
    assert!(!reasm.take_loss().any(), "a loss is reported once");
    // ...while a true duplicate of a finished frame is ignored.
    assert!(reasm.push(&frags[0]).is_none());
    assert!(!reasm.take_loss().any());
}

/// A frame completing past a missing one says the one between is lost, so
/// the viewer asks for a keyframe instead of decoding against a gap.
#[test]
fn a_frame_after_a_gap_reports_the_loss() {
    let mut reasm = Reassembler::new();
    let first = body(true, 1, &[1; 10]);
    assert!(reasm.push(&fragment(&first, 40, 1200)[0]).is_some());
    let next = body(false, 2, &[2; 10]);
    assert!(reasm.push(&fragment(&next, 41, 1200)[0]).is_some());
    assert!(!reasm.take_loss().any(), "consecutive frames are no loss");
    let after_gap = body(false, 4, &[4; 10]);
    assert!(reasm.push(&fragment(&after_gap, 43, 1200)[0]).is_some());
    assert_eq!(
        reasm.take_loss(),
        Losses {
            gap: 1,
            ..Losses::default()
        },
        "frame 42 went missing unnoticed"
    );
}

/// A frame evicted unfinished counts once, even when a later frame skips
/// past it, so diagnostics count frames, not ways of noticing them.
#[test]
fn a_lost_frame_is_counted_once_by_how_it_was_lost() {
    let mut reasm = Reassembler::new();
    let first = body(true, 1, &[1; 10]);
    assert!(reasm.push(&fragment(&first, 10, 1200)[0]).is_some());
    // Nine unfinished frames: the oldest is pushed out by the ninth.
    for seq in 11..20 {
        let big = body(false, u64::from(seq), &[2; 3000]);
        assert!(reasm.push(&fragment(&big, seq, 1200)[0]).is_none());
    }
    assert_eq!(
        reasm.take_loss(),
        Losses {
            evicted: 1,
            ..Losses::default()
        }
    );
    // Frame 21 pushes out 12 as it arrives, then completes, skipping 11 to
    // 20: ten frames lost in all, 11 and 12 not counted again.
    let next = body(false, 21, &[3; 10]);
    assert!(reasm.push(&fragment(&next, 21, 1200)[0]).is_some());
    assert_eq!(
        reasm.take_loss(),
        Losses {
            evicted: 1,
            gap: 8,
            ..Losses::default()
        }
    );
}

#[test]
fn malformed_fragments_are_ignored() {
    let mut reasm = Reassembler::new();
    assert!(reasm.push(&[]).is_none());
    assert!(reasm.push(&[0; FRAG_HEADER - 1]).is_none());
    // count zero, index past count.
    let mut bad = vec![0; FRAG_HEADER + 4];
    bad[6..8].copy_from_slice(&0u16.to_be_bytes());
    assert!(reasm.push(&bad).is_none());
    let mut bad = vec![0; FRAG_HEADER + 4];
    bad[4..6].copy_from_slice(&5u16.to_be_bytes());
    bad[6..8].copy_from_slice(&3u16.to_be_bytes());
    assert!(reasm.push(&bad).is_none());
}

#[test]
fn interleaved_frames_each_complete() {
    let first = body(true, 1, &[1; 2500]);
    let second = body(false, 2, &[2; 2500]);
    let a = fragment(&first, 30, 900);
    let b = fragment(&second, 31, 900);
    let mut reasm = Reassembler::new();
    let mut done = Vec::new();
    for pair in a.iter().zip(b.iter()) {
        if let Some(body) = reasm.push(pair.0) {
            done.push(body);
        }
        if let Some(body) = reasm.push(pair.1) {
            done.push(body);
        }
    }
    assert_eq!(done.len(), 2);
    assert!(done.contains(&first) && done.contains(&second));
}
