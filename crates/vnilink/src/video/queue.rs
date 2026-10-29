//! Keyframe queue. A full queue drops predicted frames until the next keyframe.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use tokio::sync::Notify;

use super::frame::{EncodedFrame, FrameRole};

/// Keyframe plus seven predicted frames. The next predicted frame is dropped.
/// Sized to absorb encode/network jitter (~130ms at 60fps, ~260ms at 30fps)
/// without a keyframe request on every hiccup; sustained overload still drops
/// P-frames until the next keyframe to bound latency.
pub(crate) const VIDEO_QUEUE: usize = 8;

/// The viewer's frames may wait this long for the window. Past it, the
/// queue is a standing delay rather than jitter: it drops predicted frames
/// and starts again from a keyframe, whatever its depth.
pub(crate) const INBOX_MAX_AGE: Duration = Duration::from_millis(150);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum VideoPush {
    Queued,
    StillNeedsKeyframe,
    KeyframeNeeded,
    /// A still picture was left out: the queue was full.
    Skipped,
}

/// Drops predicted frames after the queue fills, until a keyframe arrives.
pub(crate) struct VideoQueue {
    frames: VecDeque<(EncodedFrame, Instant)>,
    received_frames: u64,
    received_bytes: u64,
    needs_keyframe: bool,
    dropped: u64,
    /// Oldest a waiting frame may be before the queue drops to a keyframe.
    max_age: Option<Duration>,
}

impl VideoQueue {
    pub(crate) fn new() -> Self {
        Self {
            frames: VecDeque::new(),
            received_frames: 0,
            received_bytes: 0,
            needs_keyframe: false,
            dropped: 0,
            max_age: None,
        }
    }

    /// Bounds the queue by the age of its oldest frame as well as its depth.
    pub(crate) fn with_max_age(mut self, max_age: Duration) -> Self {
        self.max_age = Some(max_age);
        self
    }

    pub(crate) fn push(&mut self, frame: EncodedFrame) -> VideoPush {
        self.push_at(frame, Instant::now())
    }
    pub(crate) fn push_at(&mut self, frame: EncodedFrame, now: Instant) -> VideoPush {
        self.received_frames = self.received_frames.saturating_add(1);
        self.received_bytes = self
            .received_bytes
            .saturating_add(frame.payload.len() as u64);
        // A still picture owes nothing: it neither answers nor clears the
        // keyframe the stream waits for, and a full queue leaves it out
        // rather than drop the stream's frames. A keyframe after it replaces
        // it uncounted. The host sends it only as the newest picture.
        if frame.role == FrameRole::Still {
            if self.frames.len() >= VIDEO_QUEUE {
                return VideoPush::Skipped;
            }
            self.frames.push_back((frame, now));
            return VideoPush::Queued;
        }
        if frame.keyframe {
            let replaced = self
                .frames
                .iter()
                .filter(|(frame, _)| frame.role != FrameRole::Still)
                .count();
            self.dropped = self.dropped.saturating_add(replaced as u64);
            self.frames.clear();
            self.needs_keyframe = false;
            self.frames.push_back((frame, now));
            return VideoPush::Queued;
        }
        if self.needs_keyframe {
            self.dropped = self.dropped.saturating_add(1);
            return VideoPush::StillNeedsKeyframe;
        }
        let stale = self.max_age.is_some_and(|max_age| {
            self.frames
                .front()
                .is_some_and(|(_, at)| now.saturating_duration_since(*at) > max_age)
        });
        if self.frames.len() >= VIDEO_QUEUE || stale {
            // A full queue drops predicted frames until the next keyframe. A
            // buffered keyframe stays: the network task may not have sent it,
            // and it is what the decoder restarts from.
            let dropped = self
                .frames
                .iter()
                .filter(|(frame, _)| !frame.keyframe)
                .count();
            self.dropped = self.dropped.saturating_add((dropped + 1) as u64);
            self.frames
                .retain(|(frame, _)| frame.keyframe && frame.role != FrameRole::Still);
            self.needs_keyframe = true;
            return VideoPush::KeyframeNeeded;
        }
        self.frames.push_back((frame, now));
        VideoPush::Queued
    }

    pub(crate) fn pop(&mut self) -> Option<EncodedFrame> {
        self.frames.pop_front().map(|(mut frame, at)| {
            frame.timing.queued_us = super::micros_since(at);
            frame
        })
    }

    /// A frame was lost on the way: the predicted frames after it decode
    /// against a missing picture, so they are dropped until a keyframe.
    pub(crate) fn lost(&mut self) {
        self.needs_keyframe = true;
    }

    pub(crate) fn drain(&mut self) -> Vec<EncodedFrame> {
        self.frames.drain(..).map(|(frame, _)| frame).collect()
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.frames.len()
    }

    pub(crate) fn dropped(&self) -> u64 {
        self.dropped
    }
    pub(crate) fn telemetry(&self, now: Instant) -> QueueTelemetry {
        QueueTelemetry {
            received_frames: self.received_frames,
            received_bytes: self.received_bytes,
            queued: self.frames.len() as u16,
            dropped: self.dropped,
            oldest_ms: self.frames.front().map_or(0, |(_, at)| {
                now.saturating_duration_since(*at)
                    .as_millis()
                    .min(u64::MAX as u128) as u64
            }),
        }
    }
}
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct QueueTelemetry {
    pub received_frames: u64,
    pub received_bytes: u64,
    pub queued: u16,
    pub dropped: u64,
    pub oldest_ms: u64,
}

/// Host-side queue. The network task drains it. A full queue does not block input.
pub(crate) struct VideoOutbox {
    queue: Mutex<VideoQueue>,
    notify: Notify,
}

fn lock(queue: &Mutex<VideoQueue>) -> std::sync::MutexGuard<'_, VideoQueue> {
    queue.lock().unwrap_or_else(|poison| poison.into_inner())
}

impl VideoOutbox {
    pub(crate) fn new() -> Self {
        Self {
            queue: Mutex::new(VideoQueue::new()),
            notify: Notify::new(),
        }
    }

    pub(crate) fn push(&self, frame: EncodedFrame) -> VideoPush {
        let result = lock(&self.queue).push(frame);
        if matches!(result, VideoPush::Queued) {
            self.notify.notify_one();
        }
        result
    }

    pub(crate) fn drain(&self) -> Vec<EncodedFrame> {
        lock(&self.queue).drain()
    }

    /// The oldest queued frame, for a sender that paces what it writes.
    pub(crate) fn pop(&self) -> Option<EncodedFrame> {
        lock(&self.queue).pop()
    }

    pub(crate) fn telemetry(&self) -> QueueTelemetry {
        lock(&self.queue).telemetry(Instant::now())
    }
    pub(crate) fn notified(&self) -> tokio::sync::futures::Notified<'_> {
        self.notify.notified()
    }
}

/// Controller-side queue. The UI waits here. The network reader never waits on it.
pub(crate) struct VideoInbox {
    queue: Mutex<VideoQueue>,
    wake: Condvar,
    closed: AtomicBool,
    taps: Mutex<Vec<std::sync::Weak<RecordingInbox>>>,
    /// Told when a frame is queued, so a reader need not poll.
    ready: Mutex<Option<Arc<ReadyWaker>>>,
}

/// Called, on the network task, each time a frame is queued for the reader.
/// It must return at once.
pub type ReadyWaker = dyn Fn() + Send + Sync;

fn inbox_queue() -> VideoQueue {
    VideoQueue::new().with_max_age(INBOX_MAX_AGE)
}

impl VideoInbox {
    pub(crate) fn new() -> Self {
        Self {
            queue: Mutex::new(inbox_queue()),
            wake: Condvar::new(),
            closed: AtomicBool::new(false),
            taps: Mutex::new(Vec::new()),
            ready: Mutex::new(None),
        }
    }

    pub(crate) fn set_ready(&self, waker: Option<Arc<ReadyWaker>>) {
        *self
            .ready
            .lock()
            .unwrap_or_else(|poison| poison.into_inner()) = waker;
    }

    pub(crate) fn push(&self, frame: EncodedFrame) -> VideoPush {
        // Recordings keep the stream's one codec: a still picture is shown,
        // never recorded.
        if !self.closed.load(Ordering::Acquire) && frame.role != FrameRole::Still {
            let mut taps = self.taps.lock().unwrap_or_else(|e| e.into_inner());
            taps.retain(|tap| tap.strong_count() > 0);
            if !taps.is_empty() {
                let shared = Arc::new(frame.clone());
                for tap in taps.iter().filter_map(std::sync::Weak::upgrade) {
                    tap.push(shared.clone());
                }
            }
        }
        let mut queue = lock(&self.queue);
        if self.closed.load(Ordering::Acquire) {
            return VideoPush::StillNeedsKeyframe;
        }
        let result = queue.push(frame);
        drop(queue);
        if matches!(result, VideoPush::Queued) {
            self.wake.notify_one();
            let ready = self
                .ready
                .lock()
                .unwrap_or_else(|poison| poison.into_inner())
                .clone();
            if let Some(ready) = ready {
                ready();
            }
        }
        result
    }

    /// A frame was lost on the way; see [`VideoQueue::lost`].
    pub(crate) fn lost(&self) {
        lock(&self.queue).lost();
    }

    pub(crate) fn close(&self) {
        self.set_ready(None);
        self.closed.store(true, Ordering::Release);
        self.close_taps();
        let mut queue = lock(&self.queue);
        *queue = inbox_queue();
        self.closed.store(true, Ordering::Release);
        self.wake.notify_all();
    }

    pub(crate) fn close_taps(&self) {
        let mut taps = self.taps.lock().unwrap_or_else(|e| e.into_inner());
        for tap in taps.drain(..).filter_map(|t| t.upgrade()) {
            tap.closed.store(true, Ordering::Release);
        }
    }
    pub(crate) fn recording_tap(&self) -> Result<RecordingVideoTap, crate::Error> {
        let mut taps = self.taps.lock().unwrap_or_else(|e| e.into_inner());
        taps.retain(|tap| tap.strong_count() > 0);
        if self.closed.load(Ordering::Acquire) {
            return Err(crate::Error::NoSession);
        }
        if taps.len() >= 4 {
            return Err(crate::Error::Busy);
        }
        let inbox = Arc::new(RecordingInbox {
            queue: Mutex::new(VecDeque::new()),
            started: AtomicBool::new(false),
            closed: AtomicBool::new(false),
            failed: AtomicBool::new(false),
        });
        taps.push(Arc::downgrade(&inbox));
        Ok(RecordingVideoTap { inbox })
    }

    pub(crate) fn try_pop(&self) -> Option<EncodedFrame> {
        lock(&self.queue).pop()
    }

    pub(crate) fn telemetry(&self) -> QueueTelemetry {
        lock(&self.queue).telemetry(Instant::now())
    }
    pub(crate) fn pop_timeout(&self, timeout: Duration) -> Option<EncodedFrame> {
        let mut queue = lock(&self.queue);
        let deadline = Instant::now() + timeout;
        loop {
            if self.closed.load(Ordering::Acquire) {
                return None;
            }
            if let Some(frame) = queue.pop() {
                return Some(frame);
            }
            let now = Instant::now();
            if now >= deadline {
                return None;
            }
            let (guard, status) = self
                .wake
                .wait_timeout(queue, deadline - now)
                .unwrap_or_else(|poison| poison.into_inner());
            queue = guard;
            if status.timed_out() {
                return queue.pop();
            }
        }
    }

    pub(crate) fn dropped(&self) -> u64 {
        lock(&self.queue).dropped()
    }
}

/// Encoded frames for the controlling session. One subscription at a time.
///
/// This is not a [`crate::CoreEvent`]. Frames are large, and a slow UI must not
/// share the event queue or block input. Drop the subscription to free the slot.
pub struct VideoSubscription {
    inbox: Arc<VideoInbox>,
    _alive: Arc<()>,
}

impl VideoSubscription {
    pub(crate) fn new(inbox: Arc<VideoInbox>, alive: Arc<()>) -> Self {
        Self {
            inbox,
            _alive: alive,
        }
    }

    pub fn try_recv(&self) -> Option<EncodedFrame> {
        self.inbox.try_pop()
    }

    /// Calls `waker` each time a frame is queued for this subscription,
    /// until the subscription is dropped, the stream closes or another
    /// waker replaces it. It runs on the network task and must return at
    /// once, typically by waking the reader.
    pub fn on_ready(&self, waker: impl Fn() + Send + Sync + 'static) {
        self.inbox.set_ready(Some(Arc::new(waker)));
    }

    pub fn recv_timeout(&self, timeout: Duration) -> Option<EncodedFrame> {
        self.inbox.pop_timeout(timeout)
    }

    /// Frames dropped because the queue was full or a keyframe replaced them.
    pub fn lagged(&self) -> u64 {
        self.inbox.dropped()
    }
}

impl Drop for VideoSubscription {
    fn drop(&mut self) {
        // Its reader is gone: nothing is left to wake.
        self.inbox.set_ready(None);
    }
}

/// Independent recording feed. Queue pressure ends only this feed; it never
/// steals decoder frames, requests keyframes repeatedly or queues disk work.
pub struct RecordingVideoTap {
    inbox: Arc<RecordingInbox>,
}
struct RecordingInbox {
    queue: Mutex<VecDeque<Arc<EncodedFrame>>>,
    started: AtomicBool,
    closed: AtomicBool,
    failed: AtomicBool,
}
impl RecordingInbox {
    fn push(&self, frame: Arc<EncodedFrame>) {
        let mut queue = self.queue.lock().unwrap_or_else(|e| e.into_inner());
        if self.closed.load(Ordering::Acquire) {
            return;
        }
        if !self.started.load(Ordering::Acquire) {
            if !frame.keyframe {
                return;
            }
            self.started.store(true, Ordering::Release);
        }
        if queue.len() >= 8 {
            queue.clear();
            self.failed.store(true, Ordering::Release);
            self.closed.store(true, Ordering::Release);
            return;
        }
        queue.push_back(frame);
    }
}
impl RecordingVideoTap {
    pub fn try_recv(&self) -> Option<Arc<EncodedFrame>> {
        self.inbox
            .queue
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .pop_front()
    }
    pub fn is_closed(&self) -> bool {
        self.inbox.closed.load(Ordering::Acquire)
    }
    pub fn has_failed(&self) -> bool {
        self.inbox.failed.load(Ordering::Acquire)
    }
}

/// Rising-edge request so the controller asks the host for one keyframe.
pub(crate) struct KeyframeFlag {
    wanted: AtomicBool,
    notify: Notify,
}

impl KeyframeFlag {
    pub(crate) fn new() -> Self {
        Self {
            wanted: AtomicBool::new(false),
            notify: Notify::new(),
        }
    }

    pub(crate) fn request(&self) {
        if !self.wanted.swap(true, Ordering::AcqRel) {
            self.notify.notify_one();
        }
    }

    pub(crate) fn take(&self) -> bool {
        self.wanted.swap(false, Ordering::AcqRel)
    }

    /// Observes a pending request without consuming it, for diagnostics.
    pub(crate) fn pending(&self) -> bool {
        self.wanted.load(Ordering::Acquire)
    }

    pub(crate) fn notified(&self) -> tokio::sync::futures::Notified<'_> {
        self.notify.notified()
    }
}

#[cfg(test)]
mod telemetry_tests {
    use super::*;
    #[test]
    fn queue_age_uses_local_arrival_and_counts_received_bytes_even_after_overload() {
        let mut queue = VideoQueue::new();
        let now = Instant::now();
        let frame = |keyframe| EncodedFrame::h264(keyframe, u64::MAX, vec![1, 2, 3]).unwrap();
        queue.push_at(frame(true), now);
        queue.push_at(frame(false), now + Duration::from_millis(20));
        let sample = queue.telemetry(now + Duration::from_millis(50));
        assert_eq!(
            sample,
            QueueTelemetry {
                received_frames: 2,
                received_bytes: 6,
                queued: 2,
                dropped: 0,
                oldest_ms: 50
            }
        );
        queue.pop().unwrap();
        assert_eq!(
            queue.telemetry(now + Duration::from_millis(50)).oldest_ms,
            30
        );
        for _ in 0..20 {
            queue.push_at(frame(false), now + Duration::from_millis(60));
        }
        let sample = queue.telemetry(now + Duration::from_millis(80));
        assert_eq!(sample.received_frames, 22);
        assert_eq!(sample.received_bytes, 66);
        assert!(sample.dropped > 0);
        assert!(sample.queued <= VIDEO_QUEUE as u16);
    }
}

#[cfg(test)]
mod recording_tests {
    use super::*;
    fn frame(keyframe: bool, timestamp: u64) -> EncodedFrame {
        EncodedFrame::h264(
            keyframe,
            timestamp,
            vec![0, 0, 0, 1, if keyframe { 0x65 } else { 0x41 }, 1],
        )
        .unwrap()
    }
    /// After a frame is lost on the way, predicted frames decode against a
    /// missing picture: they are dropped until the keyframe asked for.
    #[test]
    fn a_lost_frame_drops_predicted_frames_until_a_keyframe() {
        let inbox = VideoInbox::new();
        inbox.push(frame(true, 1));
        assert_eq!(inbox.try_pop().unwrap().timestamp_us, 1);
        inbox.lost();
        assert!(matches!(
            inbox.push(frame(false, 2)),
            VideoPush::StillNeedsKeyframe
        ));
        assert!(inbox.try_pop().is_none());
        assert!(matches!(inbox.push(frame(true, 3)), VideoPush::Queued));
        inbox.push(frame(false, 4));
        assert_eq!(inbox.try_pop().unwrap().timestamp_us, 3);
        assert_eq!(inbox.try_pop().unwrap().timestamp_us, 4);
    }

    #[test]
    fn recording_pressure_never_consumes_or_poison_viewer_frames() {
        let inbox = VideoInbox::new();
        let tap = inbox.recording_tap().unwrap();
        inbox.push(frame(false, 0));
        assert!(tap.try_recv().is_none());
        assert!(inbox.try_pop().is_some());
        for index in 1..=9 {
            inbox.push(frame(index == 1, index));
            assert_eq!(inbox.try_pop().unwrap().timestamp_us, index);
        }
        assert!(tap.has_failed());
        assert!(tap.is_closed());
        assert!(tap.try_recv().is_none());
        inbox.push(frame(false, 10));
        assert_eq!(inbox.try_pop().unwrap().timestamp_us, 10);
        assert_eq!(inbox.dropped(), 0);
    }
    #[test]
    fn recorder_limit_and_owner_close_release_independent_feeds() {
        let inbox = VideoInbox::new();
        let mut taps: Vec<_> = (0..4).map(|_| inbox.recording_tap().unwrap()).collect();
        assert!(matches!(inbox.recording_tap(), Err(crate::Error::Busy)));
        taps.pop();
        taps.push(inbox.recording_tap().unwrap());
        inbox.push(frame(true, 1));
        for tap in &taps {
            assert_eq!(tap.try_recv().unwrap().timestamp_us, 1);
        }
        assert_eq!(inbox.try_pop().unwrap().timestamp_us, 1);
        inbox.close();
        assert!(taps.iter().all(RecordingVideoTap::is_closed));
        assert!(matches!(
            inbox.recording_tap(),
            Err(crate::Error::NoSession)
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_still_waits_in_turn_and_owes_nothing() {
        let picture = |keyframe, at| EncodedFrame::h264(keyframe, at, vec![1]).unwrap();
        let still = |at| EncodedFrame::still(at, vec![2]).unwrap();
        let mut queue = VideoQueue::new();
        assert_eq!(queue.push(picture(true, 1)), VideoPush::Queued);
        assert_eq!(
            queue.push(still(2)),
            VideoPush::Queued,
            "behind the picture before it"
        );
        assert_eq!(queue.pop().unwrap().timestamp_us, 1);
        assert_eq!(queue.pop().unwrap().role, FrameRole::Still);
        // A full queue leaves a still out rather than drop the stream's frames.
        for at in 0..crate::video::VIDEO_QUEUE as u64 {
            queue.push(picture(at == 0, 10 + at));
        }
        assert_eq!(queue.push(still(30)), VideoPush::Skipped);
        while queue.pop().is_some() {}
        // A loss leaves the stream waiting for a keyframe; a still neither
        // answers it nor lets the predicted frames through.
        queue.lost();
        assert_eq!(queue.push(still(4)), VideoPush::Queued);
        assert_eq!(queue.push(picture(false, 5)), VideoPush::StillNeedsKeyframe);
        // A keyframe replaces a still waiting before it, uncounted.
        let dropped = queue.dropped();
        assert_eq!(queue.push(picture(true, 6)), VideoPush::Queued);
        assert_eq!(queue.dropped(), dropped);
        assert_eq!(queue.len(), 1);
        assert_eq!(queue.pop().unwrap().timestamp_us, 6);
    }

    #[test]
    fn a_still_is_shown_but_never_recorded() {
        let inbox = VideoInbox::new();
        let tap = inbox.recording_tap().unwrap();
        inbox.push(EncodedFrame::h264(true, 1, vec![1]).unwrap());
        inbox.push(EncodedFrame::still(2, vec![2]).unwrap());
        assert_eq!(inbox.try_pop().unwrap().timestamp_us, 1);
        assert_eq!(inbox.try_pop().unwrap().role, FrameRole::Still);
        let recorded: Vec<_> = std::iter::from_fn(|| tap.try_recv())
            .map(|frame| frame.timestamp_us)
            .collect();
        assert_eq!(recorded, [1]);
    }

    #[test]
    fn an_overflow_episode_requests_one_keyframe_until_it_arrives() {
        let mut queue = VideoQueue::new();
        assert_eq!(queue.push(frame(true, 1)), VideoPush::Queued);
        // Fill to the bound with predicted frames.
        for index in 0..VIDEO_QUEUE - 1 {
            assert_eq!(
                queue.push(frame(false, 2 + index as u64)),
                VideoPush::Queued
            );
        }
        // The next predicted frame overflows: exactly one keyframe request for
        // the whole episode, then silence until the keyframe arrives.
        assert_eq!(queue.push(frame(false, 99)), VideoPush::KeyframeNeeded);
        for timestamp in 100..110u64 {
            assert_eq!(
                queue.push(frame(false, timestamp)),
                VideoPush::StillNeedsKeyframe
            );
        }
        assert_eq!(queue.push(frame(true, 110)), VideoPush::Queued);
        assert_eq!(queue.pop().unwrap().timestamp_us, 110);
    }

    #[test]
    fn predicted_frames_drop_until_the_next_keyframe() {
        let mut queue = VideoQueue::new();
        assert_eq!(queue.push(frame(true, 1)), VideoPush::Queued);
        for index in 0..VIDEO_QUEUE - 1 {
            assert_eq!(
                queue.push(frame(false, 2 + index as u64)),
                VideoPush::Queued
            );
        }
        assert_eq!(queue.len(), VIDEO_QUEUE);
        assert_eq!(queue.push(frame(false, 99)), VideoPush::KeyframeNeeded);
        assert_eq!(queue.len(), 1);
        let kept = queue.pop().unwrap();
        assert!(kept.keyframe);
        assert_eq!(kept.timestamp_us, 1);
        assert!(queue.dropped() >= 1);
        assert_eq!(queue.push(frame(false, 100)), VideoPush::StillNeedsKeyframe);
        assert_eq!(queue.push(frame(true, 101)), VideoPush::Queued);
        assert_eq!(queue.len(), 1);
        let dropped = queue.dropped();
        assert_eq!(queue.push(frame(true, 102)), VideoPush::Queued);
        assert!(queue.dropped() > dropped);
        assert_eq!(queue.len(), 1);
        let only = queue.pop().unwrap();
        assert!(only.keyframe);
        assert_eq!(only.timestamp_us, 102);
        assert!(queue.pop().is_none());
    }

    #[test]
    fn a_viewer_queue_older_than_its_budget_starts_again_from_a_keyframe() {
        let start = Instant::now();
        let frame = |keyframe| EncodedFrame::h264(keyframe, 1, vec![0x41]).unwrap();
        let mut queue = VideoQueue::new().with_max_age(crate::video::INBOX_MAX_AGE);
        assert_eq!(queue.push_at(frame(true), start), VideoPush::Queued);
        assert_eq!(
            queue.push_at(frame(false), start + Duration::from_millis(100)),
            VideoPush::Queued,
            "two frames within the budget are jitter, not delay"
        );
        // The window has not taken the first frame for longer than the budget:
        // the predicted frames go, and a keyframe is wanted.
        assert_eq!(
            queue.push_at(frame(false), start + Duration::from_millis(151)),
            VideoPush::KeyframeNeeded
        );
        assert_eq!(queue.len(), 1, "the buffered keyframe stays");
        assert_eq!(
            queue.push_at(frame(true), start + Duration::from_millis(160)),
            VideoPush::Queued
        );
        // A queue without a budget is bounded by depth only.
        let mut deep = VideoQueue::new();
        deep.push_at(frame(true), start);
        assert_eq!(
            deep.push_at(frame(false), start + Duration::from_secs(5)),
            VideoPush::Queued
        );
    }

    /// A reader told when a frame is queued need not poll; once it is gone, the
    /// network task calls nothing.
    #[test]
    fn a_subscription_is_woken_when_a_frame_is_queued_until_it_is_dropped() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let inbox = Arc::new(crate::video::VideoInbox::new());
        let subscription = crate::VideoSubscription::new(Arc::clone(&inbox), Arc::new(()));
        let woken = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&woken);
        subscription.on_ready(move || {
            counter.fetch_add(1, Ordering::Relaxed);
        });
        let frame = |keyframe| EncodedFrame::h264(keyframe, 1, vec![0x65]).unwrap();
        inbox.push(frame(true));
        assert_eq!(woken.load(Ordering::Relaxed), 1);
        assert!(subscription.try_recv().is_some());
        // A frame dropped while a keyframe is owed wakes no one.
        inbox.lost();
        inbox.push(frame(false));
        assert_eq!(woken.load(Ordering::Relaxed), 1);
        drop(subscription);
        inbox.push(frame(true));
        assert_eq!(
            woken.load(Ordering::Relaxed),
            1,
            "a dropped reader was woken"
        );
    }

    fn frame(keyframe: bool, timestamp_us: u64) -> EncodedFrame {
        EncodedFrame::h264(keyframe, timestamp_us, vec![1]).unwrap()
    }
}
