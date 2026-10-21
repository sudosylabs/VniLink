use crate::{EncodeParams, RecoveryQp, RECOVERY_KEYFRAME_FRAMES};

fn params(bitrate_bps: u32, frame_rate: u8) -> EncodeParams {
    EncodeParams {
        force_keyframe: true,
        recovery: true,
        bitrate_bps,
        frame_rate,
    }
}

#[test]
fn the_budget_is_a_few_pictures_at_the_current_rate() {
    assert_eq!(
        RecoveryQp::budget(&params(2_400_000, 30)),
        10_000 * RECOVERY_KEYFRAME_FRAMES as usize
    );
    assert_eq!(RecoveryQp::budget(&params(2_400_000, 0)), 1_200_000);
}

#[test]
fn a_keyframe_inside_its_budget_keeps_the_floor() {
    let mut qp = RecoveryQp::default();
    let start = qp.floor();
    let budget = RecoveryQp::budget(&params(2_000_000, 30));
    qp.observe(budget, &params(2_000_000, 30));
    qp.observe(budget / 2, &params(2_000_000, 30));
    assert_eq!(qp.floor(), start);
}

/// Six steps per halving: twice the budget is six steps up.
#[test]
fn an_overrun_raises_the_floor_by_how_far_it_overran() {
    let mut qp = RecoveryQp::default();
    let start = qp.floor();
    let budget = RecoveryQp::budget(&params(2_000_000, 30));
    qp.observe(budget * 2, &params(2_000_000, 30));
    assert_eq!(qp.floor(), start + 6);
    qp.observe(budget * 1_000, &params(2_000_000, 30));
    assert_eq!(qp.floor(), 51);
}

/// A small keyframe lets the floor down one halving at a time, never below
/// where it stops being worth the detail.
#[test]
fn a_small_keyframe_lowers_the_floor_slowly() {
    let mut qp = RecoveryQp::default();
    let start = qp.floor();
    let budget = RecoveryQp::budget(&params(2_000_000, 30));
    qp.observe(budget / 64, &params(2_000_000, 30));
    assert_eq!(qp.floor(), start - 6);
    for _ in 0..10 {
        qp.observe(budget / 64, &params(2_000_000, 30));
    }
    assert_eq!(qp.floor(), 24);
    qp.observe(0, &params(2_000_000, 30));
    assert_eq!(qp.floor(), 24);
}
