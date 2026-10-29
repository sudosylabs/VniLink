//! When a viewer asks the host for a keyframe.
//!
//! Every loss after the first wants the same keyframe: predicted frames
//! still on the way, or the fragments of a keyframe already coming, report
//! loss again before the keyframe can arrive. Asking for each would make
//! the host send keyframe after keyframe, the largest frames, at the moment
//! the network is losing them. So one request is outstanding at a time; it
//! is asked again only when no keyframe arrived within a round trip and two
//! frames, and any keyframe that arrives answers it.

use std::time::{Duration, Instant};

/// Shortest wait before asking again, for a loopback round trip.
const MIN_PATIENCE: Duration = Duration::from_millis(50);
/// Longest wait, so a lost request on a slow path is asked again.
const MAX_PATIENCE: Duration = Duration::from_secs(1);

#[derive(Debug, Default)]
pub(crate) struct KeyframeRequests {
    /// A keyframe is wanted and not yet asked for or answered.
    owed: bool,
    /// When the outstanding request was sent.
    asked_at: Option<Instant>,
}

impl KeyframeRequests {
    /// A loss or a decode failure wants a keyframe.
    pub(crate) fn want(&mut self) {
        self.owed = true;
    }

    /// Whether to send a request now: something is owed, and no request
    /// is outstanding or the outstanding one went unanswered too long.
    pub(crate) fn should_send(&mut self, now: Instant, rtt: Duration, frame: Duration) -> bool {
        if !self.owed {
            return false;
        }
        if let Some(asked) = self.asked_at {
            if now < asked + patience(rtt, frame) {
                return false;
            }
        }
        self.asked_at = Some(now);
        true
    }

    /// When to look again for an unanswered request, if one is owed.
    pub(crate) fn retry_at(&self, rtt: Duration, frame: Duration) -> Option<Instant> {
        self.asked_at
            .filter(|_| self.owed)
            .map(|asked| asked + patience(rtt, frame))
    }

    /// Whether a keyframe is wanted and has not arrived.
    pub(crate) fn outstanding(&self) -> bool {
        self.owed
    }

    /// A keyframe arrived: whatever was owed is answered.
    pub(crate) fn keyframe_arrived(&mut self) {
        self.owed = false;
        self.asked_at = None;
    }
}

fn patience(rtt: Duration, frame: Duration) -> Duration {
    (rtt + frame * 2).clamp(MIN_PATIENCE, MAX_PATIENCE)
}

#[cfg(test)]
mod tests {
    use super::*;

    const RTT: Duration = Duration::from_millis(40);
    const FRAME: Duration = Duration::from_millis(16);

    #[test]
    fn one_request_is_outstanding_until_a_keyframe_or_patience_runs_out() {
        let start = Instant::now();
        let mut requests = KeyframeRequests::default();
        assert!(!requests.should_send(start, RTT, FRAME), "nothing is owed");
        requests.want();
        assert!(requests.should_send(start, RTT, FRAME));
        // Losses while the keyframe is on its way ask nothing more.
        requests.want();
        assert!(!requests.should_send(start + Duration::from_millis(60), RTT, FRAME));
        assert_eq!(
            requests.retry_at(RTT, FRAME),
            Some(start + Duration::from_millis(72))
        );
        // Unanswered past a round trip and two frames: asked again.
        assert!(requests.should_send(start + Duration::from_millis(72), RTT, FRAME));
        // A keyframe answers it; the next loss asks at once.
        requests.keyframe_arrived();
        assert_eq!(requests.retry_at(RTT, FRAME), None);
        let later = start + Duration::from_millis(80);
        assert!(!requests.should_send(later, RTT, FRAME));
        requests.want();
        assert!(requests.should_send(later, RTT, FRAME));
    }

    #[test]
    fn patience_is_bounded_on_very_short_and_very_long_paths() {
        let start = Instant::now();
        let mut requests = KeyframeRequests::default();
        requests.want();
        assert!(requests.should_send(start, Duration::ZERO, Duration::ZERO));
        assert_eq!(
            requests.retry_at(Duration::ZERO, Duration::ZERO),
            Some(start + MIN_PATIENCE)
        );
        assert_eq!(
            requests.retry_at(Duration::from_secs(5), FRAME),
            Some(start + MAX_PATIENCE)
        );
    }
}
