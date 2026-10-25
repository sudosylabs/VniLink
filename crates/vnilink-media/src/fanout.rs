//! One physical capture, one encoder and queue per consumer.
//!
//! The reader is the only caller of [`FrameSource::poll_frame`]. Each consumer
//! keeps the newest raw picture and encodes it on its own thread at its own
//! quality. A slow encoder skips intervening pictures; it does not queue them
//! and it does not stall the reader or any sibling.
//!
//! A source that can scale as it captures ([`FrameSource::deliver_at`]) is
//! asked for the largest size any consumer encodes, so that consumer's
//! encoder takes the picture as it comes; smaller consumers scale it down.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::ScopedJoinHandle;
use std::time::{Duration, Instant};

use vnilink::{Core, CoreEvent, CursorState, StreamFailure, VideoTarget};

use crate::cursor::{composite, for_picture};

use crate::error::{Error, PublishError};
use crate::frame::{encoded_size, RawFrame};
use crate::host::HostSession;
use crate::pump::{FrameSink, VideoEncoder};
use crate::still::{StillWorker, STILL_AFTER};
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
struct PictureSlot {
    picture: Mutex<Picture>,
    wake: Condvar,
}
impl PictureSlot {
    fn new() -> Self {
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
    fn wait_newer(&self, seen: u64, timeout: Duration) -> Option<Sample> {
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
    fn latest(&self) -> Option<Sample> {
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
    fn poke(&self) {
        self.wake.notify_all();
    }
    /// Forgets the picture, as when the source changes size: consumers
    /// wait for one of the new size.
    fn clear(&self) {
        self.picture
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .frame = None;
    }
}

/// The size to ask a source for: the largest any of `targets` encodes a
/// picture of `full` at, or `None` without a target.
fn capture_target(full: (u16, u16), targets: &[VideoTarget]) -> Option<(u16, u16)> {
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
struct Sample {
    generation: u64,
    frame: RawFrame,
    cursor: Option<CursorState>,
    capture_us: u64,
    captured_at: Option<Instant>,
    unchanged: bool,
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

struct Control {
    cancel: AtomicBool,
    finished: AtomicBool,
    keyframe: AtomicBool,
    quality: Mutex<Option<vnilink::Quality>>,
}

struct Running<'scope> {
    key: (bool, u64, u64, u64),
    control: Arc<Control>,
    /// Joined when this value drops, including shutdown and a departed consumer.
    #[allow(dead_code)]
    handle: ScopedJoinHandle<'scope, ()>,
}

fn target_key(target: &VideoTarget) -> (bool, u64, u64, u64) {
    (
        target.service,
        target.id,
        target.generation,
        target.display_id,
    )
}

fn serves(target: &VideoTarget, capture_display: u64, default: Option<u64>) -> bool {
    target.serves_source(capture_display, default)
}

fn same_membership(left: &[VideoTarget], right: &[VideoTarget]) -> bool {
    left.len() == right.len()
        && left.iter().zip(right).all(|(left, right)| {
            target_key(left) == target_key(right)
                && left.separate_cursor == right.separate_cursor
                && left.revision == right.revision
        })
}

struct TargetSink<'a> {
    core: &'a Core,
    service: bool,
    generation: u64,
    capture_display: u64,
    capture_revision: Arc<AtomicU64>,
    /// When the picture being encoded was captured.
    captured_at: Option<Instant>,
    /// The picture being encoded shows the same screen as the one before.
    unchanged: bool,
}

impl FrameSink for TargetSink<'_> {
    fn publish_frame(
        &mut self,
        id: u64,
        mut frame: vnilink::EncodedFrame,
    ) -> Result<(), vnilink::Error> {
        // Attest the actual capture source: Core backstops any remaining
        // zero from the owner's accepted selection, but only the producer
        // knows which display an unselected ("follows capture") target
        // actually receives.
        if frame.display_id == 0 {
            frame.display_id = self.capture_display;
        }
        if frame.timing.captured_at.is_none() {
            frame.timing.captured_at = self.captured_at;
        }
        if self.unchanged {
            frame = frame.unchanged();
        }
        // Attest the revision the pixels were captured for, read live:
        // a supervisor re-attesting an unchanged capture updates the
        // stamp without restarting this worker. A capture that does not
        // know its revision (0) falls back to the current topology; a
        // pinned capture keeps its attestation even after the topology
        // advanced, so a stale crop is never relabeled current.
        if frame.revision == 0 {
            let pinned = self.capture_revision.load(Ordering::Acquire);
            frame.revision = if pinned != 0 {
                pinned
            } else {
                self.core
                    .topology()
                    .map(|topology| topology.revision)
                    .unwrap_or(0)
            };
        }
        if self.service {
            self.core.publish_desktop_video(id, self.generation, frame)
        } else {
            self.core.publish_video(id, frame)
        }
    }
}

fn ended(error: &vnilink::Error) -> bool {
    matches!(
        error,
        vnilink::Error::NoSession | vnilink::Error::StaleSession
    )
}

/// Ends one consumer because its producer failed: a desktop service tells
/// its viewer what failed; a legacy session peer is stopped.
fn fail_target(core: &Core, target: &VideoTarget, failure: StreamFailure) {
    if target.service {
        let _ = core.fail_hosted_service(target.id, target.generation, failure);
    } else {
        let _ = core.stop_hosted_peer(target.id);
    }
}

/// One target's still pictures: asked for once the screen has stayed the
/// same for [`STILL_AFTER`], once per still period, and published only if
/// the screen did not change meanwhile.
struct Stills {
    /// The viewer agreed to still pictures.
    agreed: bool,
    worker: Option<StillWorker>,
    /// Changes seen, and when the last came.
    change: u64,
    changed_at: Instant,
    /// The change a still was last asked for after.
    asked: Option<u64>,
}

impl Stills {
    fn new() -> Self {
        Self {
            agreed: false,
            worker: None,
            change: 0,
            changed_at: Instant::now(),
            asked: None,
        }
    }

    fn changed(&mut self) {
        self.change = self.change.wrapping_add(1);
        self.changed_at = Instant::now();
    }

    /// Whether to ask for a still now: agreed, still for long enough, not
    /// asked for since the last change, and no keyframe owed.
    fn due(&self, keyframe_pending: bool) -> bool {
        self.agreed
            && !keyframe_pending
            && self.asked != Some(self.change)
            && self.changed_at.elapsed() >= STILL_AFTER
    }

    fn make(&mut self, frame: RawFrame, size: (u16, u16), budget: usize) {
        if self.worker.is_none() {
            self.worker = StillWorker::new().ok();
        }
        if self
            .worker
            .as_ref()
            .is_some_and(|worker| worker.make(self.change, frame, size, budget))
        {
            self.asked = Some(self.change);
        }
    }

    /// A still made since, unless the screen changed after it was asked for.
    fn made(&mut self) -> Option<vnilink::EncodedFrame> {
        let made = self.worker.as_ref()?.take()?;
        if made.change != self.change {
            return None;
        }
        vnilink::EncodedFrame::still(made.timestamp_us, made.picture?).ok()
    }
}

fn consume<E, F>(
    core: &Core,
    slot: &PictureSlot,
    target: VideoTarget,
    capture_display: u64,
    capture_revision: Arc<AtomicU64>,
    capture_size: (u16, u16),
    control: &Control,
    open_encoder: Arc<F>,
) where
    E: VideoEncoder,
    F: Fn(&VideoTarget, u16, u16, u8, u32) -> Result<E, Error> + Send + Sync + 'static,
{
    let target_for_open = target;
    let reopen = Arc::clone(&open_encoder);
    let opened = HostSession::open(
        capture_size.0,
        capture_size.1,
        target.quality,
        move |width, height, fps, bps| reopen(&target_for_open, width, height, fps, bps),
    );
    let mut session = match opened {
        Ok(session) => session,
        Err(_) => {
            fail_target(core, &target, StreamFailure::Encoder);
            return;
        }
    };
    let mut sink = TargetSink {
        core,
        service: target.service,
        generation: target.generation,
        capture_display,
        capture_revision,
        captured_at: None,
        unchanged: false,
    };
    // The encoder as last reported: at open, at every reopen, and whenever
    // a picture took another input than the one before.
    let mut reported = session.implementation();
    if target.service {
        let _ = core.report_desktop_encoder(target.id, target.generation, reported);
    }
    let mut applied_quality = target.quality;
    let mut seen = 0u64;
    let mut stills = Stills::new();
    while !control.cancel.load(Ordering::Acquire) {
        let reopens = session.reopens();
        if let Some(quality) = control
            .quality
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .take()
        {
            if session
                .observe(&CoreEvent::QualityUpdated {
                    id: target.id,
                    quality,
                })
                .is_err()
            {
                fail_target(core, &target, StreamFailure::Encoder);
                return;
            }
            applied_quality = quality;
        }
        if target.service {
            match core.hosted_desktop_info(target.id, target.generation) {
                Ok(info) => {
                    stills.agreed = info.stills;
                    if info.quality != applied_quality {
                        if session
                            .observe(&CoreEvent::QualityUpdated {
                                id: target.id,
                                quality: info.quality,
                            })
                            .is_err()
                        {
                            fail_target(core, &target, StreamFailure::Encoder);
                            return;
                        }
                        applied_quality = info.quality;
                    }
                }
                // The owner ended, or the core shut down: nobody to tell.
                Err(_) => return,
            }
            if core
                .take_desktop_keyframe_request(target.id, target.generation)
                .unwrap_or(false)
            {
                session
                    .observe(&CoreEvent::KeyframeRequested { id: target.id })
                    .ok();
            }
        } else if control.keyframe.swap(false, Ordering::AcqRel) {
            session
                .observe(&CoreEvent::KeyframeRequested { id: target.id })
                .ok();
        }
        if target.service && session.reopens() != reopens {
            let _ = core.report_desktop_encoder_reopen(target.id, target.generation);
            reported = session.implementation();
            let _ = core.report_desktop_encoder(target.id, target.generation, reported);
        }
        if let Some(still) = stills.made() {
            sink.captured_at = None;
            sink.unchanged = false;
            match sink.publish_frame(target.id, still) {
                Err(error) if ended(&error) => return,
                // Left out or refused: the stream goes on without it.
                _ => {}
            }
        }
        if stills.due(session.keyframe_pending()) {
            if let Some(sample) = slot.latest() {
                let frame = match (&sample.cursor, target.separate_cursor) {
                    (Some(cursor), false) => composite(&sample.frame, cursor).ok(),
                    _ => Some(sample.frame),
                };
                if let Some(frame) = frame {
                    let budget = core
                        .desktop_still_budget(target.id, target.generation)
                        .unwrap_or(0);
                    let size = encoded_size(
                        capture_size.0,
                        capture_size.1,
                        applied_quality.max_width,
                        applied_quality.max_height,
                    )
                    .unwrap_or(capture_size);
                    stills.make(frame, size, budget);
                }
            }
        }
        // On an unchanged screen no new picture comes: a keyframe still
        // owed is made from the last one, and after a keyframe the last one
        // is encoded again, stamped now, for the encoder to refine it.
        let next = slot
            .wait_newer(seen, Duration::from_millis(20))
            .or_else(|| {
                (session.keyframe_pending() || session.refining())
                    .then(|| slot.latest())
                    .flatten()
                    .map(|mut sample| {
                        sample.unchanged = true;
                        if let Some(at) = sample.captured_at {
                            let since = u64::try_from(at.elapsed().as_micros()).unwrap_or(u64::MAX);
                            let stamp = sample.frame.timestamp_us().saturating_add(since);
                            sample.frame = sample.frame.with_timestamp(stamp);
                        }
                        sample
                    })
            });
        let Some(Sample {
            generation,
            mut frame,
            cursor,
            capture_us,
            captured_at,
            unchanged,
        }) = next
        else {
            continue;
        };
        seen = generation;
        sink.captured_at = captured_at;
        sink.unchanged = unchanged;
        if !unchanged {
            stills.changed();
        }
        if frame.width() != capture_size.0 || frame.height() != capture_size.1 {
            // The source changed size. Leave the target up so the supervisor
            // can open a fresh encoder; do not close the peer.
            return;
        }
        if target.separate_cursor {
            if let Some(cursor) = cursor {
                // The viewer places the cursor on the picture it decodes.
                let picture = encoded_size(
                    capture_size.0,
                    capture_size.1,
                    applied_quality.max_width,
                    applied_quality.max_height,
                );
                let Ok(cursor) =
                    picture.and_then(|picture| for_picture(&cursor, capture_size, picture))
                else {
                    fail_target(core, &target, StreamFailure::Capture);
                    return;
                };
                let published = if target.service {
                    core.publish_desktop_cursor(target.id, target.generation, cursor)
                } else {
                    core.publish_cursor(target.id, target.generation, cursor)
                };
                match published {
                    Ok(()) => {}
                    Err(error) if ended(&error) => return,
                    Err(_) => {
                        fail_target(core, &target, StreamFailure::Capture);
                        return;
                    }
                }
            }
        } else if let Some(cursor) = cursor.as_ref() {
            match composite(&frame, cursor) {
                Ok(composited) => frame = composited,
                Err(_) => {
                    fail_target(core, &target, StreamFailure::Capture);
                    return;
                }
            }
        }
        let encode_started = Instant::now();
        match session.publish(&frame, &mut sink, target.id) {
            Ok(crate::PumpOutput::Encoded(_)) => {
                if target.service && session.implementation() != reported {
                    reported = session.implementation();
                    let _ = core.report_desktop_encoder(target.id, target.generation, reported);
                }
                if target.service {
                    let _ = core.report_desktop_producer_duration(
                        target.id,
                        target.generation,
                        capture_us,
                        encode_started.elapsed().as_micros().min(5_000_000) as u64,
                    );
                }
            }
            Ok(_) => {}
            Err(PublishError::Session(error)) if ended(&error) => return,
            // The encoder failed, or what it made could not be sent.
            Err(_) => {
                fail_target(core, &target, StreamFailure::Encoder);
                return;
            }
        }
    }
}

/// Copy `capture` to its [`Core::video_targets`] consumers until `stop`.
///
/// Selected consumers bind their exact source; unselected consumers follow
/// the lowest requested display once anyone selected, and every capture
/// while nothing is selected (bootstrap). A zero `capture_display` is the
/// legacy whole-desktop singleton and serves every consumer.
///
/// The caller has already opened `capture`. This function does not open a
/// second capture and does not draw. When `exit_when_idle` is set, it returns
/// once no consumer remains so the caller can drop the capture. Otherwise it
/// waits through gaps between consumers until `stop`. One consumer's encoder
/// failure closes only that consumer.
pub fn serve_shared_capture<C, E, F>(
    capture: &C,
    core: &Core,
    stop: &AtomicBool,
    exit_when_idle: bool,
    capture_display: u64,
    mut note_targets: impl FnMut(&[VideoTarget]),
    open_encoder: F,
) -> Result<(), PublishError>
where
    C: FrameSource,
    E: VideoEncoder + 'static,
    F: Fn(&VideoTarget, u16, u16, u8, u32) -> Result<E, Error> + Send + Sync + 'static,
{
    let open_encoder = Arc::new(open_encoder);
    let slot = PictureSlot::new();
    let events = core.subscribe().ok();
    let started = Instant::now();
    let mut noted: Vec<VideoTarget> = Vec::new();
    let mut last_attested = None;
    let mut last_cursor = None;
    let mut last_published: Option<Instant> = None;
    // A size the source would not deliver is not asked for again.
    let mut refused = None;
    let mut next_read = Instant::now();
    // Whether the source cannot wait for pictures, so its reads are timed.
    let mut timed = true;
    std::thread::scope(|scope| {
        let mut running: Vec<Running<'_>> = Vec::new();
        let mut draining: Vec<Running<'_>> = Vec::new();
        let result = loop {
            if stop.load(Ordering::Acquire) {
                break Ok(());
            }
            let routing = match core.video_routing() {
                Ok(targets) => targets,
                Err(vnilink::Error::ShutDown) => break Ok(()),
                Err(error) => break Err(error.into()),
            };
            let default = routing.default_source;
            let targets: Vec<_> = routing
                .targets
                .into_iter()
                .filter(|target| serves(target, capture_display, default))
                .collect();
            if targets.is_empty() && exit_when_idle {
                break Ok(());
            }
            if !same_membership(&noted, &targets) {
                note_targets(&targets);
                noted = targets.clone();
            }
            if let Some(events) = &events {
                while let Some(event) = events.try_recv() {
                    match event {
                        CoreEvent::QualityUpdated { id, quality } => {
                            if let Some(worker) = running
                                .iter()
                                .find(|worker| !worker.key.0 && worker.key.1 == id)
                            {
                                *worker
                                    .control
                                    .quality
                                    .lock()
                                    .unwrap_or_else(|poison| poison.into_inner()) = Some(quality);
                            }
                        }
                        CoreEvent::KeyframeRequested { id } => {
                            if let Some(worker) = running
                                .iter()
                                .find(|worker| !worker.key.0 && worker.key.1 == id)
                            {
                                worker.control.keyframe.store(true, Ordering::Release);
                            }
                        }
                        _ => {}
                    }
                }
            }
            // Before any consumer opens at the size the source delivers now:
            // those that read another size end and open again at the new one.
            if let Some(wanted) = capture_target(capture.full_size(), &targets) {
                if wanted != capture.size() && refused != Some(wanted) {
                    match capture.deliver_at(wanted) {
                        Ok(true) => slot.clear(),
                        Ok(false) => refused = Some(wanted),
                        Err(error) => break Err(error.into()),
                    }
                }
            }
            let capture_size = capture.size();
            let capture_revision = capture.capture_revision();
            let live: Vec<_> = targets
                .iter()
                .copied()
                .map(|target| target_key(&target))
                .collect();
            let mut index = 0;
            while index < running.len() {
                let gone = !live.contains(&running[index].key)
                    || running[index].control.finished.load(Ordering::Acquire);
                if gone {
                    running[index].control.cancel.store(true, Ordering::Release);
                    draining.push(running.swap_remove(index));
                } else {
                    index += 1;
                }
            }
            draining.retain(|worker| !worker.control.finished.load(Ordering::Acquire));
            for target in &targets {
                let key = target_key(target);
                if running.iter().any(|worker| worker.key == key) {
                    continue;
                }
                let control = Arc::new(Control {
                    cancel: AtomicBool::new(false),
                    finished: AtomicBool::new(false),
                    keyframe: AtomicBool::new(false),
                    quality: Mutex::new(None),
                });
                let worker_control = Arc::clone(&control);
                let worker_slot = &slot;
                let worker_core = core;
                let worker_open = Arc::clone(&open_encoder);
                let worker_target = *target;
                let worker_revision = Arc::clone(&capture_revision);
                let handle = scope.spawn(move || {
                    consume(
                        worker_core,
                        worker_slot,
                        worker_target,
                        capture_display,
                        worker_revision,
                        capture_size,
                        &worker_control,
                        worker_open,
                    );
                    worker_control.finished.store(true, Ordering::Release);
                });
                running.push(Running {
                    key,
                    control,
                    handle,
                });
            }
            if targets.is_empty() {
                std::thread::sleep(Duration::from_millis(20));
                continue;
            }
            let period = targets
                .iter()
                .map(|target| {
                    Duration::from_micros(
                        1_000_000 / u64::from(target.quality.max_frame_rate.max(1)),
                    )
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
            if now + slack < next_read {
                std::thread::sleep((next_read - slack - now).min(WAKE_SLICE));
                continue;
            }
            // Due: a picture that arrived meanwhile is read at once, and
            // otherwise the read waits for one rather than for a timer, so
            // a picture is read as it arrives and not up to a period later.
            let wait = capture.wait_frame(now + WAKE_SLICE);
            timed = wait == FrameWait::Unsupported;
            if wait == FrameWait::TimedOut
                && last_published.is_some_and(|at| at.elapsed() < IDLE_KEEPALIVE)
                && capture.cursor() == last_cursor
            {
                // Still: nothing to read until a picture, the pointer or the
                // keepalive says so.
                continue;
            }
            let read_started = Instant::now();
            let timestamp = u64::try_from(started.elapsed().as_micros()).unwrap_or(u64::MAX);
            let frame = match capture.poll_frame(timestamp) {
                Ok(Some(frame)) => frame,
                Ok(None) => {
                    // No first picture yet: a source that can wait is waited
                    // on at the top of the next turn.
                    if timed {
                        std::thread::sleep(Duration::from_millis(10));
                    }
                    continue;
                }
                Err(error) => break Err(error.into()),
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
            let unchanged =
                attested.is_some() && attested == last_attested && cursor == last_cursor;
            if !unchanged || last_published.is_none_or(|at| at.elapsed() >= IDLE_KEEPALIVE) {
                last_attested = attested;
                last_cursor.clone_from(&cursor);
                last_published = Some(read_started);
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
            next_read = if timed {
                (next_read + period).max(now)
            } else {
                // The grid moves on a period; a read long after its place
                // starts it again, with no more than the slack in credit.
                (next_read + period).max(read_started + period - slack)
            };
        };
        for worker in running.iter().chain(draining.iter()) {
            worker.control.cancel.store(true, Ordering::Release);
        }
        slot.poke();
        drop(running);
        drop(draining);
        result
    })
}

#[cfg(test)]
mod display_filter {
    use super::serves;
    use vnilink::{Quality, VideoTarget};

    fn target(display_id: u64) -> VideoTarget {
        VideoTarget {
            service: false,
            id: 1,
            generation: 1,
            quality: Quality::new(1_000, 30, 1280, 720).unwrap(),
            separate_cursor: false,
            codec: 1,
            matrix: vnilink::YuvMatrix::Bt709,
            display_id,
            revision: 1,
        }
    }

    #[test]
    fn a_capture_serves_its_display_selected_or_defaulted() {
        let unselected = target(0);
        let left = target(4);
        let right = target(5);
        // Bootstrap: nothing requested yet, every capture serves.
        assert!(serves(&unselected, 4, None));
        // A selected viewer binds its exact source, never its twin.
        assert!(serves(&left, 4, Some(5)));
        assert!(!serves(&right, 4, Some(5)));
        assert!(serves(&right, 5, Some(4)));
        assert!(!serves(&left, 5, Some(4)));
        // An unselected viewer follows the lowest requested display.
        assert!(serves(&unselected, 4, Some(4)));
        assert!(!serves(&unselected, 5, Some(4)));
        assert!(!serves(&unselected, 4, Some(5)));
        // The legacy whole-desktop capture still serves every consumer.
        assert!(serves(&unselected, 0, Some(4)));
        assert!(serves(&left, 0, Some(5)));
    }
}

#[cfg(test)]
mod capture_target_tests {
    use super::capture_target;
    use vnilink::{Quality, VideoTarget};

    fn target(width: u16, height: u16) -> VideoTarget {
        VideoTarget {
            service: true,
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
