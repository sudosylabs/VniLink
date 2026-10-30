// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! The viewer's receive side for one desktop owner: datagrams to pictures,
//! which pictures were lost on the way, and when to ask the host for a
//! keyframe.
//!
//! Its owner holds it behind one lock: the datagram router feeds it
//! fragments, both transports hand it numbered pictures, and the client send
//! loop asks it whether to send a keyframe request. It does no I/O and takes
//! the time from its caller, so the same steps run on a simulated clock in
//! `video/tests.rs`.

use std::time::{Duration, Instant};

use super::{
    EncodedFrame, FragmentCounts, FrameRole, KeyframeFlag, KeyframeRequests, Reassembler,
    Unfinished, VideoInbox, VideoPush,
};

/// Where a picture's number falls among the stream pictures seen so far.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Order {
    /// In order: no stream picture before it is missing.
    Next,
    /// This many stream pictures before it never arrived.
    Gap(u32),
    /// A later stream picture arrived first: too late to decode.
    Stale,
}

/// The stream pictures a viewer has seen, by the numbers their host gave
/// them (see [`super::VideoBody`]). Both transports carry the same numbers,
/// so a picture lost on either shows as a gap.
#[derive(Debug, Default)]
pub(crate) struct PictureSequence {
    /// The number the next stream picture should carry. A stream starts at
    /// zero, so a lost first picture is a gap as well.
    expected: u32,
}

impl PictureSequence {
    /// Where `sequence` falls; unless it is stale, the sequence moves past
    /// it. A still repeats the number of the stream picture before it, so
    /// in order it moves the sequence nowhere.
    pub(crate) fn check(&mut self, sequence: u32, still: bool) -> Order {
        // The number of the stream picture this one comes after, plus one.
        let follows = if still {
            sequence.wrapping_add(1)
        } else {
            sequence
        };
        let ahead = follows.wrapping_sub(self.expected);
        if ahead > u32::MAX / 2 {
            return Order::Stale;
        }
        self.expected = if still {
            follows
        } else {
            follows.wrapping_add(1)
        };
        match ahead {
            0 => Order::Next,
            missing => Order::Gap(missing),
        }
    }
}

/// What one datagram did.
#[derive(Debug, Default)]
pub(crate) struct Arrival {
    /// The body this datagram completed.
    pub(crate) body: Option<Vec<u8>>,
    /// Fragments seen, missed and rebuilt, for the loss report.
    pub(crate) fragments: FragmentCounts,
    /// Frames the reassembler gave up, for diagnostics.
    pub(crate) unfinished: Unfinished,
}

pub(crate) struct ViewerPictures {
    reassembler: Reassembler,
    sequence: PictureSequence,
    requests: KeyframeRequests,
}

impl ViewerPictures {
    pub(crate) fn new() -> Self {
        Self {
            reassembler: Reassembler::new(),
            sequence: PictureSequence::default(),
            requests: KeyframeRequests::default(),
        }
    }

    /// One datagram arrived at `now`. A frame given up asks nothing: the
    /// picture after it says whether a stream picture is missing.
    pub(crate) fn datagram(&mut self, fragment: &[u8], now: Instant) -> Arrival {
        let body = self.reassembler.push_at(fragment, now);
        Arrival {
            body,
            fragments: self.reassembler.take_fragments(),
            unfinished: self.reassembler.take_unfinished(),
        }
    }

    /// Picture `sequence` arrived, by datagrams or on the stream. After a
    /// gap what follows has no reference: `inbox` waits for a keyframe, and
    /// one is wanted now rather than at the next periodic one. A stale
    /// picture is discarded; its gap already asked. A keyframe of the
    /// stream answers the outstanding request; a full queue wants one.
    pub(crate) fn picture(
        &mut self,
        sequence: u32,
        frame: EncodedFrame,
        inbox: &VideoInbox,
        wanted: &KeyframeFlag,
    ) -> Order {
        // A still picture is not the stream's keyframe: it answers no request.
        let still = frame.role == FrameRole::Still;
        let order = self.sequence.check(sequence, still);
        match order {
            Order::Stale => return order,
            Order::Gap(_) => {
                inbox.lost();
                wanted.request();
            }
            Order::Next => {}
        }
        let keyframe = frame.keyframe && !still;
        if matches!(inbox.push(frame), VideoPush::KeyframeNeeded) {
            wanted.request();
        }
        if keyframe {
            self.requests.keyframe_arrived();
        }
        order
    }

    /// Whether the client send loop sends a keyframe request at `now`:
    /// `wanted` raised since it last looked, or a request unanswered past
    /// a round trip and two frames.
    pub(crate) fn should_ask(
        &mut self,
        wanted: &KeyframeFlag,
        now: Instant,
        rtt: Duration,
        frame: Duration,
    ) -> bool {
        if wanted.take() {
            self.requests.want();
        }
        self.requests.should_send(now, rtt, frame)
    }

    /// When to look again for an unanswered request, if one is owed.
    pub(crate) fn retry_at(&self, rtt: Duration, frame: Duration) -> Option<Instant> {
        self.requests.retry_at(rtt, frame)
    }

    /// Whether a keyframe is wanted and has not arrived.
    pub(crate) fn keyframe_pending(&self) -> bool {
        self.requests.outstanding()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn consecutive_pictures_are_in_order() {
        let mut sequence = PictureSequence::default();
        for number in 0..4 {
            assert_eq!(
                sequence.check(number, false),
                Order::Next,
                "picture {number} follows the one before"
            );
        }
    }

    #[test]
    fn a_skipped_picture_is_a_gap_and_the_sequence_moves_past_it() {
        let mut sequence = PictureSequence::default();
        assert_eq!(sequence.check(0, false), Order::Next, "the first picture");
        assert_eq!(
            sequence.check(3, false),
            Order::Gap(2),
            "pictures 1 and 2 never arrived"
        );
        assert_eq!(
            sequence.check(4, false),
            Order::Next,
            "the picture after the gap is in order again"
        );
    }

    #[test]
    fn a_lost_first_picture_is_a_gap() {
        let mut sequence = PictureSequence::default();
        assert_eq!(
            sequence.check(1, false),
            Order::Gap(1),
            "a stream starts at zero"
        );
    }

    #[test]
    fn a_picture_after_a_later_one_is_stale_and_moves_nothing() {
        let mut sequence = PictureSequence::default();
        assert_eq!(sequence.check(0, false), Order::Next, "the first picture");
        assert_eq!(sequence.check(2, false), Order::Gap(1), "1 is overtaken");
        assert_eq!(sequence.check(1, false), Order::Stale, "1 arrives late");
        assert_eq!(sequence.check(2, false), Order::Stale, "2 arrives twice");
        assert_eq!(
            sequence.check(3, false),
            Order::Next,
            "a stale picture leaves the sequence where it was"
        );
    }

    #[test]
    fn a_still_repeats_the_last_number_and_leaves_no_gap() {
        let mut sequence = PictureSequence::default();
        assert_eq!(
            sequence.check(u32::MAX, true),
            Order::Next,
            "a still before any picture repeats the number before zero"
        );
        assert_eq!(sequence.check(0, false), Order::Next, "the first picture");
        assert_eq!(sequence.check(0, true), Order::Next, "a still after it");
        assert_eq!(sequence.check(0, true), Order::Next, "and another");
        assert_eq!(
            sequence.check(1, false),
            Order::Next,
            "the next picture follows whatever stills came between"
        );
    }

    #[test]
    fn a_lost_still_leaves_no_gap() {
        let mut sequence = PictureSequence::default();
        assert_eq!(sequence.check(0, false), Order::Next, "the first picture");
        // A still repeating 0 went missing here.
        assert_eq!(
            sequence.check(1, false),
            Order::Next,
            "a still takes no number of its own"
        );
    }

    #[test]
    fn a_still_after_a_lost_picture_is_a_gap_and_one_overtaken_is_stale() {
        let mut sequence = PictureSequence::default();
        assert_eq!(sequence.check(0, false), Order::Next, "the first picture");
        assert_eq!(
            sequence.check(1, true),
            Order::Gap(1),
            "the still follows picture 1, which never arrived"
        );
        assert_eq!(sequence.check(2, false), Order::Next, "2 follows the still");
        assert_eq!(
            sequence.check(1, true),
            Order::Stale,
            "a still from before picture 2 is too late to show"
        );
    }

    #[test]
    fn the_sequence_wraps() {
        let mut sequence = PictureSequence {
            expected: u32::MAX - 1,
        };
        assert_eq!(
            sequence.check(u32::MAX - 1, false),
            Order::Next,
            "before the wrap"
        );
        assert_eq!(
            sequence.check(u32::MAX, false),
            Order::Next,
            "the last number"
        );
        assert_eq!(
            sequence.check(u32::MAX, true),
            Order::Next,
            "a still repeating it"
        );
        assert_eq!(
            sequence.check(0, false),
            Order::Next,
            "zero follows the last number"
        );
        assert_eq!(sequence.check(2, false), Order::Gap(1), "a gap across zero");
        assert_eq!(
            sequence.check(u32::MAX, false),
            Order::Stale,
            "a picture from before the wrap is stale"
        );
    }
}
