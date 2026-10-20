//! Keyframe queue. A full queue drops predicted frames until the next keyframe.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use tokio::sync::Notify;

use super::frame::EncodedFrame;

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
        if frame.keyframe {
            self.dropped = self.dropped.saturating_add(self.frames.len() as u64);
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
            self.frames.retain(|(frame, _)| frame.keyframe);
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

    /// Queued frames waiting for the network task. Poll with
    /// `session_paths` RTT/relay to drive adaptive quality.
    pub(crate) fn queued(&self) -> usize {
        lock(&self.queue).len()
    }

    /// Cumulative P-frames dropped on full queue. Rising fast means the
    /// encoder outruns the path: lower bitrate/fps via quality update.
    pub(crate) fn dropped(&self) -> u64 {
        lock(&self.queue).dropped()
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
}

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
        }
    }

    pub(crate) fn push(&self, frame: EncodedFrame) -> VideoPush {
        if !self.closed.load(Ordering::Acquire) {
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
        if matches!(result, VideoPush::Queued) {
            self.wake.notify_one();
        }
        result
    }

    /// A frame was lost on the way; see [`VideoQueue::lost`].
    pub(crate) fn lost(&self) {
        lock(&self.queue).lost();
    }

    pub(crate) fn reset(&self) {
        self.closed.store(true, Ordering::Release);
        self.close_taps();
        let mut queue = lock(&self.queue);
        *queue = inbox_queue();
        self.closed.store(false, Ordering::Release);
    }

    pub(crate) fn close(&self) {
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

    /// Frames waiting for the UI. Rising means the UI drains slower than the
    /// network delivers: used as the controller-side `queued` signal.
    pub(crate) fn len(&self) -> usize {
        lock(&self.queue).len()
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

    pub fn recv_timeout(&self, timeout: Duration) -> Option<EncodedFrame> {
        self.inbox.pop_timeout(timeout)
    }

    /// Frames dropped because the queue was full or a keyframe replaced them.
    pub fn lagged(&self) -> u64 {
        self.inbox.dropped()
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
        inbox.reset();
        assert!(inbox.recording_tap().is_ok());
        assert!(taps.iter().all(RecordingVideoTap::is_closed));
    }
}
