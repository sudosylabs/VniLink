// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! How much a file receiver lets its sender have in flight, so a transfer
//! yields to what else crosses the link.
//!
//! Through a relay, every connection to a peer shares one queue that never
//! drops: QUIC sees no loss, only delay, and a bulk transfer grows that
//! queue to seconds ahead of the desktop session's packets. A direct path
//! with a deep bottleneck queue does the same on a smaller scale. The receiver
//! bounds it with QUIC flow control: its connection's receive window is all
//! the sender may have outstanding, whatever the sender has buffered. The
//! window follows the connection's round trip as LEDBAT does (RFC 6817):
//! the least round trip seen lately is the path's, anything above it is
//! queue, and the window moves to keep that queue near [`TARGET_QUEUE`].
//! The queue is shared, so the transfer sees the very delay it would add to
//! the session, and yields without knowing it is there.
//!
//! A connection is dialed with [`INITIAL_WINDOW`], not the endpoint's full
//! window, so the sender cannot have the whole window in flight before the
//! path is known. At most once a round trip the window then grows by up to an
//! eighth while no queue shows, eases down as the queue passes
//! [`TARGET_QUEUE`], and halves once the queue reaches four times it. It
//! never falls below [`MIN_WINDOW`] and never grows past twice what the path
//! carried in a base round trip plus the target, so a fast path with no
//! queue opens to the full window and a slow one stays near where it began
//! until queueing shows.

use std::collections::VecDeque;
use std::time::Duration;

use tokio::time::Instant;

/// Queueing the transfer aims to add: well under what the desktop's own
/// backlog rule tolerates.
pub(crate) const TARGET_QUEUE: Duration = Duration::from_millis(50);

/// Queueing at which the window halves rather than easing down.
const HALVE_AT: u32 = 4;

/// The least window, so a transfer always progresses: about the target's
/// worth of queue at 5 Mb/s.
pub(crate) const MIN_WINDOW: u64 = 32 * 1024;

/// The window a files connection is dialed with and starts from.
pub(crate) const INITIAL_WINDOW: u64 = 1024 * 1024;

/// Bytes per second a path is assumed to carry when its first round trip
/// seeds the window: a path of that length starts from what it holds in one
/// round trip at this rate, when that is more than [`INITIAL_WINDOW`].
const SEED_RATE: f64 = 16.0 * 1024.0 * 1024.0;

/// Queueing at which the window stops doubling each round trip.
const SLOW_START_ENDS: Duration = Duration::from_millis(25);

/// The base delay is the least round trip over [`BASE_WINDOW`], kept as one
/// least per [`BUCKET`]: old enough to have seen an empty queue, young
/// enough to follow a path that got longer.
const BUCKET: Duration = Duration::from_secs(10);
const BASE_WINDOW: Duration = Duration::from_secs(120);

/// The least round trip over the last [`BASE_WINDOW`].
#[derive(Debug, Default)]
struct BaseDelay {
    buckets: VecDeque<(Instant, Duration)>,
}

impl BaseDelay {
    fn observe(&mut self, rtt: Duration, now: Instant) {
        match self.buckets.back_mut() {
            Some((started, least)) if now.saturating_duration_since(*started) < BUCKET => {
                *least = (*least).min(rtt);
            }
            _ => self.buckets.push_back((now, rtt)),
        }
        while self
            .buckets
            .front()
            .is_some_and(|(started, _)| now.saturating_duration_since(*started) >= BASE_WINDOW)
        {
            self.buckets.pop_front();
        }
    }

    fn least(&self) -> Option<Duration> {
        self.buckets.iter().map(|(_, least)| *least).min()
    }
}

/// The receive window controller.
#[derive(Debug)]
pub(crate) struct Window {
    /// The full window, for a direct path.
    full: u64,
    window: u64,
    base: BaseDelay,
    /// Bytes per second received lately, which caps how far the window
    /// grows past what the path carries.
    received: f64,
    /// The last window change, so the window moves at most once a round
    /// trip.
    changed: Option<Instant>,
    /// The path's first round trip has seeded the window.
    seeded: bool,
    /// The window doubles each round trip until a queue shows.
    slow_start: bool,
}

impl Window {
    /// A controller for a connection whose full window is `full`.
    pub(crate) fn new(full: u64) -> Self {
        Self {
            full,
            window: INITIAL_WINDOW.min(full),
            base: BaseDelay::default(),
            received: 0.0,
            changed: None,
            seeded: false,
            slow_start: true,
        }
    }

    /// The window to set now.
    pub(crate) fn window(&self) -> u64 {
        self.window
    }

    /// One sample of the connection's selected path: its round trip,
    /// whether it `changed` since the last, and the bytes per second
    /// received over the last interval.
    pub(crate) fn observe(&mut self, rtt: Duration, changed: bool, received: f64, now: Instant) {
        if changed {
            self.base = BaseDelay::default();
            self.seeded = false;
            self.slow_start = true;
        }
        self.received = if self.received == 0.0 {
            received
        } else {
            self.received * 0.8 + received * 0.2
        };
        if rtt.is_zero() {
            return;
        }
        self.base.observe(rtt, now);
        if !self.seeded {
            self.seeded = true;
            let seed = (SEED_RATE * rtt.as_secs_f64()) as u64;
            self.window = self.window.max(seed.min(self.full));
        }
        let Some(base) = self.base.least() else {
            return;
        };
        let due = self.changed.is_none_or(|at| {
            now.saturating_duration_since(at) >= rtt.max(Duration::from_millis(50))
        });
        if !due {
            return;
        }
        let queue = rtt.saturating_sub(base);
        let target = TARGET_QUEUE.as_secs_f64();
        if queue >= SLOW_START_ENDS {
            self.slow_start = false;
        }
        let next = if queue >= TARGET_QUEUE * HALVE_AT {
            self.window / 2
        } else if self.slow_start {
            self.window.saturating_mul(2)
        } else {
            // Up to an eighth more a round trip with no queue, as much less
            // at twice the target.
            let off_target = 1.0 - queue.as_secs_f64() / target;
            (self.window as f64 * (1.0 + 0.125 * off_target)) as u64
        };
        // Growing past twice what the path carried in a base round trip and
        // the target only queues; the queue alone brings a window down.
        let carried = (self.received * (base.as_secs_f64() + target) * 2.0) as u64;
        let ceiling = carried.max(MIN_WINDOW).min(self.full);
        let next = if next > self.window {
            next.min(ceiling.max(self.window))
        } else {
            next
        }
        .max(MIN_WINDOW);
        if next != self.window {
            self.window = next;
            self.changed = Some(now);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MS: Duration = Duration::from_millis(1);
    const FULL: u64 = 64 << 20;

    /// Samples every 50 ms from `start` at `received` B/s.
    fn run(window: &mut Window, start: Instant, rtts: &[u64], received: f64) -> Instant {
        let mut at = start;
        for rtt in rtts {
            at += MS * 50;
            window.observe(MS * *rtt as u32, false, received, at);
        }
        at
    }

    /// A fast path with no queue opens the window to the full one, within
    /// a few seconds of round trips.
    #[test]
    fn a_fast_path_opens_to_the_full_window() {
        let now = Instant::now();
        let mut window = Window::new(FULL);
        assert_eq!(window.window(), INITIAL_WINDOW);
        run(&mut window, now, &[20; 200], 1e9);
        assert_eq!(window.window(), FULL);
    }

    #[test]
    fn a_deep_queue_halves_the_window_down_to_the_least() {
        let now = Instant::now();
        let mut window = Window::new(FULL);
        let at = run(&mut window, now, &[40], 1e6);
        let mut at = at;
        let mut last = window.window();
        for _ in 0..40 {
            at += MS * 500;
            window.observe(MS * 400, false, 1e6, at);
            assert!(window.window() <= last);
            last = window.window();
        }
        assert_eq!(window.window(), MIN_WINDOW);
    }

    #[test]
    fn with_no_queue_the_window_grows_to_what_the_path_carries() {
        let now = Instant::now();
        let mut window = Window::new(FULL);
        // 20 MB/s at a 40 ms base: a window past 2 x 20 MB/s x 90 ms only
        // queues.
        run(&mut window, now, &[40; 400], 20e6);
        let ceiling = (20e6 * 0.09 * 2.0) as u64;
        assert_eq!(window.window(), ceiling);
        // A path that carries less than the window started from leaves it
        // until queueing shows.
        let mut slow = Window::new(FULL);
        run(&mut slow, now, &[40; 400], 1e6);
        assert_eq!(slow.window(), INITIAL_WINDOW);
    }

    /// Until a queue shows, the window doubles each round trip: a distant
    /// path reaches what it carries in a few of them, not in dozens.
    #[test]
    fn a_path_without_a_queue_doubles_the_window_each_round_trip() {
        let now = Instant::now();
        let mut window = Window::new(FULL);
        let mut at = now;
        let mut sizes = Vec::new();
        for _ in 0..7 {
            at += MS * 100;
            window.observe(MS * 100, false, 1e9, at);
            sizes.push(window.window());
        }
        let first = sizes[0];
        assert!(
            first >= INITIAL_WINDOW,
            "the window starts no lower than it was dialed with"
        );
        assert_eq!(
            sizes[1..],
            [first * 2, first * 4, first * 8, first * 16, FULL, FULL][..6],
            "doubled at each round trip until the full window"
        );
    }

    /// The window moves at most once a round trip, and at most once every
    /// 50 ms on a shorter one.
    #[test]
    fn the_window_moves_again_only_a_round_trip_after_it_last_moved() {
        let start = Instant::now();
        let mut window = Window::new(FULL);
        window.observe(MS * 100, false, 1e9, start);
        let moved = window.window();
        window.observe(
            MS * 100,
            false,
            1e9,
            start + MS * 100 - Duration::from_nanos(1),
        );
        assert_eq!(window.window(), moved, "within the round trip it holds");
        window.observe(MS * 100, false, 1e9, start + MS * 100);
        assert_eq!(window.window(), moved * 2, "a round trip later it moves");

        let mut short = Window::new(FULL);
        short.observe(MS * 10, false, 1e9, start);
        let moved = short.window();
        short.observe(MS * 10, false, 1e9, start + MS * 49);
        assert_eq!(short.window(), moved, "a short trip still waits 50 ms");
        short.observe(MS * 10, false, 1e9, start + MS * 50);
        assert_eq!(short.window(), moved * 2);
    }

    /// A queue ends the doubling: from then on the window grows by an
    /// eighth at most.
    #[test]
    fn a_queue_ends_the_doubling() {
        let now = Instant::now();
        let mut window = Window::new(FULL);
        let mut at = now;
        for _ in 0..2 {
            at += MS * 100;
            window.observe(MS * 40, false, 1e9, at);
        }
        let grown = window.window();
        // 30 ms of queue: less than the target, more than a seed would see.
        at += MS * 100;
        window.observe(MS * 70, false, 1e9, at);
        let queued = window.window();
        assert!(
            queued <= grown + grown / 8 + 1,
            "a queue of 30 ms grows it by an eighth at most: {grown} -> {queued}"
        );
        for _ in 0..3 {
            at += MS * 100;
            window.observe(MS * 40, false, 1e9, at);
        }
        assert!(
            window.window() <= queued * 3 / 2,
            "the doubling does not come back: {queued} -> {}",
            window.window()
        );
    }

    /// The first round trip seeds the window with what a path of that
    /// length carries at a modest rate: a long path does not crawl up from
    /// a megabyte, a short one starts where it was.
    #[test]
    fn a_long_path_starts_from_what_it_carries() {
        let now = Instant::now();
        let mut near = Window::new(FULL);
        near.observe(MS * 10, false, 0.0, now);
        assert_eq!(near.window(), INITIAL_WINDOW);
        let mut far = Window::new(FULL);
        far.observe(MS * 300, false, 0.0, now);
        assert_eq!(far.window(), (SEED_RATE * 0.3) as u64);
        assert!(far.window() > 2 * INITIAL_WINDOW);
        // A new path is seeded again.
        far.observe(MS * 10, true, 0.0, now + MS * 500);
        assert!(far.window() >= INITIAL_WINDOW);
    }

    #[test]
    fn near_the_target_the_window_eases() {
        let now = Instant::now();
        let mut window = Window::new(FULL);
        let at = run(&mut window, now, &[40; 4], 20e6);
        let before = window.window();
        // 75 ms of queue: one round trip later, a little less.
        window.observe(MS * 115, false, 20e6, at + MS * 200);
        let after = window.window();
        assert!(after < before && after > before / 2, "{before} -> {after}");
    }

    #[test]
    fn the_base_delay_follows_a_new_or_longer_path() {
        let now = Instant::now();
        let mut window = Window::new(FULL);
        window.observe(MS * 20, false, 1e6, now);
        // A new path's own round trip is not queue.
        window.observe(MS * 300, true, 1e6, now + MS * 500);
        let kept = window.window();
        window.observe(MS * 310, false, 1e6, now + MS * 1000);
        assert!(window.window() >= kept, "{kept} -> {}", window.window());
        // Without a path change, an old least ages out.
        let mut aged = Window::new(FULL);
        aged.observe(MS * 20, false, 1e6, now);
        let later = now + BASE_WINDOW + MS * 1;
        aged.observe(MS * 300, false, 1e6, later);
        aged.observe(MS * 300, false, 1e6, later + MS * 400);
        assert!(aged.window() >= INITIAL_WINDOW, "{}", aged.window());
    }
}
