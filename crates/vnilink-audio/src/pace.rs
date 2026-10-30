// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! How much sound an output keeps queued ahead of its device, and how it
//! gets back there: one small target, nudged by a slight speed change for
//! clock drift and met at once by dropping a block when far behind. The
//! target grows after the device runs dry, as on a jittery network, and
//! shrinks back while it does not.

use std::time::Duration;

/// The least a device keeps queued: two 20 ms packets.
pub const MIN_TARGET: Duration = Duration::from_millis(40);
/// The most a jittery network can raise the target to.
pub const MAX_TARGET: Duration = Duration::from_millis(120);
/// How much each time the device runs dry raises the target.
const RAISE: Duration = Duration::from_millis(20);
/// How much each calm stretch lowers it again.
const LOWER: Duration = Duration::from_millis(5);
/// Blocks without running dry that make a calm stretch: five seconds.
const CALM_BLOCKS: u32 = 250;
/// Queued sound beyond the target past which a block is dropped instead of
/// played late: more than one block of ordinary arrival jitter.
const DROP_MARGIN: Duration = Duration::from_millis(30);
/// The largest speed change, in parts per million: 0.5 %, too small to hear
/// in speech and only just in music, and only while catching up.
pub const MAX_PPM: f64 = 5_000.0;

/// What to do with the next block.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PaceStep {
    /// Play it, resampled `ppm` faster (positive: more output) or slower.
    Play { ppm: f64 },
    /// Drop it: the output is too far behind.
    Drop,
}

/// The pacing state of one output, in interleaved samples at its rate.
#[derive(Debug, Clone)]
pub struct Pace {
    per_ms: f64,
    target: f64,
    ppm: f64,
    underruns: u64,
    calm: u32,
}

impl Pace {
    /// Pacing for an output of `rate` frames a second and `channels` channels.
    pub fn new(rate: u32, channels: usize) -> Self {
        let per_ms = f64::from(rate) * channels as f64 / 1000.0;
        Self {
            per_ms,
            target: millis(MIN_TARGET) * per_ms,
            ppm: 0.0,
            underruns: 0,
            calm: 0,
        }
    }

    /// The queue to keep, in samples.
    pub fn target(&self) -> usize {
        self.target as usize
    }

    /// The queue to keep, in milliseconds.
    pub fn target_ms(&self) -> f64 {
        self.target / self.per_ms
    }

    /// Decides the next block from `queued` samples ahead of the device and
    /// the device's running count of times it ran dry.
    pub fn step(&mut self, queued: usize, underruns: u64) -> PaceStep {
        let (low, high) = (
            millis(MIN_TARGET) * self.per_ms,
            millis(MAX_TARGET) * self.per_ms,
        );
        if underruns > self.underruns {
            self.underruns = underruns;
            self.target = (self.target + millis(RAISE) * self.per_ms).min(high);
            self.calm = 0;
        } else {
            self.calm += 1;
            if self.calm >= CALM_BLOCKS {
                self.calm = 0;
                self.target = (self.target - millis(LOWER) * self.per_ms).max(low);
            }
        }
        let queued = queued as f64;
        if queued > self.target + millis(DROP_MARGIN) * self.per_ms {
            return PaceStep::Drop;
        }
        let error = ((self.target - queued) / self.target).clamp(-1.0, 1.0);
        self.ppm = (self.ppm * 0.8 + error * MAX_PPM * 0.2).clamp(-MAX_PPM, MAX_PPM);
        PaceStep::Play { ppm: self.ppm }
    }
}

fn millis(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1000.0
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 20 ms of stereo at 48 kHz.
    const BLOCK: f64 = 1920.0;

    #[test]
    fn a_backlog_is_dropped_at_once_and_a_device_clock_is_followed() {
        let mut pace = Pace::new(48_000, 2);
        assert_eq!(pace.step(30_000, 0), PaceStep::Drop, "300 ms behind drops");
        for device_ppm in [-300.0, 300.0] {
            let mut pace = Pace::new(48_000, 2);
            let mut queued = pace.target() as f64;
            for _ in 0..3_000 {
                // A minute of blocks: what is played adds, the device takes at its own rate.
                let PaceStep::Play { ppm } = pace.step(queued as usize, 0) else {
                    panic!("a device near its target never drops");
                };
                queued += BLOCK * (1.0 + ppm / 1e6) - BLOCK * (1.0 + device_ppm / 1e6);
            }
            // A proportional correction settles a little off target under
            // steady drift: 300 ppm of 40 ms leaves about 2.4 ms.
            let target = pace.target() as f64;
            assert!(
                (queued - target).abs() < 5.0 * 96.0,
                "device {device_ppm}: {} ms from target",
                (queued - target) / 96.0
            );
        }
    }

    #[test]
    fn running_dry_raises_the_target_and_calm_lowers_it() {
        let mut pace = Pace::new(48_000, 2);
        assert_eq!(pace.target_ms().round(), 40.0);
        pace.step(0, 1);
        pace.step(0, 2);
        assert_eq!(pace.target_ms().round(), 80.0);
        for underruns in 3..20 {
            pace.step(0, underruns);
        }
        assert_eq!(pace.target_ms().round(), 120.0, "the target stays bounded");
        for _ in 0..CALM_BLOCKS * 20 {
            pace.step(pace.target(), 19);
        }
        assert_eq!(
            pace.target_ms().round(),
            40.0,
            "a calm network brings it back"
        );
    }
}
