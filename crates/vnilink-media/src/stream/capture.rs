//! The shared capture's reader: it alone reads the source, at the pace the
//! fastest consumer wants, and leaves the newest picture in one slot each
//! consumer takes from.
// trace: records every ignored failure

use std::sync::{Condvar, Mutex};
use std::time::{Duration, Instant};

use vnilink::{Core, CursorState, VideoTarget};

use crate::error::PublishError;
use crate::frame::{encoded_size, RawFrame};
use crate::{FrameSource, FrameWait};

#[derive(Default)]
struct Picture {
    generation: u64,
    frame: Option<RawFrame>,
    cursor: Option<CursorState>,
    capture_us: u64,
    captured_at: Option<Instant>,
    /// The source attested the same screen as the picture before.
    unchanged: bool,
}
pub(super) struct PictureSlot {
    picture: Mutex<Picture>,
    wake: Condvar,
}
impl PictureSlot {
    pub(super) fn new() -> Self {
        Self {
            picture: Mutex::new(Picture::default()),
            wake: Condvar::new(),
        }
    }
    fn publish(
        &self,
        frame: RawFrame,
        cursor: Option<CursorState>,
        capture_us: u64,
        captured_at: Instant,
        unchanged: bool,
    ) {
        let mut picture = self
            .picture
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        picture.frame = Some(frame);
        picture.cursor = cursor;
        picture.capture_us = capture_us;
        picture.captured_at = Some(captured_at);
        picture.unchanged = unchanged;
        picture.generation = picture.generation.wrapping_add(1).max(1);
        drop(picture);
        self.wake.notify_all();
    }
    /// One coherent newest sample. The cursor cannot advance independently
    /// between cloning this picture and reading its generation.
    pub(super) fn wait_newer(&self, seen: u64, timeout: Duration) -> Option<Sample> {
        let picture = self
            .picture
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        let ready = |picture: &Picture| {
            picture.generation != 0 && picture.generation != seen && picture.frame.is_some()
        };
        let picture = if ready(&picture) {
            picture
        } else {
            self.wake
                .wait_timeout(picture, timeout)
                .unwrap_or_else(|poison| poison.into_inner())
                .0
        };
        if !ready(&picture) {
            return None;
        }
        Some(Sample {
            generation: picture.generation,
            frame: picture.frame.clone()?,
            cursor: picture.cursor.clone(),
            capture_us: picture.capture_us,
            captured_at: picture.captured_at,
            unchanged: picture.unchanged,
        })
    }
    /// The newest picture, whether or not this consumer has seen it.
    pub(super) fn latest(&self) -> Option<Sample> {
        let picture = self
            .picture
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        Some(Sample {
            generation: picture.generation,
            frame: picture.frame.clone()?,
            cursor: picture.cursor.clone(),
            capture_us: picture.capture_us,
            captured_at: picture.captured_at,
            unchanged: picture.unchanged,
        })
    }
    pub(super) fn poke(&self) {
        self.wake.notify_all();
    }
    /// Forgets the picture, as when the source changes size: consumers
    /// wait for one of the new size.
    pub(super) fn clear(&self) {
        self.picture
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .frame = None;
    }
}

/// The size to ask a source for: the largest any of `targets` encodes a
/// picture of `full` at, or `None` without a target.
pub(super) fn capture_target(full: (u16, u16), targets: &[VideoTarget]) -> Option<(u16, u16)> {
    targets
        .iter()
        .filter_map(|target| {
            encoded_size(
                full.0,
                full.1,
                target.quality.max_width,
                target.quality.max_height,
            )
            .ok()
        })
        .max_by_key(|(width, height)| u32::from(*width) * u32::from(*height))
}

/// The newest picture as one consumer takes it.
pub(super) struct Sample {
    pub(super) generation: u64,
    pub(super) frame: RawFrame,
    pub(super) cursor: Option<CursorState>,
    pub(super) capture_us: u64,
    pub(super) captured_at: Option<Instant>,
    pub(super) unchanged: bool,
}

/// How often an unchanged picture is still handed on, so a viewer sees the
/// stream is alive.
const IDLE_KEEPALIVE: Duration = Duration::from_secs(1);
/// The longest the shared capture sleeps or waits before it looks at its
/// stop flag, the routing and the consumers again.
const WAKE_SLICE: Duration = Duration::from_millis(20);

/// Older than this, a source's capture time is not trusted over the read.
const CAPTURE_AGE_LIMIT: Duration = Duration::from_secs(1);

/// When a newly read picture was captured: the OS's time when the source
/// gives a plausible one, otherwise when it was read. The host stage of a
/// frame's latency starts here.
fn capture_time(captured_at: Option<Instant>, read_at: Instant) -> Instant {
    captured_at
        .filter(|at| *at <= read_at && read_at.duration_since(*at) <= CAPTURE_AGE_LIMIT)
        .unwrap_or(read_at)
}

/// The reading half of the shared capture: when to read the source, and
/// which pictures to leave in the slot.
pub(super) struct Reader {
    started: Instant,
    last_attested: Option<Instant>,
    last_cursor: Option<CursorState>,
    last_published: Option<Instant>,
    next_read: Instant,
    /// Whether the source cannot wait for pictures, so its reads are timed.
    timed: bool,
}

impl Reader {
    pub(super) fn new() -> Self {
        Self {
            started: Instant::now(),
            last_attested: None,
            last_cursor: None,
            last_published: None,
            next_read: Instant::now(),
            timed: true,
        }
    }

    /// One turn of the capture loop for `targets`: reads the source when a
    /// read is due and publishes what it read to `slot`. An error ends the
    /// capture.
    pub(super) fn turn<C: FrameSource>(
        &mut self,
        capture: &C,
        core: &Core,
        slot: &PictureSlot,
        capture_display: u64,
        targets: &[VideoTarget],
    ) -> Result<(), PublishError> {
        let period = targets
            .iter()
            .map(|target| {
                Duration::from_micros(1_000_000 / u64::from(target.quality.max_frame_rate.max(1)))
            })
            .min()
            .unwrap_or(Duration::from_millis(33));
        // Reads keep to the frame rate, however fast pictures come: a
        // 120 Hz display at 60 fps takes the newest once the period has
        // passed. They follow a grid a period apart, and one may come a
        // quarter of a period before its place on it: a compositor's
        // pictures are some milliseconds early or late, and a read held
        // for the grid would wait that long for nothing. A read long
        // after its place starts the grid again at itself. Waits and
        // sleeps are short, so stop is seen.
        let slack = period / 4;
        let now = Instant::now();
        if now + slack < self.next_read {
            std::thread::sleep((self.next_read - slack - now).min(WAKE_SLICE));
            return Ok(());
        }
        // Due: a picture that arrived meanwhile is read at once, and
        // otherwise the read waits for one rather than for a timer, so
        // a picture is read as it arrives and not up to a period later.
        let wait = capture.wait_frame(now + WAKE_SLICE);
        self.timed = wait == FrameWait::Unsupported;
        if wait == FrameWait::TimedOut
            && self
                .last_published
                .is_some_and(|at| at.elapsed() < IDLE_KEEPALIVE)
            && capture.cursor() == self.last_cursor
        {
            // Still: nothing to read until a picture, the pointer or the
            // keepalive says so.
            return Ok(());
        }
        let read_started = Instant::now();
        let timestamp = u64::try_from(self.started.elapsed().as_micros()).unwrap_or(u64::MAX);
        let frame = match capture.poll_frame(timestamp) {
            Ok(Some(frame)) => frame,
            Ok(None) => {
                // No first picture yet: a source that can wait is waited
                // on at the top of the next turn.
                if self.timed {
                    std::thread::sleep(Duration::from_millis(10));
                }
                return Ok(());
            }
            Err(error) => {
                tracing::warn!(
                    target: vnilink::trace::CAPTURE,
                    failure = vnilink::trace::Failure::failure_name(&error),
                    reason = vnilink::trace::Failure::failure_reason(&error),
                    "the screen could not be read: capture ends"
                );
                return Err(error.into());
            }
        };
        // Reading a platform-held picture's pixels copies it: only when wanted.
        if let Some(at) = capture
            .attested_at()
            .filter(|at| core.wants_attested_picture(*at))
        {
            core.offer_attested_picture(
                at,
                frame.width(),
                frame.height(),
                frame.stride(),
                frame.pixels(),
                &capture_display.to_string(),
            );
        }
        let cursor = capture.cursor();
        // A source that attests its pixels says when nothing changed: the
        // same picture is handed on once a second, not encoded and sent
        // at the full frame rate. A consumer that owes a keyframe takes
        // the last picture meanwhile.
        let attested = capture.attested_at();
        // A read that says the screen changed is a change, whatever the
        // attestation says: a picture stored between the read and the
        // attestation would otherwise lose where it changed.
        let unchanged = attested.is_some()
            && attested == self.last_attested
            && cursor == self.last_cursor
            && !frame.changed().is_some_and(|changes| changes.share > 0.0);
        if !unchanged
            || self
                .last_published
                .is_none_or(|at| at.elapsed() >= IDLE_KEEPALIVE)
        {
            self.last_attested = attested;
            self.last_cursor.clone_from(&cursor);
            self.last_published = Some(read_started);
            let captured_at = if unchanged {
                read_started
            } else {
                capture_time(frame.captured_at(), read_started)
            };
            slot.publish(
                frame,
                cursor,
                captured_at.elapsed().as_micros().min(5_000_000) as u64,
                captured_at,
                unchanged,
            );
        }
        // A source read on a timer keeps the frame rate's cadence: the
        // loop's own work is part of the period, not added to it, which
        // had a 30 fps stream read 25 times a second and a 60 fps one
        // 52. A read that ran past its time starts the cadence again
        // from now rather than reading again at once to catch up. A
        // source that wakes the read keeps the grid described above.
        let now = Instant::now();
        self.next_read = if self.timed {
            (self.next_read + period).max(now)
        } else {
            // The grid moves on a period; a read long after its place
            // starts it again, with no more than the slack in credit.
            (self.next_read + period).max(read_started + period - slack)
        };
        Ok(())
    }
}

#[cfg(test)]
mod capture_target_tests {
    use super::capture_target;
    use vnilink::{Quality, VideoTarget};

    fn target(width: u16, height: u16) -> VideoTarget {
        VideoTarget {
            id: 1,
            generation: 1,
            quality: Quality::new(8_000, 60, width, height).unwrap(),
            separate_cursor: false,
            codec: 1,
            matrix: vnilink::YuvMatrix::Bt709,
            display_id: 0,
            revision: 1,
        }
    }

    #[test]
    fn a_source_is_asked_for_the_largest_size_a_consumer_encodes() {
        let full = (3840, 2160);
        assert_eq!(capture_target(full, &[]), None);
        assert_eq!(
            capture_target(full, &[target(1920, 1080)]),
            Some((1920, 1080))
        );
        // The display's shape is kept, and the larger consumer wins.
        assert_eq!(
            capture_target(full, &[target(1280, 1024), target(1920, 1200)]),
            Some((1920, 1080))
        );
        // Never more than the display.
        assert_eq!(capture_target(full, &[target(7680, 4320)]), Some(full));
        // The size a consumer encodes from the scaled picture is the size
        // it would have encoded from the display's own.
        let wanted = capture_target((3440, 1440), &[target(1920, 1080)]).unwrap();
        assert_eq!(
            crate::frame::encoded_size(wanted.0, wanted.1, 1920, 1080).unwrap(),
            wanted
        );
    }
}

#[cfg(test)]
mod coherent_slot_tests {
    use super::*;
    use std::sync::Arc;
    #[test]
    fn concurrent_consumers_receive_matching_frame_cursor_and_generation() {
        let slot = Arc::new(PictureSlot::new());
        let producer = Arc::clone(&slot);
        let origin = Instant::now();
        let thread = std::thread::spawn(move || {
            for sequence in 1..=10_000 {
                producer.publish(
                    RawFrame::bgra(2, 2, sequence, vec![0; 16]).unwrap(),
                    Some(CursorState {
                        position: (sequence as i32, 0),
                        visible: true,
                        image: None,
                    }),
                    sequence,
                    origin + Duration::from_micros(sequence),
                    false,
                );
            }
        });
        let mut seen = 0;
        let until = Instant::now() + Duration::from_secs(5);
        while seen < 10_000 {
            assert!(Instant::now() < until, "newest sample did not arrive");
            if let Some(Sample {
                generation,
                frame,
                cursor,
                capture_us,
                captured_at,
                ..
            }) = slot.wait_newer(seen, Duration::from_millis(20))
            {
                assert_eq!(generation, frame.timestamp_us());
                assert_eq!(generation, capture_us);
                assert_eq!(
                    captured_at,
                    Some(origin + Duration::from_micros(generation))
                );
                assert_eq!(cursor.unwrap().position.0 as u64, generation);
                seen = generation;
            }
        }
        thread.join().unwrap();
        assert!(slot.wait_newer(seen, Duration::ZERO).is_none());
    }
}

#[cfg(test)]
mod capture_time_tests {
    use super::*;
    #[test]
    fn the_source_capture_time_is_used_only_when_plausible() {
        let read = Instant::now();
        let shown = read.checked_sub(Duration::from_millis(40)).unwrap();
        assert_eq!(capture_time(Some(shown), read), shown);
        // Unknown, later than the read, or too old: the read stands in.
        assert_eq!(capture_time(None, read), read);
        assert_eq!(
            capture_time(Some(read + Duration::from_millis(1)), read),
            read
        );
        let stale = read
            .checked_sub(CAPTURE_AGE_LIMIT + Duration::from_millis(1))
            .unwrap();
        assert_eq!(capture_time(Some(stale), read), read);
    }
}
