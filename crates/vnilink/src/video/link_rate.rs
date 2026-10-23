//! What the link carries now, to size a still picture by.
//!
//! The congestion window over the round trip is the rate the sender may
//! send. A still is sent on an idle link, whose queue has drained: its round
//! trip is then the floor, while the window is still sized for the loaded
//! link, so the two together overstate the rate many times over. The
//! window is divided instead by the round trip the link showed lately: the
//! 90th percentile of the samples of the last [`LOADED_FOR`]. Samples come
//! with each frame sent, so a screen in motion fills the window, and a link
//! whose queue fills while it moves reads as loaded through the still
//! period after. A lone spike, a scheduling hiccup on an idle link, is one
//! sample among many and does not count.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

/// How long a round trip seen under load still counts.
pub(crate) const LOADED_FOR: Duration = Duration::from_secs(10);

/// At most this many samples are kept: a second at 240 fps, eight times over.
const SAMPLES: usize = 2_048;

#[derive(Debug, Default)]
pub(crate) struct LinkRate {
    /// The latest congestion window, in bytes.
    window: u64,
    /// The round trips seen within [`LOADED_FOR`], oldest first.
    rtts: VecDeque<(Instant, Duration)>,
}

impl LinkRate {
    /// One reading of the selected path.
    pub(crate) fn observe(&mut self, window: u64, rtt: Duration, now: Instant) {
        self.window = window;
        while self
            .rtts
            .front()
            .is_some_and(|(at, _)| now.saturating_duration_since(*at) > LOADED_FOR)
            || self.rtts.len() >= SAMPLES
        {
            self.rtts.pop_front();
        }
        if !rtt.is_zero() {
            self.rtts.push_back((now, rtt));
        }
    }

    /// The path changed: what was learnt of the old one does not hold.
    pub(crate) fn reset(&mut self) {
        *self = Self::default();
    }

    /// Bits per second the link carries, `None` before any reading.
    pub(crate) fn bits_per_second(&self) -> Option<u64> {
        let mut rtts: Vec<Duration> = self.rtts.iter().map(|(_, rtt)| *rtt).collect();
        if rtts.is_empty() {
            return None;
        }
        rtts.sort_unstable();
        let loaded = rtts[(rtts.len() * 9).div_ceil(10) - 1];
        let micros = u64::try_from(loaded.as_micros()).unwrap_or(u64::MAX).max(1);
        Some(self.window.saturating_mul(8).saturating_mul(1_000_000) / micros)
    }
}
