//! How fast a file sender writes, so a transfer yields to what else
//! crosses the link.
//!
//! Through a relay, every connection to a peer shares one queue that never
//! drops: QUIC sees no loss, only delay, and a bulk transfer grows that
//! queue to seconds ahead of the desktop session's packets. A sender paces
//! its writes from its own connection's round trip instead, as LEDBAT does
//! (RFC 6817): the least round trip seen lately is the path's, anything
//! above it is queue, and the rate moves to keep that queue near
//! [`TARGET_QUEUE`]. The queue is shared, so the transfer sees the very
//! delay it would add to the session, and yields without knowing it is
//! there.
//!
//! On a direct path the sender is left to QUIC's congestion control.

use std::collections::VecDeque;
use std::sync::Mutex;
use std::time::Duration;

use tokio::time::Instant;

/// Queueing the sender aims to add: well under what the desktop's own
/// backlog rule tolerates.
pub(crate) const TARGET_QUEUE: Duration = Duration::from_millis(50);

/// Queueing at which the rate halves at once rather than easing down.
const HALVE_AT: u32 = 4;

/// Rate change per second at a full target's distance from it.
const GAIN: f64 = 1_000_000.0;

/// The least rate a paced transfer keeps, so it always progresses.
pub(crate) const FLOOR: f64 = 64.0 * 1024.0;

/// The base delay is the least round trip over this many buckets of
/// [`BUCKET`]: old enough to have seen an empty queue, young enough to
/// follow a path that got longer.
const BUCKETS: usize = 12;
const BUCKET: Duration = Duration::from_secs(10);

/// Bytes a sender may write at once before the rate applies.
const BURST: f64 = 256.0 * 1024.0;

/// The least round trip over the last [`BUCKETS`] × [`BUCKET`].
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
        // Buckets older than the window are dropped by age, not count: a
        // sender that sampled rarely keeps no stale least.
        let window = BUCKET * BUCKETS as u32;
        while self
            .buckets
            .front()
            .is_some_and(|(started, _)| now.saturating_duration_since(*started) >= window)
        {
            self.buckets.pop_front();
        }
    }

    fn least(&self) -> Option<Duration> {
        self.buckets.iter().map(|(_, least)| *least).min()
    }
}

/// The rate controller: no limit until a relayed path shows queueing, then
/// a rate that follows it.
#[derive(Debug, Default)]
pub(crate) struct Controller {
    /// Bytes per second; `None` writes as fast as QUIC takes.
    rate: Option<f64>,
    base: BaseDelay,
    /// Bytes per second written lately, the rate a first cut starts from.
    written: f64,
    updated: Option<Instant>,
}

impl Controller {
    /// One round-trip sample of the connection's selected path. A path
    /// that is not `relayed` lifts the limit, and `path_changed` forgets
    /// what the old path's round trip was.
    pub(crate) fn observe(
        &mut self,
        rtt: Duration,
        relayed: bool,
        path_changed: bool,
        now: Instant,
    ) {
        if path_changed {
            self.base = BaseDelay::default();
        }
        let elapsed = self
            .updated
            .replace(now)
            .map_or(Duration::ZERO, |at| now.saturating_duration_since(at));
        if !relayed || rtt.is_zero() {
            self.rate = None;
            return;
        }
        self.base.observe(rtt, now);
        let Some(base) = self.base.least() else {
            return;
        };
        let queue = rtt.saturating_sub(base);
        if queue >= TARGET_QUEUE * HALVE_AT {
            let from = self.rate.unwrap_or(self.written.max(FLOOR * 2.0));
            self.rate = Some((from / 2.0).max(FLOOR));
            return;
        }
        let Some(rate) = self.rate else {
            // No limit yet, and no queue worth one.
            if queue <= TARGET_QUEUE {
                return;
            }
            self.rate = Some(self.written.max(FLOOR));
            return;
        };
        let off_target = 1.0 - queue.as_secs_f64() / TARGET_QUEUE.as_secs_f64();
        let next = rate + GAIN * off_target * elapsed.as_secs_f64();
        // Growing far past what is written proves nothing: the sender or
        // QUIC holds it back, not the rate.
        let ceiling = (self.written * 2.0).max(FLOOR * 2.0);
        self.rate = Some(next.clamp(FLOOR, ceiling.max(rate)));
    }

    /// The rate writes are held to, if any.
    pub(crate) fn rate(&self) -> Option<f64> {
        self.rate
    }

    fn wrote(&mut self, bytes_per_second: f64) {
        self.written = if self.written == 0.0 {
            bytes_per_second
        } else {
            self.written * 0.8 + bytes_per_second * 0.2
        };
    }
}

/// A connection's pace, shared by its streams: the controller and the
/// token bucket writes draw from.
#[derive(Debug)]
pub(crate) struct Pace {
    state: Mutex<State>,
}

#[derive(Debug)]
struct State {
    controller: Controller,
    tokens: f64,
    refilled: Instant,
    /// Bytes written since `window_started`, for the written rate.
    window_bytes: u64,
    window_started: Instant,
}

impl Default for Pace {
    fn default() -> Self {
        let now = Instant::now();
        Self {
            state: Mutex::new(State {
                controller: Controller::default(),
                tokens: BURST,
                refilled: now,
                window_bytes: 0,
                window_started: now,
            }),
        }
    }
}

impl Pace {
    /// See [`Controller::observe`].
    pub(crate) fn observe(&self, rtt: Duration, relayed: bool, path_changed: bool) {
        let now = Instant::now();
        let mut state = lock(&self.state);
        let window = now.saturating_duration_since(state.window_started);
        if window >= Duration::from_millis(500) {
            let rate = state.window_bytes as f64 / window.as_secs_f64();
            state.controller.wrote(rate);
            state.window_bytes = 0;
            state.window_started = now;
        }
        state.controller.observe(rtt, relayed, path_changed, now);
    }

    /// The rate writes are held to, if any, in bytes per second.
    #[cfg(any(test, feature = "test-probes"))]
    pub(crate) fn rate(&self) -> Option<f64> {
        lock(&self.state).controller.rate()
    }

    /// Waits until `bytes` may be written at the current rate, then takes
    /// them.
    pub(crate) async fn wait(&self, bytes: usize) {
        loop {
            let delay = {
                let mut state = lock(&self.state);
                let now = Instant::now();
                state.window_bytes += bytes as u64;
                let Some(rate) = state.controller.rate() else {
                    state.tokens = BURST;
                    state.refilled = now;
                    return;
                };
                let since = now.saturating_duration_since(state.refilled).as_secs_f64();
                state.tokens = (state.tokens + since * rate).min(BURST.max(bytes as f64));
                state.refilled = now;
                if state.tokens >= bytes as f64 {
                    state.tokens -= bytes as f64;
                    return;
                }
                state.window_bytes -= bytes as u64;
                Duration::from_secs_f64((bytes as f64 - state.tokens) / rate)
            };
            tokio::time::sleep(delay).await;
        }
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use super::*;

    const MS: Duration = Duration::from_millis(1);

    /// Feeds `samples` round trips 50 ms apart from `start`, relayed.
    fn feed(controller: &mut Controller, start: Instant, from: u64, rtts: &[u64]) -> Instant {
        let mut at = start;
        for (i, rtt) in rtts.iter().enumerate() {
            at = start + MS * 50 * (from as u32 + i as u32);
            controller.observe(MS * *rtt as u32, true, false, at);
        }
        at
    }

    #[test]
    fn no_limit_until_a_relayed_path_queues() {
        let now = Instant::now();
        let mut controller = Controller::default();
        controller.wrote(5_000_000.0);
        feed(&mut controller, now, 0, &[40, 42, 60, 85]);
        assert_eq!(controller.rate(), None, "within target");
        controller.observe(MS * 120, true, false, now + MS * 250);
        assert_eq!(
            controller.rate(),
            Some(5_000_000.0),
            "limited from what is written"
        );
        // A direct path lifts it.
        controller.observe(MS * 300, false, false, now + MS * 300);
        assert_eq!(controller.rate(), None);
    }

    #[test]
    fn a_deep_queue_halves_the_rate_and_the_floor_holds() {
        let now = Instant::now();
        let mut controller = Controller::default();
        controller.wrote(4_000_000.0);
        feed(&mut controller, now, 0, &[40]);
        controller.observe(MS * 300, true, false, now + MS * 50);
        assert_eq!(controller.rate(), Some(2_000_000.0));
        for step in 2..40 {
            controller.observe(MS * 300, true, false, now + MS * 50 * step);
        }
        assert_eq!(controller.rate(), Some(FLOOR));
    }

    #[test]
    fn the_rate_eases_toward_the_target_queue() {
        let now = Instant::now();
        let mut controller = Controller::default();
        controller.wrote(2_000_000.0);
        feed(&mut controller, now, 0, &[40, 100]);
        let limited = controller.rate().unwrap();
        // Above target: down, gently.
        controller.observe(MS * 140, true, false, now + MS * 150);
        let lower = controller.rate().unwrap();
        assert!(
            lower < limited && lower > limited / 2.0,
            "{limited} -> {lower}"
        );
        // Below target: up again, no further than twice what is written.
        let mut at = now + MS * 150;
        for _ in 0..200 {
            at += MS * 50;
            controller.observe(MS * 45, true, false, at);
        }
        assert_eq!(controller.rate(), Some(4_000_000.0));
    }

    #[test]
    fn the_base_delay_follows_a_longer_path() {
        let now = Instant::now();
        let mut controller = Controller::default();
        controller.wrote(1_000_000.0);
        controller.observe(MS * 20, true, false, now);
        // A new path, longer: its own round trip is not queue.
        controller.observe(MS * 200, true, true, now + MS * 50);
        controller.observe(MS * 210, true, false, now + MS * 100);
        assert_eq!(controller.rate(), None);
        // Without a path change, an old least expires after two minutes.
        let mut aged = Controller::default();
        aged.observe(MS * 20, true, false, now);
        aged.observe(MS * 200, true, false, now + BUCKET * (BUCKETS as u32 + 1));
        aged.observe(
            MS * 205,
            true,
            false,
            now + BUCKET * (BUCKETS as u32 + 1) + MS * 50,
        );
        assert_eq!(aged.rate(), None);
    }

    #[tokio::test(start_paused = true)]
    async fn writes_wait_for_the_rate() {
        let pace = Pace::default();
        pace.observe(MS * 40, true, false);
        {
            let mut state = lock(&pace.state);
            state.controller.rate = Some(1_000_000.0);
        }
        let started = tokio::time::Instant::now();
        // The burst goes at once; the next megabyte takes about a second.
        pace.wait(256 * 1024).await;
        for _ in 0..4 {
            pace.wait(256 * 1024).await;
        }
        let took = started.elapsed();
        assert!(
            took >= Duration::from_millis(1000) && took < Duration::from_millis(1200),
            "{took:?}"
        );
        // No limit: no wait.
        lock(&pace.state).controller.rate = None;
        let unlimited = tokio::time::Instant::now();
        for _ in 0..8 {
            pace.wait(256 * 1024).await;
        }
        assert!(unlimited.elapsed() < Duration::from_millis(1));
    }
}
