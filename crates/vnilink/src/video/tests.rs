// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! Video tests: what a still picture may cost, and viewer keyframe recovery
//! under datagram loss.
//!
//! These tests pin what the receive path does, so a later change to keyframe
//! recovery shows up as a changed count. `ViewerPath` drives the
//! [`ViewerPictures`] a desktop owner keeps, as `route_desktop_datagrams`,
//! `DesktopRuntime::receive_video` and `client_send` in
//! `core/desktop_stream.rs` do, with the inbox and keyframe flag beside it:
//! every datagram is followed by a client send pass. `Host` answers a request
//! the way the fanout does: the encoder's next frame after the request
//! arrives is a keyframe, however many requests are waiting.
//!
//! Loss is a seeded generator over fragments and time is simulated frame by
//! frame, so every count below is exact and repeatable. `before 51bcf1c2`
//! notes give the counts from before the viewer kept one request
//! outstanding.

use std::time::{Duration, Instant};

use super::{
    decode_video_body, encode_video_body, fragment, EncodedFrame, KeyframeFlag, Losses, Order,
    Reassembler, VideoBody, VideoInbox, ViewerPictures, VIDEO_QUEUE,
};

/// Fragment payload budget. The host passes `max_datagram_size()` minus the
/// 20-byte session prefix; 1,150 bytes approximates that budget at the
/// initial QUIC MTU. Larger budgets change the counts, not the shape.
const DATAGRAM: usize = 1_150;

/// The default offer: 2 Mb/s at 30 frames per second.
const FPS: u64 = 30;
const FRAME_PERIOD: Duration = Duration::from_nanos(1_000_000_000 / FPS);
const P_FRAME: usize = 2_000_000 / 8 / FPS as usize;

/// Keyframes cost about ten predicted frames of screen content.
const KEYFRAME: usize = P_FRAME * 10;

/// VideoToolbox keyframe interval (`crates/vnilink-macos/src/encode.rs`).
const GOP: u64 = 120;

/// The harness's impaired profiles keep a 20 ms round trip, so a request
/// reaches the host and its keyframe comes back within one frame.
const RTT: Duration = Duration::from_millis(20);
const ANSWER_FRAMES: u64 = 1;

/// Deterministic xorshift64*, so a seed always drops the same fragments.
struct Loss {
    state: u64,
    per_million: u64,
}

impl Loss {
    fn new(seed: u64, percent: f64) -> Self {
        Self {
            state: seed.max(1),
            per_million: (percent * 10_000.0) as u64,
        }
    }

    fn drops(&mut self) -> bool {
        self.state ^= self.state >> 12;
        self.state ^= self.state << 25;
        self.state ^= self.state >> 27;
        let value = self.state.wrapping_mul(0x2545_F491_4F6C_DD1D);
        value % 1_000_000 < self.per_million
    }
}

/// Frame `index` carries its index as the timestamp so tests can tell which
/// frames reached the decoder.
fn frame(index: u64, keyframe: bool) -> EncodedFrame {
    let size = if keyframe { KEYFRAME } else { P_FRAME };
    EncodedFrame::h264(keyframe, index, vec![0x41; size]).unwrap()
}

fn datagrams(index: u64, keyframe: bool) -> Vec<Vec<u8>> {
    fragment(
        &encode_video_body(&frame(index, keyframe), index as u32),
        index as u32,
        DATAGRAM,
        0.0,
    )
}

/// The viewer half of the desktop datagram path.
struct ViewerPath {
    pictures: ViewerPictures,
    inbox: VideoInbox,
    keyframe: KeyframeFlag,
    losses: Losses,
    requests: u64,
}

impl ViewerPath {
    fn new() -> Self {
        Self {
            pictures: ViewerPictures::new(),
            inbox: VideoInbox::new(),
            keyframe: KeyframeFlag::new(),
            losses: Losses::default(),
            requests: 0,
        }
    }

    /// One datagram at `now`, followed by a client send pass.
    fn receive(&mut self, datagram: &[u8], now: Instant) {
        let arrival = self.pictures.datagram(datagram, now);
        self.losses.expired += arrival.unfinished.expired;
        self.losses.evicted += arrival.unfinished.evicted;
        if let Some(body) = arrival.body {
            if let Ok(body) = decode_video_body(&body) {
                self.picture(body);
            }
        }
        self.pass(now);
    }

    /// A picture by datagrams or on the stream, as `receive_video` takes it.
    fn picture(&mut self, body: VideoBody) {
        match self
            .pictures
            .picture(body.sequence, body.frame, &self.inbox, &self.keyframe)
        {
            Order::Next => {}
            Order::Gap(missing) => self.losses.gap += u64::from(missing),
            Order::Stale => self.losses.stale += 1,
        }
    }

    /// The client send loop: also runs when its retry time comes round.
    fn pass(&mut self, now: Instant) {
        if self
            .pictures
            .should_ask(&self.keyframe, now, RTT, FRAME_PERIOD)
        {
            self.requests += 1;
        }
    }

    fn drain(&self) -> Vec<u64> {
        std::iter::from_fn(|| self.inbox.try_pop())
            .map(|frame| frame.timestamp_us)
            .collect()
    }
}

/// The encoder side: a periodic keyframe, plus one for every frame on which
/// at least one request has arrived.
struct Host {
    due: Option<u64>,
    keyframes: u64,
    bytes: usize,
    keyframe_bytes: usize,
}

impl Host {
    fn new() -> Self {
        Self {
            due: None,
            keyframes: 0,
            bytes: 0,
            keyframe_bytes: 0,
        }
    }

    fn requested(&mut self, at: u64) {
        let due = at + ANSWER_FRAMES;
        self.due = Some(self.due.map_or(due, |current| current.min(due)));
    }

    fn encode(&mut self, index: u64) -> Vec<Vec<u8>> {
        let keyframe = index.is_multiple_of(GOP) || self.due.is_some_and(|due| due <= index);
        if keyframe {
            self.due = None;
            self.keyframes += 1;
        }
        let parts = datagrams(index, keyframe);
        let size: usize = parts.iter().map(Vec::len).sum();
        self.bytes += size;
        if keyframe {
            self.keyframe_bytes += size;
        }
        parts
    }
}

/// Streams `frames` frames; `deliver` decides which datagrams arrive.
fn stream(
    frames: u64,
    mut deliver: impl FnMut(u64, usize) -> bool,
) -> (ViewerPath, Host, Vec<u64>, u64) {
    let start = Instant::now();
    let mut viewer = ViewerPath::new();
    let mut host = Host::new();
    let mut shown = Vec::new();
    let (mut frozen, mut longest_frozen) = (0, 0);
    for index in 0..frames {
        let now = start + FRAME_PERIOD * index as u32;
        let before = viewer.requests;
        for (position, part) in host.encode(index).iter().enumerate() {
            if deliver(index, position) {
                viewer.receive(part, now);
            }
        }
        viewer.pass(now);
        if viewer.requests > before {
            host.requested(index);
        }
        let delivered = viewer.drain();
        frozen = if delivered.is_empty() { frozen + 1 } else { 0 };
        longest_frozen = longest_frozen.max(frozen);
        shown.extend(delivered);
    }
    (viewer, host, shown, longest_frozen)
}

#[test]
fn a_lost_fragment_asks_once_and_holds_predicted_frames_until_the_keyframe() {
    let (viewer, host, shown, _) = stream(GOP, |index, position| (index, position) != (5, 1));
    // Frame 6 completing past the hole reports frame 5 lost. Frame 6 is held
    // back with it, and frame 7 is the keyframe the host owes.
    assert_eq!(
        viewer.losses,
        Losses {
            gap: 1,
            ..Losses::default()
        },
        "the hole is one frame skipped by a newer one"
    );
    assert_eq!(viewer.requests, 1, "one loss asks one keyframe");
    assert_eq!(host.keyframes, 2, "the periodic keyframe and the answer");
    let expected: Vec<u64> = (0..5).chain(7..GOP).collect();
    assert_eq!(
        shown, expected,
        "predicted frames after the loss wait for the keyframe"
    );
}

#[test]
fn a_lost_keyframe_is_asked_for_once_until_patience_runs_out() {
    // The answering keyframe of a loss at frame 5 loses a fragment too, and
    // so does its first retry: every loss in between wants the same keyframe.
    let lost = [(5, 1), (7, 3), (10, 3)];
    let (viewer, host, shown, _) =
        stream(GOP, |index, position| !lost.contains(&(index, position)));
    // Patience is RTT plus two frames, about 87 ms. Requests go at frames 6,
    // 9 and 12; the losses at frames 8 and 11 fall inside it and ask nothing.
    // The third answer, at frame 13, gets through. Before 51bcf1c2 the losses
    // at frames 8 and 11 asked at once.
    assert_eq!(viewer.requests, 3, "losses within patience ask nothing");
    assert_eq!(host.keyframes, 4, "every request is answered");
    assert_eq!(shown.first(), Some(&0), "the first keyframe is shown");
    assert_eq!(
        shown.iter().find(|&&index| index > 4),
        Some(&13),
        "the picture resumes at the first keyframe that arrives whole"
    );
}

#[test]
fn a_lost_first_keyframe_is_asked_for_by_the_picture_after_it() {
    let start = Instant::now();
    let mut viewer = ViewerPath::new();
    let mut lost = datagrams(0, true);
    lost.pop();
    for part in &lost {
        viewer.receive(part, start);
    }
    let mut shown = Vec::new();
    for index in 1..30 {
        for part in &datagrams(index, false) {
            viewer.receive(part, start);
        }
        shown.extend(viewer.drain());
    }
    // A stream starts at picture 0, so picture 1 says the keyframe is
    // missing, and predicted frames without a reference wait for one.
    assert_eq!(viewer.losses.gap, 1, "picture 1 reports picture 0 missing");
    assert_eq!(viewer.requests, 1, "the gap asks one keyframe at once");
    assert_eq!(
        shown,
        Vec::<u64>::new(),
        "no predicted frame without its reference"
    );
    assert_eq!(
        viewer.losses.expired, 0,
        "the partial keyframe went when picture 1 completed, not by silence"
    );
    for part in &datagrams(30, true) {
        viewer.receive(part, start);
    }
    assert_eq!(viewer.drain(), vec![30], "the answering keyframe is shown");
}

/// A still the host sends between stream pictures repeats the number of
/// the one before it, so losing it leaves the stream whole.
#[test]
fn a_lost_still_asks_no_keyframe() {
    let start = Instant::now();
    let mut viewer = ViewerPath::new();
    let mut shown = Vec::new();
    for index in 0..5 {
        for part in &datagrams(index, index == 0) {
            viewer.receive(part, start);
        }
        shown.extend(viewer.drain());
    }
    // The still takes the next datagram frame (fragment sequence 5) and
    // repeats picture 4; one of its fragments is lost.
    let still = EncodedFrame::still(100, vec![0x0A; KEYFRAME]).unwrap();
    let mut parts = fragment(&encode_video_body(&still, 4), 5, DATAGRAM, 0.0);
    parts.remove(1);
    for part in &parts {
        viewer.receive(part, start);
    }
    // Pictures 5 onwards ride fragment sequences 6 onwards.
    for index in 5..10 {
        let body = encode_video_body(&frame(index, false), index as u32);
        for part in &fragment(&body, index as u32 + 1, DATAGRAM, 0.0) {
            viewer.receive(part, start);
        }
        shown.extend(viewer.drain());
    }
    assert_eq!(viewer.losses.gap, 0, "a lost still leaves no gap");
    assert_eq!(viewer.requests, 0, "a lost still asks no keyframe");
    assert_eq!(
        shown,
        (0..10).collect::<Vec<_>>(),
        "the stream goes on past the lost still"
    );
}

#[test]
fn queue_overflow_asks_once_then_again_after_patience() {
    let start = Instant::now();
    let mut viewer = ViewerPath::new();
    for part in &datagrams(0, true) {
        viewer.receive(part, start);
    }
    // The UI stops draining: the inbox holds the keyframe plus seven
    // predicted frames, and the next predicted frame overflows it.
    for index in 1..VIDEO_QUEUE as u64 + 20 {
        for part in &datagrams(index, false) {
            viewer.receive(part, start);
        }
    }
    assert_eq!(viewer.requests, 1, "an overflow asks one keyframe");
    assert_eq!(viewer.drain(), vec![0], "the buffered keyframe stays");
    // No keyframe has come back: the request is asked again once a round
    // trip and two frames have passed, and not before.
    viewer.pass(start + Duration::from_millis(80));
    assert_eq!(viewer.requests, 1, "not asked again within patience");
    viewer.pass(start + Duration::from_millis(90));
    assert_eq!(viewer.requests, 2, "asked again once patience runs out");

    // The flag is a rising edge: requests raised before the client send
    // loop takes it are one wanted keyframe.
    let flag = KeyframeFlag::new();
    flag.request();
    flag.request();
    assert!(flag.take(), "a raised flag is taken");
    assert!(!flag.take(), "two raises are one wanted keyframe");
}

/// One loss rate over 30 seconds of the default stream.
#[derive(Debug, PartialEq)]
struct LossRun {
    keyframes_sent: u64,
    keyframe_share: f64,
    requests: u64,
    shown: u64,
    longest_frozen: u64,
}

fn run_loss(percent: f64, seed: u64) -> LossRun {
    let mut loss = Loss::new(seed, percent);
    let (viewer, host, shown, longest_frozen) = stream(30 * FPS, |_, _| !loss.drops());
    LossRun {
        keyframes_sent: host.keyframes,
        keyframe_share: host.keyframe_bytes as f64 / host.bytes as f64,
        requests: viewer.requests,
        shown: shown.len() as u64,
        longest_frozen,
    }
}

#[test]
fn loss_asks_for_one_keyframe_at_a_time() {
    let runs: Vec<(f64, LossRun)> = [0.5, 1.0, 2.0, 5.0]
        .into_iter()
        .map(|percent| (percent, run_loss(percent, 0x5EED)))
        .collect();
    for (percent, run) in &runs {
        eprintln!("loss {percent}%: {run:?}");
    }
    let by_percent = |wanted: f64| &runs.iter().find(|(p, _)| *p == wanted).unwrap().1;
    let periodic = 30 * FPS / GOP + 1;
    for (percent, run) in &runs {
        // Every request is answered; a request can fall on a periodic one.
        assert!(run.requests > 0, "loss {percent}% asks for keyframes");
        assert!(
            (run.requests..=periodic + run.requests).contains(&run.keyframes_sent),
            "loss {percent}%: every request is answered: {run:?}"
        );
    }
    // A keyframe spans about 74 datagrams, so under loss it is often lost
    // itself, and that, not repeated asking, drives the storm. At a 20 ms
    // round trip an answer lands within a frame, so one outstanding request
    // saves few: 2% loss sends 236 keyframes (257 before 51bcf1c2) and they
    // are still most of the bytes. It shows fewer frames, 257 against 340,
    // because a lost keyframe now waits a round trip and two frames before
    // it is asked again. A smaller keyframe after a loss (capped QP) is the
    // planned next step; it should move these counts.
    let two = by_percent(2.0);
    assert!(
        (200..257).contains(&two.keyframes_sent),
        "fewer keyframes than one per loss: {two:?}"
    );
    assert!(
        two.keyframe_share > 0.7,
        "keyframes are most bytes: {two:?}"
    );
    assert!(two.shown < 340, "a lost keyframe waits patience: {two:?}");
    let five = by_percent(5.0);
    assert!(
        five.keyframe_share > 0.8,
        "keyframes are most bytes: {five:?}"
    );
    assert!(
        five.shown < 30 * FPS / 10,
        "5% loss shows under a tenth: {five:?}"
    );
    assert_eq!(
        run_loss(2.0, 0x5EED),
        *two,
        "the same seed reproduces the same run"
    );
}

#[test]
fn an_incomplete_frame_is_evicted_after_eight_newer_partials() {
    let mut reassembler = Reassembler::new();
    let mut oldest = datagrams(0, false);
    let last = oldest.pop().unwrap();
    for part in &oldest {
        assert!(reassembler.push(part).is_none(), "the frame is incomplete");
    }
    // A loss burst leaves eight newer frames incomplete as well.
    for index in 1..=8 {
        let parts = datagrams(index, false);
        assert!(
            reassembler.push(&parts[0]).is_none(),
            "one fragment of many"
        );
    }
    assert_eq!(
        reassembler.take_unfinished().evicted,
        1,
        "the ninth partial evicts the oldest"
    );
    // The ninth partial evicted frame 0, so its late fragment opens a fresh
    // partial that can never complete.
    assert!(
        reassembler.push(&last).is_none(),
        "an evicted frame's late fragment completes nothing"
    );
}

#[test]
fn an_incomplete_frame_expires_after_a_fixed_ttl() {
    let start = Instant::now();
    let mut reassembler = Reassembler::new();
    let mut parts = datagrams(0, false);
    let last = parts.pop().unwrap();
    for part in &parts {
        assert!(
            reassembler.push_at(part, start).is_none(),
            "the frame is incomplete"
        );
    }
    // The 500 ms lifetime is fixed and ignores the path RTT, so a
    // retransmitted or reordered tail after that point is wasted.
    assert!(
        reassembler
            .push_at(&last, start + Duration::from_millis(550))
            .is_none(),
        "a tail after the silence completes nothing"
    );
    assert_eq!(
        reassembler.take_unfinished().expired,
        1,
        "silence expires the frame"
    );
}

#[test]
fn a_picture_overtaken_is_a_gap_and_discarded_when_it_arrives_late() {
    let start = Instant::now();
    let mut viewer = ViewerPath::new();
    let mut shown = Vec::new();
    for index in 0..10 {
        for part in &datagrams(index, index == 0) {
            viewer.receive(part, start);
        }
        shown.extend(viewer.drain());
    }
    assert_eq!(shown, (0..10).collect::<Vec<_>>(), "the stream so far");
    // Keyframe 10 is delayed whole behind pictures 11 and 12.
    for index in 11..=12 {
        for part in &datagrams(index, false) {
            viewer.receive(part, start);
        }
    }
    assert_eq!(viewer.losses.gap, 1, "picture 11 reports 10 missing");
    assert_eq!(viewer.requests, 1, "the gap asks one keyframe");
    for part in &datagrams(10, true) {
        viewer.receive(part, start);
    }
    // It completes after newer pictures, so it is stale: discarded, and
    // its gap already asked for the keyframe the stream needs.
    assert_eq!(
        viewer.losses.stale, 1,
        "a picture after a later one is stale"
    );
    assert_eq!(viewer.requests, 1, "a stale picture asks nothing more");
    for part in &datagrams(13, false) {
        viewer.receive(part, start);
    }
    assert_eq!(
        viewer.drain(),
        Vec::<u64>::new(),
        "predicted frames after the gap wait for the keyframe"
    );
}

#[test]
fn a_still_fits_what_the_link_drains_in_a_moment() {
    // 8 Mb/s carries 250 kB in the quarter second a still may hold it.
    assert_eq!(
        crate::video::still_drain(8_000_000),
        250_000,
        "a still takes what the link drains in 250 ms"
    );
}

/// A still is made to the size the send gate admits: its datagrams, their
/// prefixes and its parity included. Sized to the drain itself, every still
/// on a 12 Mb/s link was refused (374,989 encoded bytes, 422,786 to 557,116
/// on the wire, against 375,000).
#[test]
fn a_still_made_to_its_budget_is_not_refused_for_its_framing() {
    let cost = |payload: usize, max: usize, protection: f64| {
        let (datagrams, bytes) =
            crate::video::fragmented_size(crate::video::VIDEO_HEADER + payload, max, protection);
        bytes + datagrams * 20
    };
    let wire = crate::video::still_drain(12_000_000);
    for max in [1180, 1432] {
        for protection in [0.1, 0.33, crate::video::MAX_PROTECTION] {
            let payload = crate::video::still_payload(wire, max, 20, protection);
            assert!(
                cost(payload, max, protection) <= wire,
                "the still fits its budget"
            );
            assert!(
                cost(payload + 1, max, protection) > wire,
                "the largest that fits"
            );
            assert!(payload < wire, "framing costs some of the budget");
        }
    }
    // A link too slow for any picture takes none, and none exceeds a frame.
    assert_eq!(
        crate::video::still_payload(10, 1180, 20, 0.1),
        0,
        "a link too slow for any picture takes none"
    );
    assert!(
        crate::video::still_payload(usize::MAX / 4, 1180, 20, 0.0) <= crate::MAX_VIDEO_FRAME,
        "no still exceeds a frame"
    );
}
