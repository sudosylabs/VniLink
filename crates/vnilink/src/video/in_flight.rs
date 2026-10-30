// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! Pictures written to a reliable stream that its viewer has not yet
//! acknowledged. A stream holds whatever is written to it, so on a link
//! slower than the pictures it would deliver them seconds late. A picture
//! is sent only while the oldest unacknowledged one is recent; otherwise it
//! waits in the sender's queue, which drops what grows old and starts again
//! from a keyframe, as the desktop's send gate does.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

/// How much longer than the link's round trip the oldest unacknowledged
/// picture may wait before no more are sent.
pub(crate) const PICTURE_LAG: Duration = Duration::from_millis(100);

/// The most pictures unacknowledged at once, whatever their age.
pub(crate) const MOST_IN_FLIGHT: usize = 32;

#[derive(Default)]
pub(crate) struct InFlight {
    /// Each unacknowledged picture's number and when it was written.
    sent: VecDeque<(u32, Instant)>,
    /// The fastest acknowledgement seen: the link's round trip at best.
    fastest: Option<Duration>,
}

impl InFlight {
    /// Whether a picture may be written at `now`.
    pub(crate) fn may_send(&self, now: Instant) -> bool {
        if self.sent.len() >= MOST_IN_FLIGHT {
            return false;
        }
        self.sent.front().is_none_or(|(_, at)| {
            now.saturating_duration_since(*at) <= self.fastest.unwrap_or_default() + PICTURE_LAG
        })
    }

    /// Picture `number` was written at `now`.
    pub(crate) fn sent(&mut self, number: u32, now: Instant) {
        self.sent.push_back((number, now));
    }

    /// The viewer received every picture up to `number`, which it said at
    /// `now`. Numbers wrap; one not sent, or already acknowledged, changes
    /// nothing.
    pub(crate) fn acknowledged(&mut self, number: u32, now: Instant) {
        let mut newest = None;
        while let Some(&(sent, at)) = self.sent.front() {
            // At or before `number`, in wrapping order.
            if (number.wrapping_sub(sent) as i32) < 0 {
                break;
            }
            newest = Some(at);
            self.sent.pop_front();
        }
        if let Some(at) = newest {
            let trip = now.saturating_duration_since(at);
            self.fastest = Some(self.fastest.map_or(trip, |fastest| fastest.min(trip)));
        }
    }

    /// How many pictures are unacknowledged.
    #[cfg(test)]
    fn len(&self) -> usize {
        self.sent.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(millis: u64) -> Duration {
        Duration::from_millis(millis)
    }

    #[test]
    fn pictures_go_while_the_oldest_unacknowledged_is_recent() {
        let start = Instant::now();
        let mut flight = InFlight::default();
        assert!(
            flight.may_send(start),
            "nothing in flight lets a picture go"
        );
        flight.sent(0, start);
        assert!(
            flight.may_send(start + PICTURE_LAG),
            "a picture just within the lag lets the next go"
        );
        assert!(
            !flight.may_send(start + PICTURE_LAG + ms(1)),
            "an older unacknowledged picture holds the next back"
        );
    }

    #[test]
    fn an_acknowledgement_frees_every_picture_up_to_it_and_learns_the_round_trip() {
        let start = Instant::now();
        let mut flight = InFlight::default();
        for number in 0..3 {
            flight.sent(number, start + ms(10 * u64::from(number)));
        }
        flight.acknowledged(1, start + ms(40));
        assert_eq!(
            flight.len(),
            1,
            "pictures 0 and 1 are acknowledged together"
        );
        // The round trip learnt is picture 1's, 30 ms, so picture 2 may wait
        // 30 ms more than the lag alone.
        let limit = start + ms(20) + ms(30) + PICTURE_LAG;
        assert!(
            flight.may_send(limit),
            "the link's round trip extends the wait"
        );
        assert!(
            !flight.may_send(limit + ms(1)),
            "past the round trip and the lag, pictures are held"
        );
    }

    #[test]
    fn a_stale_or_unknown_acknowledgement_changes_nothing() {
        let start = Instant::now();
        let mut flight = InFlight::default();
        flight.sent(5, start);
        flight.acknowledged(4, start + ms(1));
        assert_eq!(
            flight.len(),
            1,
            "an acknowledgement before the picture frees nothing"
        );
        assert!(
            !flight.may_send(start + PICTURE_LAG + ms(1)),
            "an acknowledgement that freed nothing teaches no round trip"
        );
    }

    #[test]
    fn numbers_wrap() {
        let start = Instant::now();
        let mut flight = InFlight::default();
        flight.sent(u32::MAX, start);
        flight.sent(0, start);
        flight.acknowledged(0, start + ms(5));
        assert_eq!(
            flight.len(),
            0,
            "an acknowledgement after the wrap frees both"
        );
    }

    #[test]
    fn no_more_than_a_bounded_number_is_unacknowledged() {
        let start = Instant::now();
        let mut flight = InFlight::default();
        for number in 0..MOST_IN_FLIGHT as u32 {
            flight.sent(number, start);
        }
        assert!(
            !flight.may_send(start),
            "a full flight holds pictures back however recent"
        );
    }
}
