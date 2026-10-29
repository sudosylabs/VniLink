//! How coarse a recovery keyframe is.
//!
//! A keyframe that answers a loss crosses the same lossy link as the frame
//! it replaces, so it should cost a few pictures' bytes, not the dozens a
//! full-quality keyframe of a screen takes. Platform encoders take no byte
//! limit for one picture. They do take a quantiser floor, and a higher QP is
//! a coarser, smaller picture; the pictures after it refine at the normal
//! rate. [`RecoveryQp`] picks the floor for the next recovery keyframe from
//! how far the last one missed its budget: about six QP steps per halving of
//! the bytes, as H.264 and HEVC quantisers scale.

use crate::pump::EncodeParams;

/// A recovery keyframe aims at this many pictures' bytes at the current
/// bitrate and frame rate.
pub const RECOVERY_KEYFRAME_FRAMES: u32 = 4;

/// H.264 and HEVC 8-bit quantiser range.
const MAX_QP: u8 = 51;
/// Below this the floor stops shrinking anything worth the lost detail.
const MIN_FLOOR: u8 = 24;
/// Where an encoder that has made no recovery keyframe yet starts.
const FIRST_FLOOR: u8 = 40;
/// QP steps that halve an intra picture's bytes.
const STEPS_PER_HALVING: f64 = 6.0;
/// What a keyframe at [`FIRST_FLOOR`] weighs against one at full quality.
/// Measured on VideoToolbox with a text-heavy screen (commit `1aca252f`):
/// 0.58 at 720p and 0.74 at 1080p, both at 2 Mb/s, on Apple silicon's
/// hardware H.264 only. The other encoders that take a floor, Media
/// Foundation's AMD and unlisted hardware transforms, start from the same
/// share unmeasured; it only seeds the first recovery keyframe, and
/// [`RecoveryQp::observe`] corrects the floor from there. The
/// `recovery_keyframe_sizes` measurements of `vnilink-macos` and
/// `vnilink-windows` show whether recovery keyframes fit their budget on
/// another encoder.
const FIRST_FLOOR_SHARE: f64 = 0.65;

/// The quantiser floor for one encoder's recovery keyframes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RecoveryQp {
    floor: u8,
    /// A recovery keyframe has been seen, and the floor follows those.
    learned: bool,
}

impl Default for RecoveryQp {
    fn default() -> Self {
        Self {
            floor: FIRST_FLOOR,
            learned: false,
        }
    }
}

impl RecoveryQp {
    /// The lowest QP the next recovery keyframe may use.
    pub fn floor(&self) -> u8 {
        self.floor
    }

    /// Bytes a recovery keyframe should fit in at `params`' rate.
    pub fn budget(params: &EncodeParams) -> usize {
        let per_frame = params.bitrate_bps / 8 / u32::from(params.frame_rate.max(1));
        per_frame.saturating_mul(RECOVERY_KEYFRAME_FRAMES) as usize
    }

    /// Moves the floor after a recovery keyframe of `bytes` encoded at
    /// `params`: up when it overran the budget, down when it used less than
    /// half, so a floor that was only needed for a busy screen is let go.
    pub fn observe(&mut self, bytes: usize, params: &EncodeParams) {
        self.learned = true;
        let budget = Self::budget(params).max(1);
        if bytes == 0 || (bytes <= budget && bytes * 2 >= budget) {
            return;
        }
        let steps = (STEPS_PER_HALVING * (bytes as f64 / budget as f64).log2()).round();
        // A floor going down moves at most one halving per keyframe: the
        // picture that used less than half may have been an easy one.
        let steps = steps.clamp(-STEPS_PER_HALVING, f64::from(MAX_QP)) as i16;
        self.floor =
            (i16::from(self.floor) + steps).clamp(i16::from(MIN_FLOOR), i16::from(MAX_QP)) as u8;
    }

    /// Seeds the floor from a full-quality keyframe of `bytes` at `params`,
    /// until a recovery keyframe has been seen: the first recovery keyframe
    /// would otherwise start from a guess, and on a busy screen run several
    /// times over its budget.
    pub fn observe_full(&mut self, bytes: usize, params: &EncodeParams) {
        if self.learned || bytes == 0 {
            return;
        }
        let budget = Self::budget(params).max(1) as f64;
        let at_first_floor = bytes as f64 * FIRST_FLOOR_SHARE;
        let steps = (STEPS_PER_HALVING * (at_first_floor / budget).log2()).round();
        self.floor =
            (f64::from(FIRST_FLOOR) + steps).clamp(f64::from(MIN_FLOOR), f64::from(MAX_QP)) as u8;
    }
}
