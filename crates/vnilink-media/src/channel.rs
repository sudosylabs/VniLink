//! Latest-frame delivery from a platform capture thread.
//!
//! The platform pushes each copied picture; the serve loop receives the newest
//! one it has not seen yet. Pushes overwrite, so a slow consumer skips to the
//! latest instead of queuing stale pictures. The first failure sticks and is
//! reported to every later receive. The bytes are screen content: do not log
//! them.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::error::Error;

/// One copied picture, still in the capture stride. The platform converts it
/// to a [`crate::RawFrame`] after receiving.
#[derive(Debug, Clone)]
pub struct ChannelSlot {
    pub width: u16,
    pub height: u16,
    pub stride: usize,
    pub bytes: Vec<u8>,
}

/// Newest-picture slot shared between a capture thread and its serve loop.
pub struct FrameChannel {
    slot: Mutex<Option<ChannelSlot>>,
    generation: AtomicU64,
    returned: AtomicU64,
    failure: Mutex<Option<&'static str>>,
    /// When `push` last stored a picture. Idle readers do not refresh this.
    pushed_at: Mutex<Option<Instant>>,
}

impl FrameChannel {
    pub fn new() -> Self {
        Self {
            slot: Mutex::new(None),
            generation: AtomicU64::new(0),
            returned: AtomicU64::new(0),
            failure: Mutex::new(None),
            pushed_at: Mutex::new(None),
        }
    }

    /// Store `slot` as the latest picture. Overwrites whatever is waiting.
    pub fn push(&self, slot: ChannelSlot) {
        *self.slot.lock().expect("frame channel") = Some(slot);
        *self.pushed_at.lock().expect("frame channel") = Some(Instant::now());
        self.generation.fetch_add(1, Ordering::Relaxed);
    }

    /// Acquisition time of the last `push`. A later read of the same slot does not move it.
    pub fn pushed_at(&self) -> Option<Instant> {
        *self.pushed_at.lock().expect("frame channel")
    }

    /// Record a permanent capture failure. The first reason wins; later
    /// pushes do not clear it.
    pub fn fail(&self, reason: &'static str) {
        let mut failure = self.failure.lock().expect("frame channel");
        if failure.is_none() {
            *failure = Some(reason);
        }
    }

    /// The recorded failure, if any. The open loop polls this while the
    /// capture negotiates its format.
    pub fn failure(&self) -> Option<&'static str> {
        *self.failure.lock().expect("frame channel")
    }

    /// Return cached pixels without waiting. The caller must pump its native
    /// capture events first so a cached picture cannot hide newer pixels or errors.
    pub fn try_latest(&self) -> Result<Option<ChannelSlot>, Error> {
        if let Some(reason) = self.failure() {
            return Err(Error::Unsupported(reason));
        }
        Ok(self.slot.lock().expect("frame channel").clone())
    }

    /// Pump capture events and return the current picture, even if unchanged.
    /// Damage-driven compositors need not send a new frame for an idle desktop.
    /// Still wait for the first picture and propagate permanent failures.
    pub fn recv_latest(
        &self,
        timeout: Duration,
        mut wait: impl FnMut(),
    ) -> Result<ChannelSlot, Error> {
        // Always pump events so cached pixels cannot hide a failed capture or
        // prevent the next changed picture from reaching the slot.
        wait();
        if let Some(slot) = self.try_latest()? {
            return Ok(slot);
        }
        self.recv_timeout(timeout, wait)
    }

    /// Block until a picture newer than the last returned one arrives.
    ///
    /// `wait` runs between checks: the PipeWire capture pumps its main loop
    /// there, while ScreenCaptureKit only sleeps. A failed capture reports
    /// [`Error::Unsupported`]; an empty wait reports [`Error::CaptureRejected`].
    pub fn recv_timeout(
        &self,
        timeout: Duration,
        mut wait: impl FnMut(),
    ) -> Result<ChannelSlot, Error> {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(reason) = self.failure() {
                return Err(Error::Unsupported(reason));
            }
            let generation = self.generation.load(Ordering::Relaxed);
            if generation > self.returned.load(Ordering::Relaxed) {
                if let Some(slot) = self.slot.lock().expect("frame channel").clone() {
                    self.returned.store(generation, Ordering::Relaxed);
                    return Ok(slot);
                }
            }
            if Instant::now() >= deadline {
                return Err(Error::CaptureRejected(
                    "the capture did not deliver a frame",
                ));
            }
            wait();
        }
    }
}

impl Default for FrameChannel {
    fn default() -> Self {
        Self::new()
    }
}
