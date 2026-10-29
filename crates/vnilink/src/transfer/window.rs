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
        let next = if queue >= TARGET_QUEUE * HALVE_AT {
            self.window / 2
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
