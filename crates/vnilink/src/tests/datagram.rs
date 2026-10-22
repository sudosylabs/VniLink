use crate::video::{
    decode_video_body, encode_video_body, fragment, EncodedFrame, FragmentCounts, Losses,
    Reassembler, FRAG_HEADER, MAX_PARTIAL_BYTES,
};
use crate::MAX_VIDEO_FRAME;

fn body(keyframe: bool, stamp: u64, payload: &[u8]) -> Vec<u8> {
    encode_video_body(&EncodedFrame::h264(keyframe, stamp, payload.to_vec()).unwrap())
}

#[test]
fn fragments_round_trip_in_order() {
    let body = body(true, 7, &[9; 3000]);
    let frags = fragment(&body, 12, 1200, 0.0);
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
    let frags = fragment(&body, 1, 1200, 0.0);
    assert_eq!(frags.len(), 1);
    let mut reasm = Reassembler::new();
    assert_eq!(reasm.push(&frags[0]).unwrap(), body);
}

#[test]
fn shuffled_fragments_still_complete() {
    let body = body(true, 9, &[5; 5000]);
    let mut frags = fragment(&body, 4, 700, 0.0);
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
    let frags = fragment(&original, 21, 800, 0.0);
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
    for frag in fragment(&other, 22, 1200, 0.0) {
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
    assert!(reasm.push(&fragment(&first, 40, 1200, 0.0)[0]).is_some());
    let next = body(false, 2, &[2; 10]);
    assert!(reasm.push(&fragment(&next, 41, 1200, 0.0)[0]).is_some());
    assert!(!reasm.take_loss().any(), "consecutive frames are no loss");
    let after_gap = body(false, 4, &[4; 10]);
    assert!(reasm
        .push(&fragment(&after_gap, 43, 1200, 0.0)[0])
        .is_some());
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
    assert!(reasm.push(&fragment(&first, 10, 1200, 0.0)[0]).is_some());
    // Nine unfinished frames: the oldest is pushed out by the ninth.
    for seq in 11..20 {
        let big = body(false, u64::from(seq), &[2; 3000]);
        assert!(reasm.push(&fragment(&big, seq, 1200, 0.0)[0]).is_none());
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
    assert!(reasm.push(&fragment(&next, 21, 1200, 0.0)[0]).is_some());
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
    let a = fragment(&first, 30, 900, 0.0);
    let b = fragment(&second, 31, 900, 0.0);
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

/// A frame whose fragments keep coming completes however long it takes to
/// arrive, as a keyframe does on a slow link.
#[test]
fn a_frame_still_arriving_outlives_the_silence() {
    let silence = std::time::Duration::from_millis(40);
    let keyframe = body(true, 1, &[3; 6000]);
    let frags = fragment(&keyframe, 50, 900, 0.0);
    let mut reasm = Reassembler::with_silence(silence);
    let mut done = None;
    for frag in &frags {
        // Fragments a little closer together than the silence, adding up
        // to several times it.
        std::thread::sleep(silence / 2);
        done = done.or(reasm.push(frag));
    }
    assert_eq!(done, Some(keyframe));
    assert!(!reasm.take_loss().any());
}

/// A frame whose fragments stop coming is lost once it has been silent for
/// the span, and a fragment arriving after that does not revive it.
#[test]
fn a_silent_frame_expires() {
    let silence = std::time::Duration::from_millis(40);
    let frame = body(false, 2, &[4; 3000]);
    let frags = fragment(&frame, 60, 900, 0.0);
    let mut reasm = Reassembler::with_silence(silence);
    assert!(reasm.push(&frags[0]).is_none());
    std::thread::sleep(silence * 2);
    assert!(reasm.push(&frags[1]).is_none());
    assert_eq!(
        reasm.take_loss(),
        Losses {
            expired: 1,
            ..Losses::default()
        }
    );
}

/// The reassembler counts every fragment that arrives, and the fragments a
/// frame never got once it is given up.
#[test]
fn fragments_are_counted_for_the_loss_report() {
    let silence = std::time::Duration::from_millis(20);
    let whole = fragment(&body(true, 1, &[1; 3000]), 70, 900, 0.0);
    let cut = fragment(&body(false, 2, &[2; 3000]), 71, 900, 0.0);
    let mut reasm = Reassembler::with_silence(silence);
    for frag in &whole {
        reasm.push(frag);
    }
    reasm.push(&cut[0]);
    assert_eq!(
        reasm.take_fragments(),
        FragmentCounts {
            received: whole.len() as u64 + 1,
            missing: 0,
            repaired: 0,
        }
    );
    std::thread::sleep(silence * 2);
    // Pushing anything checks for silent frames.
    reasm.push(&[0; 2]);
    assert_eq!(
        reasm.take_fragments(),
        FragmentCounts {
            received: 0,
            missing: cut.len() as u64 - 1,
            repaired: 0,
        }
    );
}

/// Any parity-count of a protected frame's shards may go missing and the
/// frame still arrives exactly, its rebuilt data shards counted as repaired;
/// one more missing and it never completes.
#[test]
fn parity_rebuilds_a_frame_missing_that_many_shards() {
    let frame = body(true, 3, &(0..9000u32).map(|n| n as u8).collect::<Vec<_>>());
    let frags = fragment(&frame, 80, 1000, 0.25);
    let data = u16::from_be_bytes([frags[0][6], frags[0][7]]) as usize;
    let parity = u16::from_be_bytes([frags[0][8], frags[0][9]]) as usize;
    assert_eq!(frags.len(), data + parity);
    assert_eq!(parity, crate::video::parity_count(data, 0.25));

    // The first `parity` data shards missing: all rebuilt.
    let mut reasm = Reassembler::new();
    let mut done = None;
    for frag in &frags[parity..] {
        done = done.or(reasm.push(frag));
    }
    assert_eq!(done, Some(frame.clone()));
    let counts = reasm.take_fragments();
    assert_eq!(counts.repaired, parity as u64);
    assert!(!reasm.take_loss().any());

    // One more missing: never whole.
    let mut reasm = Reassembler::new();
    for frag in &frags[parity + 1..] {
        assert!(reasm.push(frag).is_none());
    }
}

/// A small protected frame carries two parity shards whatever its share, so
/// it survives two lost datagrams; an unprotected one carries none.
#[test]
fn a_small_protected_frame_survives_two_lost_datagrams() {
    assert_eq!(crate::video::parity_count(1, 0.01), 2);
    assert_eq!(crate::video::parity_count(40, 0.01), 2);
    assert_eq!(crate::video::parity_count(40, 0.25), 10);
    assert_eq!(crate::video::parity_count(1, 0.0), 0);
    let frame = body(false, 4, &[7u8; 300]);
    let frags = fragment(&frame, 81, 1000, 0.01);
    assert_eq!(frags.len(), 3, "one data shard and two parity shards");
    let mut reasm = Reassembler::new();
    assert_eq!(reasm.push(&frags[2]), Some(frame));
}

/// Seeded random loss: a protected frame arrives exactly whenever no more
/// than its parity count of shards is lost, and never otherwise.
#[test]
fn random_loss_within_parity_always_rebuilds() {
    use rand::rngs::StdRng;
    use rand::{RngExt, SeedableRng};
    let mut rng = StdRng::seed_from_u64(0xfec);
    for round in 0..200u32 {
        let len = rng.random_range(1..20_000);
        let payload: Vec<u8> = (0..len).map(|_| rng.random::<u8>()).collect();
        let frame = body(round % 5 == 0, u64::from(round), &payload);
        let protection = rng.random_range(0.05..0.5);
        let frags = fragment(&frame, round, rng.random_range(200..1400), protection);
        let data = u16::from_be_bytes([frags[0][6], frags[0][7]]) as usize;
        let parity = frags.len() - data;
        let lose = rng.random_range(0..=parity + 2).min(frags.len());
        let mut order: Vec<usize> = (0..frags.len()).collect();
        for at in (1..order.len()).rev() {
            order.swap(at, rng.random_range(0..=at));
        }
        let mut reasm = Reassembler::new();
        let mut done = None;
        for &index in &order[lose..] {
            done = done.or(reasm.push(&frags[index]));
        }
        if lose <= parity {
            assert_eq!(
                done.as_ref(),
                Some(&frame),
                "round {round}: {lose} of {parity} lost"
            );
        } else {
            assert_eq!(
                done, None,
                "round {round}: {lose} lost past {parity} parity"
            );
        }
    }
}

/// A fragment claiming more parity than any frame carries, or counts that
/// differ from its frame's, is ignored.
#[test]
fn malformed_parity_is_ignored() {
    let frags = fragment(&body(false, 4, &[5; 3000]), 90, 900, 0.5);
    let mut too_much = frags[0].clone();
    too_much[8..10].copy_from_slice(&100u16.to_be_bytes());
    let mut reasm = Reassembler::new();
    assert!(reasm.push(&too_much).is_none());
    assert_eq!(reasm.take_fragments().received, 0);
    assert!(reasm.push(&frags[0]).is_none());
    let mut other = frags[1].clone();
    other[8..10].copy_from_slice(&0u16.to_be_bytes());
    assert!(reasm.push(&other).is_none());
    assert_eq!(reasm.take_fragments().received, 1);
}

/// A forged shard of frame `seq` claiming 8192 data shards, so it never
/// completes.
fn forged(seq: u32, index: u16, payload: &[u8]) -> Vec<u8> {
    let mut datagram = seq.to_be_bytes().to_vec();
    datagram.extend(index.to_be_bytes());
    datagram.extend(8192u16.to_be_bytes());
    datagram.extend(0u16.to_be_bytes());
    datagram.extend_from_slice(payload);
    datagram
}

/// Eight frames that never complete, of shards as large as a fragment may
/// be, would hold 120 MB here: the oldest are evicted at the byte budget.
#[test]
fn forged_frames_never_hold_more_than_the_byte_budget() {
    let mut reasm = Reassembler::new();
    let payload = vec![7; 60 * 1024];
    for n in 0..2048u32 {
        assert!(reasm
            .push(&forged(n % 8, (n / 8) as u16, &payload))
            .is_none());
        assert!(reasm.buffered_bytes() <= MAX_PARTIAL_BYTES);
    }
    assert!(reasm.take_loss().evicted > 0);
}

/// One frame larger than the budget alone is dropped, not kept growing.
#[test]
fn a_frame_larger_than_the_byte_budget_is_dropped() {
    let mut reasm = Reassembler::new();
    let payload = vec![7; 60 * 1024];
    for index in 0..1024 {
        assert!(reasm.push(&forged(1, index, &payload)).is_none());
        assert!(reasm.buffered_bytes() <= MAX_PARTIAL_BYTES);
    }
    assert!(reasm.take_loss().evicted > 0);
}

/// The largest frame a sender makes still completes beside forged frames
/// filling most of the budget.
#[test]
fn the_largest_frame_fits_the_byte_budget() {
    let mut reasm = Reassembler::new();
    let payload = vec![7; 60 * 1024];
    for index in 0..100 {
        assert!(reasm.push(&forged(1, index, &payload)).is_none());
    }
    let largest = body(true, 2, &vec![5; MAX_VIDEO_FRAME]);
    let fragments = fragment(&largest, 2, 1410, 0.5);
    let mut whole = Vec::new();
    for datagram in fragments.iter().rev() {
        whole.extend(reasm.push(datagram));
    }
    assert_eq!(whole, [largest]);
    assert_eq!(reasm.take_loss(), Losses::default());
}

/// A frame's role crosses the wire in its flags; a still is only ever an
/// AV1 keyframe, and no frame is both a still and unchanged.
#[test]
fn a_frames_role_round_trips_and_a_malformed_still_is_refused() {
    use crate::{FrameRole, VideoCodec};
    let changed = EncodedFrame::h264(false, 1, vec![9; 40]).unwrap();
    let unchanged = changed.clone().unchanged();
    let still = EncodedFrame::still(3, vec![7; 40]).unwrap();
    for frame in [&changed, &unchanged, &still] {
        let back = decode_video_body(&encode_video_body(frame)).unwrap();
        assert_eq!(back.role, frame.role);
        assert_eq!(back.keyframe, frame.keyframe);
        assert_eq!(back.codec, frame.codec);
    }
    assert_eq!(still.codec, VideoCodec::Av1);
    assert_eq!(unchanged.role, FrameRole::Unchanged);
    // A still stays a still when marked unchanged.
    assert_eq!(still.clone().unchanged().role, FrameRole::Still);
    let flags = |frame: &EncodedFrame, flags: u8| {
        let mut body = encode_video_body(frame);
        body[1] = flags;
        decode_video_body(&body)
    };
    // Still flag on a predicted frame, on H.264, or with the unchanged flag.
    assert!(flags(&still, 2).is_err());
    assert!(flags(&EncodedFrame::h264(true, 1, vec![9; 40]).unwrap(), 3).is_err());
    assert!(flags(&still, 7).is_err());
}
