// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! One consumer's still pictures of a still screen.
// trace: records every ignored failure

use std::time::{Duration, Instant};

use crate::frame::RawFrame;
use crate::still::{Made, StillWorker, STILL_AFTER};

/// One target's still pictures: asked for once the screen has had no more
/// than small changes for [`STILL_AFTER`], once per such still period, and
/// published only if the screen did not change at all while it was made: a
/// small change then would leave the still showing what the stream already
/// replaced, and the viewer redraws only where the stream says it changed.
pub(super) struct Stills {
    /// The service the stills are for, to record.
    service: u64,
    /// The viewer agreed to still pictures.
    pub(super) agreed: bool,
    worker: Option<StillWorker>,
    /// Changes beyond small ones, and when the last came.
    change: u64,
    changed_at: Instant,
    /// Every change, small ones included, and when the last came.
    screen: u64,
    screen_at: Instant,
    /// The change and the screen a still was last asked for at.
    asked: Option<(u64, u64)>,
    /// The change a still was last published for.
    sent: Option<u64>,
}

/// How long the screen stays the same before a still is asked for again,
/// after one was made while it changed.
const STILL_SETTLE: Duration = Duration::from_millis(100);

impl Stills {
    /// No still yet for `service`, its screen as it is at `now`.
    pub(super) fn new(service: u64, now: Instant) -> Self {
        Self {
            service,
            agreed: false,
            worker: None,
            change: 0,
            changed_at: now,
            screen: 0,
            screen_at: now,
            asked: None,
            sent: None,
        }
    }

    /// The screen changed at `now`, more than a little.
    pub(super) fn changed(&mut self, now: Instant) {
        self.change = self.change.wrapping_add(1);
        self.changed_at = now;
        self.changed_a_little(now);
    }

    /// The screen changed a little at `now`: a clock or a caret.
    pub(super) fn changed_a_little(&mut self, now: Instant) {
        self.screen = self.screen.wrapping_add(1);
        self.screen_at = now;
    }

    /// Whether to ask for a still at `now`: agreed, no more than small
    /// changes for long enough, none sent since the last larger change, not
    /// asked for at this screen, the screen settled, and no keyframe owed.
    pub(super) fn due(&self, keyframe_pending: bool, now: Instant) -> bool {
        self.agreed
            && !keyframe_pending
            && self.sent != Some(self.change)
            && self.asked != Some((self.change, self.screen))
            && now.saturating_duration_since(self.changed_at) >= STILL_AFTER
            && now.saturating_duration_since(self.screen_at) >= STILL_SETTLE
    }

    pub(super) fn make(&mut self, frame: RawFrame, size: (u16, u16), budget: usize) {
        if self.worker.is_none() {
            self.worker = StillWorker::new().ok();
        }
        if self
            .worker
            .as_ref()
            .is_some_and(|worker| worker.make(self.screen, frame, size, budget))
        {
            tracing::debug!(
                target: vnilink::trace::STILL,
                service = self.service,
                budget,
                "still asked for"
            );
            self.asked = Some((self.change, self.screen));
        }
    }

    /// A still made since, unless the screen changed at all after it was
    /// asked for.
    pub(super) fn made(&mut self) -> Option<vnilink::EncodedFrame> {
        let made = self.worker.as_ref()?.take()?;
        self.accept(made)
    }

    /// `made` as the still to send, unless the screen changed at all after
    /// it was asked for or nothing worth sending fit its budget.
    fn accept(&mut self, made: Made) -> Option<vnilink::EncodedFrame> {
        if made.change != self.screen {
            tracing::debug!(
                target: vnilink::trace::STILL,
                service = self.service,
                "still discarded: the screen changed while it was made"
            );
            return None;
        }
        let Some(picture) = made.picture else {
            tracing::debug!(
                target: vnilink::trace::STILL,
                service = self.service,
                "still not sent: no better than the stream within its budget"
            );
            return None;
        };
        let still = vnilink::EncodedFrame::still(made.timestamp_us, picture).ok()?;
        tracing::debug!(
            target: vnilink::trace::STILL,
            service = self.service,
            bytes = still.payload.len(),
            timestamp_us = still.timestamp_us,
            "still made"
        );
        self.sent = Some(self.change);
        Some(still)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A screen that changed at `start` and then stayed, with stills agreed.
    fn agreed(start: Instant) -> Stills {
        let mut stills = Stills::new(7, start);
        stills.agreed = true;
        stills.changed(start);
        stills
    }

    fn made(stills: &Stills, picture: Option<Vec<u8>>) -> Made {
        Made {
            change: stills.screen,
            timestamp_us: 1,
            picture,
        }
    }

    #[test]
    fn a_still_is_due_once_the_screen_stays_unchanged_for_its_wait() {
        let start = Instant::now();
        let stills = agreed(start);
        assert!(
            !stills.due(false, start + STILL_AFTER - Duration::from_millis(1)),
            "the stream refines the screen until the wait is over"
        );
        assert!(
            stills.due(false, start + STILL_AFTER),
            "a screen unchanged for the wait is due its still"
        );
    }

    #[test]
    fn no_still_is_due_unless_the_viewer_agreed() {
        let start = Instant::now();
        let mut stills = agreed(start);
        stills.agreed = false;
        assert!(
            !stills.due(false, start + STILL_AFTER),
            "a viewer that did not agree gets no still"
        );
    }

    #[test]
    fn no_still_is_due_while_a_keyframe_is_owed() {
        let start = Instant::now();
        let stills = agreed(start);
        assert!(
            !stills.due(true, start + STILL_AFTER),
            "the stream's keyframe goes before any still"
        );
    }

    #[test]
    fn a_small_change_waits_for_the_screen_to_settle_but_not_the_whole_wait() {
        let start = Instant::now();
        let mut stills = agreed(start);
        let clock = start + STILL_AFTER;
        stills.changed_a_little(clock);
        assert!(
            !stills.due(false, clock + STILL_SETTLE - Duration::from_millis(1)),
            "a screen that just changed a little has not settled"
        );
        assert!(
            stills.due(false, clock + STILL_SETTLE),
            "a small change delays the still only until the screen settles"
        );
    }

    #[test]
    fn a_still_made_of_the_screen_as_it_is_is_sent_once_per_change() {
        let start = Instant::now();
        let mut stills = agreed(start);
        let made = made(&stills, Some(vec![7; 16]));
        let still = stills.accept(made).expect("a still of the screen as it is");
        assert_eq!(
            still.role,
            vnilink::FrameRole::Still,
            "what is sent is a still picture"
        );
        stills.changed_a_little(start + STILL_AFTER);
        assert!(
            !stills.due(false, start + STILL_AFTER * 2),
            "small changes after a still sent ask for no other"
        );
        stills.changed(start + STILL_AFTER * 2);
        assert!(
            stills.due(false, start + STILL_AFTER * 3),
            "a larger change earns the next still"
        );
    }

    #[test]
    fn a_still_made_while_the_screen_changed_is_discarded() {
        let start = Instant::now();
        let mut stills = agreed(start);
        let made = made(&stills, Some(vec![7; 16]));
        stills.changed_a_little(start + STILL_AFTER);
        assert!(
            stills.accept(made).is_none(),
            "a still of a screen since changed would hide what the stream replaced"
        );
        assert!(
            stills.due(false, start + STILL_AFTER + STILL_SETTLE),
            "the settled screen is due a still again"
        );
    }

    #[test]
    fn a_still_no_better_than_the_stream_is_not_sent() {
        let start = Instant::now();
        let mut stills = agreed(start);
        let made = made(&stills, None);
        assert!(
            stills.accept(made).is_none(),
            "nothing worth sending fit the budget"
        );
        assert!(
            stills.due(false, start + STILL_AFTER),
            "a still not sent leaves the screen due one"
        );
    }
}
