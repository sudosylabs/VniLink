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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum VideoPush {
    Queued,
    StillNeedsKeyframe,
    KeyframeNeeded,
}

/// Drops predicted frames after the queue fills, until a keyframe arrives.
pub(crate) struct VideoQueue {
    frames: VecDeque<EncodedFrame>,
    needs_keyframe: bool,
    dropped: u64,
}

impl VideoQueue {
    pub(crate) fn new() -> Self {
        Self {
            frames: VecDeque::new(),
            needs_keyframe: false,
            dropped: 0,
        }
    }

    pub(crate) fn push(&mut self, frame: EncodedFrame) -> VideoPush {
        if frame.keyframe {
            self.dropped = self.dropped.saturating_add(self.frames.len() as u64);
            self.frames.clear();
            self.needs_keyframe = false;
            self.frames.push_back(frame);
            return VideoPush::Queued;
        }
        if self.needs_keyframe {
            self.dropped = self.dropped.saturating_add(1);
            return VideoPush::StillNeedsKeyframe;
        }
        if self.frames.len() >= VIDEO_QUEUE {
            // A full queue drops predicted frames until the next keyframe. A
            // buffered keyframe stays: the network task may not have sent it,
            // and it is what the decoder restarts from.
            let dropped = self.frames.iter().filter(|frame| !frame.keyframe).count();
            self.dropped = self.dropped.saturating_add((dropped + 1) as u64);
            self.frames.retain(|frame| frame.keyframe);
            self.needs_keyframe = true;
            return VideoPush::KeyframeNeeded;
        }
        self.frames.push_back(frame);
        VideoPush::Queued
    }

    pub(crate) fn pop(&mut self) -> Option<EncodedFrame> {
        self.frames.pop_front()
    }

    pub(crate) fn drain(&mut self) -> Vec<EncodedFrame> {
        self.frames.drain(..).collect()
    }

    pub(crate) fn len(&self) -> usize {
        self.frames.len()
    }

    pub(crate) fn dropped(&self) -> u64 {
        self.dropped
    }
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

    pub(crate) fn notified(&self) -> tokio::sync::futures::Notified<'_> {
        self.notify.notified()
    }
}

/// Controller-side queue. The UI waits here. The network reader never waits on it.
pub(crate) struct VideoInbox {
    queue: Mutex<VideoQueue>,
    wake: Condvar,
}

impl VideoInbox {
    pub(crate) fn new() -> Self {
        Self {
            queue: Mutex::new(VideoQueue::new()),
            wake: Condvar::new(),
        }
    }

    pub(crate) fn push(&self, frame: EncodedFrame) -> VideoPush {
        let result = lock(&self.queue).push(frame);
        if matches!(result, VideoPush::Queued) {
            self.wake.notify_one();
        }
        result
    }

    pub(crate) fn reset(&self) {
        *lock(&self.queue) = VideoQueue::new();
    }

    pub(crate) fn try_pop(&self) -> Option<EncodedFrame> {
        lock(&self.queue).pop()
    }

    /// Frames waiting for the UI. Rising means the UI drains slower than the
    /// network delivers: used as the controller-side `queued` signal.
    pub(crate) fn len(&self) -> usize {
        lock(&self.queue).len()
    }

    pub(crate) fn pop_timeout(&self, timeout: Duration) -> Option<EncodedFrame> {
        let mut queue = lock(&self.queue);
        let deadline = Instant::now() + timeout;
        loop {
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

    pub(crate) fn notified(&self) -> tokio::sync::futures::Notified<'_> {
        self.notify.notified()
    }
}
