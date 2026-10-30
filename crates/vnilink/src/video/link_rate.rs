// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

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

#[cfg(test)]
mod tests {
    use super::*;

    /// An idle link's round trip is its floor while its window is still sized
    /// for the loaded link: the rate is the window over the round trip the link
    /// showed lately, and a lone spike is not that.
    #[test]
    fn the_link_carries_its_window_over_its_loaded_round_trip() {
        use crate::video::LinkRate;
        let start = Instant::now();
        let at = |ms: u64| start + Duration::from_millis(ms);
        let mut link = LinkRate::default();
        assert_eq!(link.bits_per_second(), None, "nothing read yet");
        // A second of motion at 30 fps on 10 Mb/s with 270 ms of round trip
        // under load: a 337.5 kB window.
        for frame in 0..30 {
            link.observe(337_500, Duration::from_millis(270), at(frame * 33));
        }
        assert_eq!(link.bits_per_second(), Some(10_000_000));
        // Then still for two seconds: the queue drains and the round trip
        // falls to its floor, but the rate does not rise.
        for second in 1..=2 {
            link.observe(337_500, Duration::from_millis(20), at(1_000 * second));
        }
        assert_eq!(link.bits_per_second(), Some(10_000_000));
        // A smaller window after a loss lowers it at once.
        link.observe(168_750, Duration::from_millis(20), at(2_500));
        assert_eq!(link.bits_per_second(), Some(5_000_000));
        // Long after the load, the idle round trip is what the link shows.
        link.observe(25_000, Duration::from_millis(20), at(15_000));
        assert_eq!(link.bits_per_second(), Some(10_000_000));
        // A lone spike on an idle link is not its load.
        let mut idle = LinkRate::default();
        for frame in 0..40 {
            idle.observe(13_200, Duration::from_millis(1), at(frame * 33));
        }
        idle.observe(13_200, Duration::from_millis(30), at(1_400));
        assert_eq!(idle.bits_per_second(), Some(105_600_000));
        link.reset();
        assert_eq!(link.bits_per_second(), None, "a new path is unknown");
    }
}
