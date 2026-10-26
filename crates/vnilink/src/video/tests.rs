//! Video tests: what a still picture may cost, and viewer keyframe recovery
//! under datagram loss.
//!
//! These tests pin what the receive path does, so a later change to keyframe
//! recovery shows up as a changed count. `ViewerPath` composes the same steps
//! as `route_desktop_datagrams`, `DesktopRuntime::receive_video` and
//! `client_send` in `core/desktop_stream.rs`:
//!
//! - each datagram goes through the [`Reassembler`]; any loss it reports
//!   marks the [`VideoInbox`] lost and raises the [`KeyframeFlag`];
//! - a complete body is pushed; a queue overflow raises the flag, and a
//!   keyframe answers the outstanding request;
//! - the client send pass turns the flag into a wanted keyframe and sends a
//!   request only when [`KeyframeRequests`] allows: one outstanding, asked
//!   again after a round trip and two frames.
//!
//! Replace this copy with the router itself if it is ever extracted from the
//! connection task. `Host` answers a request the way the fanout does: the
//! encoder's next frame after the request arrives is a keyframe, however many
//! requests are waiting.
//!
//! Loss is a seeded generator over fragments and time is simulated frame by
//! frame, so every count below is exact and repeatable. `before 51bcf1c2`
//! notes give the counts from before the viewer kept one request
//! outstanding.

use std::time::{Duration, Instant};

use super::{
    decode_video_body, encode_video_body, fragment, EncodedFrame, KeyframeFlag, KeyframeRequests,
    Losses, Reassembler, VideoInbox, VideoPush, VIDEO_QUEUE,
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
        &encode_video_body(&frame(index, keyframe)),
        index as u32,
        DATAGRAM,
        0.0,
    )
}

/// The viewer half of the desktop datagram path.
struct ViewerPath {
    reassembler: Reassembler,
    inbox: VideoInbox,
    keyframe: KeyframeFlag,
    outstanding: KeyframeRequests,
    losses: Losses,
    requests: u64,
}

impl ViewerPath {
    fn new() -> Self {
        Self {
            reassembler: Reassembler::new(),
            inbox: VideoInbox::new(),
            keyframe: KeyframeFlag::new(),
            outstanding: KeyframeRequests::default(),
            losses: Losses::default(),
            requests: 0,
        }
    }

    /// One datagram at `now`, followed by a client send pass.
    fn receive(&mut self, datagram: &[u8], now: Instant) {
        let body = self.reassembler.push(datagram);
        let losses = self.reassembler.take_loss();
        if losses.any() {
            self.losses.expired += losses.expired;
            self.losses.evicted += losses.evicted;
            self.losses.gap += losses.gap;
            self.losses.stale += losses.stale;
            self.inbox.lost();
            self.keyframe.request();
        }
        if let Some(body) = body {
            if let Ok(frame) = decode_video_body(&body) {
                let keyframe = frame.keyframe;
                if matches!(self.inbox.push(frame), VideoPush::KeyframeNeeded) {
                    self.keyframe.request();
                }
                if keyframe {
                    self.outstanding.keyframe_arrived();
                }
            }
        }
        self.pass(now);
    }

    /// The client send loop: also runs when its retry time comes round.
    fn pass(&mut self, now: Instant) {
        if self.keyframe.take() {
            self.outstanding.want();
        }
        if self.outstanding.should_send(now, RTT, FRAME_PERIOD) {
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
        }
    );
    assert_eq!(viewer.requests, 1);
    assert_eq!(host.keyframes, 2);
    let expected: Vec<u64> = (0..5).chain(7..GOP).collect();
    assert_eq!(shown, expected);
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
    assert_eq!(viewer.requests, 3);
    assert_eq!(host.keyframes, 4);
    assert_eq!(shown.first(), Some(&0));
    assert_eq!(shown.iter().find(|&&index| index > 4), Some(&13));
}

#[test]
fn a_lost_first_keyframe_is_noticed_only_when_its_partial_expires() {
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
    // The first completed frame starts the sequence, so the missing keyframe
    // is no gap, and predicted frames without a reference reach the decoder.
    assert_eq!(shown, (1..30).collect::<Vec<_>>());
    assert_eq!(viewer.requests, 0);
    // The reassembler's real 500 ms lifetime ends the partial keyframe, and
    // only then is a keyframe asked for.
    std::thread::sleep(Duration::from_millis(550));
    viewer.receive(&datagrams(30, false)[0], start);
    assert_eq!(viewer.losses.expired, 1);
    assert_eq!(viewer.requests, 1);
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
    assert_eq!(viewer.requests, 1);
    assert_eq!(viewer.drain(), vec![0], "the buffered keyframe stays");
    // No keyframe has come back: the request is asked again once a round
    // trip and two frames have passed, and not before.
    viewer.pass(start + Duration::from_millis(80));
    assert_eq!(viewer.requests, 1);
    viewer.pass(start + Duration::from_millis(90));
    assert_eq!(viewer.requests, 2);

    // The flag is a rising edge: requests raised before the client send
    // loop takes it are one wanted keyframe.
    let flag = KeyframeFlag::new();
    flag.request();
    flag.request();
    assert!(flag.take());
    assert!(!flag.take());
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
        assert!(run.requests > 0, "loss {percent}%");
        assert!(
            (run.requests..=periodic + run.requests).contains(&run.keyframes_sent),
            "loss {percent}%: {run:?}"
        );
    }
    // A keyframe spans about 74 datagrams, so under loss it is often lost
    // itself, and that, not repeated asking, drives the storm. At a 20 ms
    // round trip an answer lands within a frame, so one outstanding request
    // saves few: 2% loss sends 240 keyframes (257 before 51bcf1c2) and they
    // are still most of the bytes. It shows fewer frames, 254 against 340,
    // because a lost keyframe now waits a round trip and two frames before
    // it is asked again. A smaller keyframe after a loss (capped QP) is the
    // planned next step; it should move these counts.
    let two = by_percent(2.0);
    assert!((200..257).contains(&two.keyframes_sent), "{two:?}");
    assert!(two.keyframe_share > 0.7, "{two:?}");
    assert!(two.shown < 340, "{two:?}");
    let five = by_percent(5.0);
    assert!(five.keyframe_share > 0.8, "{five:?}");
    assert!(five.shown < 30 * FPS / 10, "{five:?}");
    // The same seed reproduces the same run.
    assert_eq!(run_loss(2.0, 0x5EED), *two);
}

#[test]
fn an_incomplete_frame_is_evicted_after_eight_newer_partials() {
    let mut reassembler = Reassembler::new();
    let mut oldest = datagrams(0, false);
    let last = oldest.pop().unwrap();
    for part in &oldest {
        assert!(reassembler.push(part).is_none());
    }
    // A loss burst leaves eight newer frames incomplete as well.
    for index in 1..=8 {
        let parts = datagrams(index, false);
        assert!(reassembler.push(&parts[0]).is_none());
    }
    assert_eq!(reassembler.take_loss().evicted, 1);
    // The ninth partial evicted frame 0, so its late fragment opens a fresh
    // partial that can never complete.
    assert!(reassembler.push(&last).is_none());
}

#[test]
fn an_incomplete_frame_expires_after_a_fixed_ttl() {
    let mut reassembler = Reassembler::new();
    let mut parts = datagrams(0, false);
    let last = parts.pop().unwrap();
    for part in &parts {
        assert!(reassembler.push(part).is_none());
    }
    // The 500 ms lifetime is fixed and ignores the path RTT, so a
    // retransmitted or reordered tail after that point is wasted.
    std::thread::sleep(Duration::from_millis(550));
    assert!(reassembler.push(&last).is_none());
    assert_eq!(reassembler.take_loss().expired, 1);
}

#[test]
fn a_late_keyframe_is_stale_and_asked_for_again() {
    let start = Instant::now();
    let mut viewer = ViewerPath::new();
    let mut keyframe = datagrams(10, true);
    let tail = keyframe.pop().unwrap();
    for part in &keyframe {
        viewer.receive(part, start);
    }
    for index in 11..=12 {
        for part in &datagrams(index, false) {
            viewer.receive(part, start);
        }
    }
    viewer.receive(&tail, start);
    // The reordered keyframe completes after newer frames, so it is stale:
    // dropped, and a fresh keyframe is asked for. Predicted frames already
    // queued still reach the decoder; later ones wait for that keyframe.
    assert_eq!(viewer.losses.stale, 1);
    assert_eq!(viewer.requests, 1);
    for part in &datagrams(13, false) {
        viewer.receive(part, start);
    }
    assert_eq!(viewer.drain(), vec![11, 12]);
}

#[test]
fn a_still_fits_what_the_link_drains_in_a_moment() {
    // 8 Mb/s carries 250 kB in the quarter second a still may hold it.
    assert_eq!(crate::video::still_drain(8_000_000), 250_000);
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
            assert!(cost(payload, max, protection) <= wire);
            assert!(
                cost(payload + 1, max, protection) > wire,
                "the largest that fits"
            );
            assert!(payload < wire);
        }
    }
    // A link too slow for any picture takes none, and none exceeds a frame.
    assert_eq!(crate::video::still_payload(10, 1180, 20, 0.1), 0);
    assert!(crate::video::still_payload(usize::MAX / 4, 1180, 20, 0.0) <= crate::MAX_VIDEO_FRAME);
}
