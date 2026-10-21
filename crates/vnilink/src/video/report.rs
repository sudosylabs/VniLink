//! What a viewer lost of a desktop stream, as it tells the host.
//!
//! The viewer's datagram router tallies frames and fragments as they arrive
//! or go missing; every quarter second the viewer sends the tally and starts
//! a new one. The host smooths the reports into loss rates it can act on:
//! how much protection to send, and whether the link needs less.

use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

/// One interval of what arrived and what did not.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct VideoReport {
    /// Frames that reached the decoder whole.
    pub(crate) frames_completed: u32,
    /// Frames lost: unfinished, skipped or too late.
    pub(crate) frames_lost: u32,
    /// Fragments that arrived, data or parity.
    pub(crate) fragments_received: u32,
    /// Fragments of tracked frames that never arrived.
    pub(crate) fragments_missing: u32,
    /// Missing data fragments rebuilt from parity.
    pub(crate) fragments_repaired: u32,
}

/// Encoded size of a [`VideoReport`].
pub(crate) const REPORT_LEN: usize = 20;

impl VideoReport {
    pub(crate) fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    pub(crate) fn encode(&self, bytes: &mut Vec<u8>) {
        for value in [
            self.frames_completed,
            self.frames_lost,
            self.fragments_received,
            self.fragments_missing,
            self.fragments_repaired,
        ] {
            bytes.extend_from_slice(&value.to_be_bytes());
        }
    }

    pub(crate) fn decode(bytes: &[u8; REPORT_LEN]) -> Self {
        let at = |index: usize| {
            u32::from_be_bytes(bytes[index * 4..index * 4 + 4].try_into().expect("4 bytes"))
        };
        Self {
            frames_completed: at(0),
            frames_lost: at(1),
            fragments_received: at(2),
            fragments_missing: at(3),
            fragments_repaired: at(4),
        }
    }
}

/// The viewer's running tally, added to by its datagram router and taken
/// by its control loop.
#[derive(Default)]
pub(crate) struct ReportTally {
    frames_completed: AtomicU32,
    frames_lost: AtomicU32,
    fragments_received: AtomicU32,
    fragments_missing: AtomicU32,
    fragments_repaired: AtomicU32,
}

fn add(counter: &AtomicU32, value: u64) {
    counter.fetch_add(u32::try_from(value).unwrap_or(u32::MAX), Ordering::Relaxed);
}

impl ReportTally {
    pub(crate) fn frame_completed(&self) {
        add(&self.frames_completed, 1);
    }

    pub(crate) fn frames_lost(&self, frames: u64) {
        add(&self.frames_lost, frames);
    }

    pub(crate) fn fragments(&self, counts: super::FragmentCounts) {
        add(&self.fragments_received, counts.received);
        add(&self.fragments_missing, counts.missing);
        add(&self.fragments_repaired, counts.repaired);
    }

    /// The tally since the last take, which starts a new one.
    pub(crate) fn take(&self) -> VideoReport {
        let take = |counter: &AtomicU32| counter.swap(0, Ordering::Relaxed);
        VideoReport {
            frames_completed: take(&self.frames_completed),
            frames_lost: take(&self.frames_lost),
            fragments_received: take(&self.fragments_received),
            fragments_missing: take(&self.fragments_missing),
            fragments_repaired: take(&self.fragments_repaired),
        }
    }
}

/// Weight of the newest report in the host's smoothed rates.
const NEWEST: f64 = 0.3;

/// Reports older than this no longer describe the link.
const REPORT_FRESH: Duration = Duration::from_secs(3);

/// Parity per unit of reported fragment loss: enough to rebuild a frame
/// through most of the loss bursts a rate that size makes likely.
const PARITY_PER_LOSS: f64 = 2.5;

/// Fragment loss below this is a clean link: no parity but a keyframe's.
const CLEAN_LOSS: f64 = 0.001;

/// Parity on a keyframe before any report: losing one starts a spiral.
const KEYFRAME_PARITY: f64 = 0.1;

/// The host's view of what its viewer loses, smoothed over reports.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct ViewerLoss {
    /// Share of fragments that did not arrive, repaired or not.
    pub(crate) fragment_loss: f64,
    /// Share of frames lost after any repair.
    pub(crate) frame_loss: f64,
    pub(crate) heard: Option<Instant>,
}

impl ViewerLoss {
    pub(crate) fn observe(&mut self, report: &VideoReport, at: Instant) {
        let fragments = f64::from(report.fragments_received) + f64::from(report.fragments_missing);
        if fragments > 0.0 {
            let lost = f64::from(report.fragments_missing) / fragments;
            self.fragment_loss = self.blend(self.fragment_loss, lost);
        }
        let frames = f64::from(report.frames_completed) + f64::from(report.frames_lost);
        if frames > 0.0 {
            let lost = f64::from(report.frames_lost) / frames;
            self.frame_loss = self.blend(self.frame_loss, lost);
        }
        self.heard = Some(at);
    }

    /// Parity for a frame, as a share of its data shards, from what the
    /// viewer reports losing. A keyframe gets double, at least
    /// [`KEYFRAME_PARITY`]: a lost one costs the most to send again. On a
    /// `congested` link loss is its queue overflowing, which more parity
    /// would only add to: the keyframe's least is all it gets.
    pub(crate) fn protection(&self, keyframe: bool, congested: bool, now: Instant) -> f64 {
        let fresh = !congested
            && self
                .heard
                .is_some_and(|at| now.saturating_duration_since(at) <= REPORT_FRESH);
        let loss = if fresh { self.fragment_loss } else { 0.0 };
        let share = if loss < CLEAN_LOSS {
            0.0
        } else {
            loss * PARITY_PER_LOSS
        };
        let share = if keyframe {
            (share * 2.0).max(KEYFRAME_PARITY)
        } else {
            share
        };
        share.min(super::MAX_PROTECTION)
    }

    fn blend(&self, smoothed: f64, newest: f64) -> f64 {
        if self.heard.is_none() {
            newest
        } else {
            smoothed * (1.0 - NEWEST) + newest * NEWEST
        }
    }
}

/// How long after the send gate drops a frame the link still counts as
/// congested.
const DROP_CONGESTS: Duration = Duration::from_secs(1);

/// The host's view of its own link: whether it is queueing. Its round trip
/// has grown well past the least seen on the path, or the send gate has
/// just dropped a frame because the link was behind.
#[derive(Debug, Default)]
pub(crate) struct Congestion {
    floor: Option<Duration>,
    dropped_at: Option<Instant>,
}

impl Congestion {
    /// The path's round trip now.
    pub(crate) fn rtt(&mut self, rtt: Duration) {
        if !rtt.is_zero() {
            self.floor = Some(self.floor.map_or(rtt, |floor| floor.min(rtt)));
        }
    }

    pub(crate) fn dropped(&mut self, at: Instant) {
        self.dropped_at = Some(at);
    }

    /// Whether the link is queueing, given its round trip now: at least
    /// twice and 30 ms over the least seen.
    pub(crate) fn congested(&self, rtt: Duration, now: Instant) -> bool {
        let queueing = self
            .floor
            .is_some_and(|floor| rtt >= floor * 2 && rtt >= floor + Duration::from_millis(30));
        queueing
            || self
                .dropped_at
                .is_some_and(|at| now.saturating_duration_since(at) < DROP_CONGESTS)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_report_round_trips() {
        let report = VideoReport {
            frames_completed: 1,
            frames_lost: 2,
            fragments_received: 3,
            fragments_missing: u32::MAX,
            fragments_repaired: 5,
        };
        let mut bytes = Vec::new();
        report.encode(&mut bytes);
        assert_eq!(bytes.len(), REPORT_LEN);
        assert_eq!(
            VideoReport::decode(bytes.as_slice().try_into().unwrap()),
            report
        );
    }

    #[test]
    fn a_tally_is_taken_once() {
        let tally = ReportTally::default();
        tally.frame_completed();
        tally.frames_lost(2);
        tally.fragments(crate::video::FragmentCounts {
            received: 30,
            missing: 3,
            repaired: 1,
        });
        let report = tally.take();
        assert_eq!((report.frames_completed, report.frames_lost), (1, 2));
        assert_eq!(
            (
                report.fragments_received,
                report.fragments_missing,
                report.fragments_repaired
            ),
            (30, 3, 1)
        );
        assert!(tally.take().is_empty());
    }

    #[test]
    fn protection_follows_reported_loss() {
        let now = Instant::now();
        let mut loss = ViewerLoss::default();
        // Before any report: only keyframes, a little.
        assert_eq!(loss.protection(false, false, now), 0.0);
        assert!((loss.protection(true, false, now) - KEYFRAME_PARITY).abs() < 1e-9);
        loss.observe(
            &VideoReport {
                fragments_received: 98,
                fragments_missing: 2,
                ..VideoReport::default()
            },
            now,
        );
        assert!((loss.protection(false, false, now) - 0.05).abs() < 1e-9);
        assert!((loss.protection(true, false, now) - 0.1).abs() < 1e-9);
        // Heavy loss is capped; a stale report counts as no loss.
        loss.fragment_loss = 0.4;
        assert_eq!(
            loss.protection(true, false, now),
            crate::video::MAX_PROTECTION
        );
        let later = now + REPORT_FRESH + Duration::from_secs(1);
        assert_eq!(loss.protection(false, false, later), 0.0);
    }

    #[test]
    fn a_congested_link_gets_no_more_parity_for_its_loss() {
        let now = Instant::now();
        let mut loss = ViewerLoss::default();
        loss.observe(
            &VideoReport {
                fragments_received: 90,
                fragments_missing: 10,
                ..VideoReport::default()
            },
            now,
        );
        assert!(loss.protection(false, false, now) > 0.2);
        assert_eq!(loss.protection(false, true, now), 0.0);
        assert!((loss.protection(true, true, now) - KEYFRAME_PARITY).abs() < 1e-9);
    }

    #[test]
    fn a_link_is_congested_while_its_round_trip_grows_or_after_a_drop() {
        let now = Instant::now();
        let ms = Duration::from_millis;
        let mut link = Congestion::default();
        assert!(!link.congested(ms(500), now), "nothing to compare with");
        link.rtt(ms(20));
        link.rtt(ms(0));
        assert!(!link.congested(ms(35), now));
        assert!(link.congested(ms(50), now));
        // A long path's jitter is not queueing.
        let mut far = Congestion::default();
        far.rtt(ms(80));
        assert!(!far.congested(ms(120), now));
        assert!(far.congested(ms(160), now));
        far.dropped(now);
        assert!(far.congested(ms(80), now + ms(500)));
        assert!(!far.congested(ms(80), now + DROP_CONGESTS));
    }

    #[test]
    fn loss_is_smoothed_from_the_first_report() {
        let mut loss = ViewerLoss::default();
        let at = Instant::now();
        let report = |received, missing, completed, lost| VideoReport {
            frames_completed: completed,
            frames_lost: lost,
            fragments_received: received,
            fragments_missing: missing,
            fragments_repaired: 0,
        };
        loss.observe(&report(90, 10, 9, 1), at);
        assert!((loss.fragment_loss - 0.1).abs() < 1e-9);
        assert!((loss.frame_loss - 0.1).abs() < 1e-9);
        loss.observe(&report(100, 0, 10, 0), at);
        assert!((loss.fragment_loss - 0.07).abs() < 1e-9);
        // An interval with nothing in it leaves the rates as they were.
        loss.observe(&report(0, 0, 0, 0), at);
        assert!((loss.fragment_loss - 0.07).abs() < 1e-9);
    }
}
