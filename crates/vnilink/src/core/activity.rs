// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! Owner-local user activity. Media/keepalive and unchanged pointer reports
//! never call this clock; deadlines use monotonic time only.
use crate::InputEvent;
use std::time::{Duration, Instant};
pub(super) struct Activity {
    last: Instant,
    pointer: Option<(i32, i32, u64)>,
}
impl Activity {
    pub(super) fn new(now: Instant) -> Self {
        Self {
            last: now,
            pointer: None,
        }
    }
    pub(super) fn input(&mut self, event: InputEvent, now: Instant) {
        match event {
            InputEvent::Pointer { x, y, display, .. } => {
                let position = (x, y, display);
                if self.pointer == Some(position) {
                    return;
                }
                self.pointer = Some(position);
            }
            InputEvent::Wheel { dx: 0, dy: 0 }
            | InputEvent::PreciseWheel { dx: 0, dy: 0 }
            | InputEvent::Motion { dx: 0, dy: 0 } => return,
            _ => {}
        }
        self.touch(now);
    }
    pub(super) fn touch(&mut self, now: Instant) {
        self.last = self.last.max(now);
    }
    pub(super) fn expired(&self, now: Instant, limit_secs: u32) -> bool {
        limit_secs != 0
            && now.saturating_duration_since(self.last) >= Duration::from_secs(limit_secs.into())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unchanged_pointer_zero_motion_and_backward_clock_do_not_extend_deadline() {
        let start = Instant::now();
        let mut activity = Activity::new(start);
        activity.input(InputEvent::pointer(1, 2, 3, 1), start);
        activity.input(
            InputEvent::pointer(1, 2, 3, 2),
            start + Duration::from_secs(8),
        );
        activity.input(InputEvent::wheel(0, 0), start + Duration::from_secs(9));
        activity.touch(start - Duration::from_secs(1));
        assert!(!activity.expired(start + Duration::from_secs(9), 10));
        assert!(activity.expired(start + Duration::from_secs(10), 10));
        activity.input(
            InputEvent::pointer(2, 2, 3, 2),
            start + Duration::from_secs(10),
        );
        assert!(!activity.expired(start + Duration::from_secs(11), 10));
        assert!(!activity.expired(start + Duration::from_secs(100), 0));
    }
}
