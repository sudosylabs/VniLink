//! Times across the two computers. Each process states a time as
//! microseconds on its own monotonic clock ([`now_us`]); the suite
//! estimates how far the agent's clock is from its own by asking the agent
//! the time several times ([`ClockOffset::estimate`]) and keeps the answer
//! that came back fastest: the agent read its clock somewhere within that
//! round trip, so the estimate is wrong by at most half of it
//! ([`ClockOffset::error_us`]). The two clocks also drift apart, by up to
//! some tens of microseconds a second for ordinary oscillators, so an
//! estimate holds only near when it was made: a run estimates before and
//! after what it measures and states the larger error plus how far the
//! offset moved between them ([`ClockOffset::bound_us`]). Nothing here
//! reads a wall clock, which either computer may step while a run
//! measures.

use std::sync::OnceLock;
use std::time::{Duration, Instant};

/// The instant this process's clock counts from: the first time it was read.
fn epoch() -> Instant {
    static EPOCH: OnceLock<Instant> = OnceLock::new();
    *EPOCH.get_or_init(Instant::now)
}

/// This process's clock now, in microseconds.
pub fn now_us() -> u64 {
    us_at(Instant::now())
}

/// `instant` on this process's clock, in microseconds; zero for an instant
/// before the clock was first read.
pub fn us_at(instant: Instant) -> u64 {
    u64::try_from(instant.saturating_duration_since(epoch()).as_micros()).unwrap_or(u64::MAX)
}

/// The instant `us` on this process's clock stands for.
pub fn instant_at(us: u64) -> Instant {
    epoch() + Duration::from_micros(us)
}

/// One question of the agent's time: when the suite asked, what the agent
/// answered, and when the answer arrived, the suite's times by its clock.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ClockSample {
    pub asked_us: u64,
    pub agent_us: u64,
    pub answered_us: u64,
}

/// How far the agent's clock is ahead of the suite's, and how well that is
/// known.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ClockOffset {
    /// The agent's clock minus the suite's, in microseconds.
    pub offset_us: i64,
    /// The round trip of the sample the estimate came from.
    pub rtt_us: u64,
}

impl ClockOffset {
    /// The estimate from `samples`: the one with the shortest round trip,
    /// taking the agent's answer as read halfway through it. `None` for no
    /// samples.
    pub fn estimate(samples: &[ClockSample]) -> Option<Self> {
        let best = samples
            .iter()
            .filter(|sample| sample.answered_us >= sample.asked_us)
            .min_by_key(|sample| sample.answered_us - sample.asked_us)?;
        let midpoint = (i128::from(best.asked_us) + i128::from(best.answered_us)) / 2;
        let offset = i128::from(best.agent_us) - midpoint;
        Some(Self {
            offset_us: i64::try_from(offset).ok()?,
            rtt_us: best.answered_us - best.asked_us,
        })
    }

    /// How far a converted time may be from the truth: the agent read its
    /// clock somewhere in the round trip, at most half of it from where the
    /// estimate puts it.
    pub fn error_us(&self) -> u64 {
        self.rtt_us.div_ceil(2)
    }

    /// How far a time converted with this estimate, made before a run, may
    /// be off anywhere in the run, given `after`, the estimate made after
    /// it: the larger of the two errors plus how far the offset drifted.
    pub fn bound_us(&self, after: &ClockOffset) -> u64 {
        self.error_us().max(after.error_us()) + self.offset_us.abs_diff(after.offset_us)
    }

    /// The agent's time `agent_us` on the suite's clock, in microseconds.
    pub fn suite_us(&self, agent_us: u64) -> u64 {
        let local = i128::from(agent_us) - i128::from(self.offset_us);
        u64::try_from(local.max(0)).unwrap_or(u64::MAX)
    }

    /// The agent's time `agent_us` as an instant of the suite's process.
    pub fn suite_instant(&self, agent_us: u64) -> Instant {
        instant_at(self.suite_us(agent_us))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(asked_us: u64, agent_us: u64, answered_us: u64) -> ClockSample {
        ClockSample {
            asked_us,
            agent_us,
            answered_us,
        }
    }

    /// The fastest answer wins, read as halfway through its round trip,
    /// and the error it states is half that round trip.
    #[test]
    fn the_estimate_takes_the_fastest_answer_as_read_halfway() {
        let offset = ClockOffset::estimate(&[
            sample(1_000, 50_900, 1_800),
            sample(2_000, 51_150, 2_200),
            sample(3_000, 53_000, 3_900),
        ])
        .expect("an estimate");
        assert_eq!(
            offset,
            ClockOffset {
                offset_us: 49_050,
                rtt_us: 200
            },
            "the 200 µs sample decides, its answer read at 2 100 µs"
        );
        assert_eq!(offset.error_us(), 100, "half the round trip");
        assert_eq!(
            offset.suite_us(51_150),
            2_100,
            "the agent's answer lands where it was assumed to be read"
        );
    }

    /// An agent whose clock is behind the suite's converts forward, and no
    /// time converts before the suite's clock began.
    #[test]
    fn a_clock_behind_converts_forward_and_never_before_zero() {
        let offset = ClockOffset::estimate(&[sample(10_000, 100, 10_400)]).expect("an estimate");
        assert_eq!(offset.offset_us, -10_100, "the agent is behind");
        assert_eq!(offset.suite_us(300), 10_400);
        let ahead = ClockOffset::estimate(&[sample(0, 5_000, 10)]).expect("an estimate");
        assert_eq!(ahead.suite_us(1_000), 0, "clamped to the clock's start");
    }

    /// A run's bound holds the larger error and the drift between the
    /// estimates before and after it.
    #[test]
    fn a_run_is_bounded_by_the_larger_error_and_the_drift() {
        let before = ClockOffset {
            offset_us: 1_000,
            rtt_us: 400,
        };
        let after = ClockOffset {
            offset_us: 1_030,
            rtt_us: 100,
        };
        assert_eq!(
            before.bound_us(&after),
            230,
            "200 µs of error and 30 of drift"
        );
        assert_eq!(before.bound_us(&before), 200, "no drift, its own error");
    }

    /// No samples, or only impossible ones, make no estimate.
    #[test]
    fn no_sample_makes_no_estimate() {
        assert_eq!(ClockOffset::estimate(&[]), None);
        assert_eq!(ClockOffset::estimate(&[sample(10, 5, 9)]), None);
    }
}
