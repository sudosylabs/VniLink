// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! Latest-frame delivery from a platform capture thread.
//!
//! The platform pushes each copied picture; the serve loop receives the newest
//! one it has not seen yet. Pushes overwrite, so a slow consumer skips to the
//! latest instead of queuing stale pictures. The first failure sticks and is
//! reported to every later receive. The bytes are screen content: do not log
//! them.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Condvar, Mutex};
use std::time::{Duration, Instant};

use crate::error::Error;

/// One copied picture, still in the capture stride. The platform converts it
/// to a [`crate::RawFrame`] after receiving. The bytes are shared, so taking
/// the latest picture again does not copy it.
#[derive(Debug, Clone)]
pub struct ChannelSlot {
    pub width: u16,
    pub height: u16,
    pub stride: usize,
    pub bytes: crate::Pixels,
    /// When the OS captured the picture, if the platform says.
    pub captured_at: Option<Instant>,
    /// Where the screen changed since the picture the platform delivered
    /// before; `None` when it cannot say.
    pub changed: Option<crate::Changes>,
}

impl ChannelSlot {
    /// Whether `other` has this picture's size and pixels, whatever either
    /// stride. Only each row's pixels are compared, not its padding.
    fn same_picture(&self, other: &Self) -> bool {
        if (self.width, self.height) != (other.width, other.height) {
            return false;
        }
        let row = usize::from(self.width) * 4;
        (0..usize::from(self.height)).all(|y| {
            match (
                self.bytes.get(y * self.stride..y * self.stride + row),
                other.bytes.get(y * other.stride..y * other.stride + row),
            ) {
                (Some(left), Some(right)) => left == right,
                _ => false,
            }
        })
    }
}

/// Newest-picture slot shared between a capture thread and its serve loop.
pub struct FrameChannel {
    slot: Mutex<Option<ChannelSlot>>,
    generation: AtomicU64,
    returned: AtomicU64,
    failure: Mutex<Option<&'static str>>,
    /// When `push` last stored a picture. Idle readers do not refresh this.
    pushed_at: Mutex<Option<Instant>>,
    /// Where the pictures stored since the last [`Self::take_changed`]
    /// changed the screen; `None` once one could not say.
    changed: Mutex<Option<crate::Changes>>,
    /// Wakes [`Self::wait_newer`] when a picture is stored or a failure is
    /// recorded. The mutex guards nothing: the state it announces is atomic
    /// or has its own lock, and `generation` is advanced under it so a wait
    /// cannot miss a push between its check and its sleep.
    arrival: (Mutex<()>, Condvar),
}

impl FrameChannel {
    pub fn new() -> Self {
        Self {
            slot: Mutex::new(None),
            generation: AtomicU64::new(0),
            returned: AtomicU64::new(0),
            failure: Mutex::new(None),
            pushed_at: Mutex::new(None),
            changed: Mutex::new(None),
            arrival: (Mutex::new(()), Condvar::new()),
        }
    }

    /// Store `slot` as the latest picture. Overwrites whatever is waiting.
    pub fn push(&self, slot: ChannelSlot) {
        self.add_changed(slot.changed.clone());
        *self.slot.lock().expect("frame channel") = Some(slot);
        self.stored();
    }

    /// Store `slot` unless it shows the same picture as the one stored, and
    /// say whether it was stored. A compositor may hand over an unchanged
    /// screen again, at the full frame rate while anything else on it moves;
    /// that is no change, and must not move [`Self::pushed_at`].
    pub fn push_if_changed(&self, slot: ChannelSlot) -> bool {
        let mut stored = self.slot.lock().expect("frame channel");
        if stored.as_ref().is_some_and(|last| last.same_picture(&slot)) {
            return false;
        }
        self.add_changed(slot.changed.clone());
        *stored = Some(slot);
        drop(stored);
        self.stored();
        true
    }

    fn add_changed(&self, after: Option<crate::Changes>) {
        let mut changed = self.changed.lock().expect("frame channel");
        *changed = changed
            .take()
            .zip(after)
            .map(|(before, after)| before.join(after));
    }

    /// Where the pictures stored since the last call changed the screen;
    /// `None` when one could not say, or before the first call. The reader
    /// takes it with each picture it reads.
    pub fn take_changed(&self) -> Option<crate::Changes> {
        self.changed
            .lock()
            .expect("frame channel")
            .replace(crate::Changes::none())
    }

    /// Forgets the stored picture, as when the capture changes size: the
    /// next read waits for a picture of the new one. A failure stays.
    pub fn clear(&self) {
        *self.slot.lock().expect("frame channel") = None;
    }

    fn stored(&self) {
        *self.pushed_at.lock().expect("frame channel") = Some(Instant::now());
        let _arrival = self.arrival.0.lock().expect("frame channel");
        self.generation.fetch_add(1, Ordering::Release);
        self.arrival.1.notify_all();
    }

    /// How many pictures have been stored. A picture stored after a call
    /// to this has a greater number.
    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::Acquire)
    }

    /// Wait until a picture newer than `seen` is stored, or `deadline`, and
    /// return the generation then current. The result is `seen` when nothing
    /// arrived in time. A recorded failure ends the wait at once, so the
    /// caller's next read reports it. A push wakes the waiter directly.
    pub fn wait_newer(&self, seen: u64, deadline: Instant) -> u64 {
        let mut guard = self.arrival.0.lock().expect("frame channel");
        loop {
            let generation = self.generation();
            if generation > seen || self.failure().is_some() {
                return generation;
            }
            let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
                return generation;
            };
            guard = self
                .arrival
                .1
                .wait_timeout(guard, remaining)
                .expect("frame channel")
                .0;
        }
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
        drop(failure);
        let _arrival = self.arrival.0.lock().expect("frame channel");
        self.arrival.1.notify_all();
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
