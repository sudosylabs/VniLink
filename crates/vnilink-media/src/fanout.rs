//! One physical capture, one encoder and queue per consumer.
//!
//! The reader is the only caller of [`FrameSource::poll_frame`]. Each consumer
//! keeps the newest raw picture and encodes it on its own thread at its own
//! quality. A slow encoder skips intervening pictures; it does not queue them
//! and it does not stall the reader or any sibling.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::ScopedJoinHandle;
use std::time::{Duration, Instant};

use vnilink::{Core, CoreEvent, CursorState, VideoTarget};

use crate::cursor::{composite, for_picture};

use crate::error::{Error, PublishError};
use crate::frame::{encoded_size, RawFrame};
use crate::host::HostSession;
use crate::pump::{FrameSink, VideoEncoder};
use crate::FrameSource;

#[derive(Default)]
struct Picture {
    generation: u64,
    frame: Option<RawFrame>,
    cursor: Option<CursorState>,
    capture_us: u64,
    captured_at: Option<Instant>,
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
    ) {
        let mut picture = self
            .picture
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        picture.frame = Some(frame);
        picture.cursor = cursor;
        picture.capture_us = capture_us;
        picture.captured_at = Some(captured_at);
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
        })
    }
    fn poke(&self) {
        self.wake.notify_all();
    }
}

/// The newest picture as one consumer takes it.
struct Sample {
    generation: u64,
    frame: RawFrame,
    cursor: Option<CursorState>,
    capture_us: u64,
    captured_at: Option<Instant>,
}

/// How often an unchanged picture is still handed on, so a viewer sees the
/// stream is alive.
const IDLE_KEEPALIVE: Duration = Duration::from_secs(1);

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

fn close_target(core: &Core, target: &VideoTarget) {
    if target.service {
        let _ = core.close_hosted_service(target.id, target.generation);
    } else {
        let _ = core.stop_hosted_peer(target.id);
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
            close_target(core, &target);
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
    };
    let mut applied_quality = target.quality;
    let mut seen = 0u64;
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
                close_target(core, &target);
                return;
            }
            applied_quality = quality;
        }
        if target.service {
            match core.hosted_desktop_info(target.id, target.generation) {
                Ok(info) if info.quality != applied_quality => {
                    if session
                        .observe(&CoreEvent::QualityUpdated {
                            id: target.id,
                            quality: info.quality,
                        })
                        .is_err()
                    {
                        close_target(core, &target);
                        return;
                    }
                    applied_quality = info.quality;
                }
                Ok(_) => {}
                Err(error) if ended(&error) => return,
                Err(_) => {
                    close_target(core, &target);
                    return;
                }
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
        }) = next
        else {
            continue;
        };
        seen = generation;
        sink.captured_at = captured_at;
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
                    close_target(core, &target);
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
                        close_target(core, &target);
                        return;
                    }
                }
            }
        } else if let Some(cursor) = cursor.as_ref() {
            match composite(&frame, cursor) {
                Ok(composited) => frame = composited,
                Err(_) => {
                    close_target(core, &target);
                    return;
                }
            }
        }
        let encode_started = Instant::now();
        match session.publish(&frame, &mut sink, target.id) {
            Ok(crate::PumpOutput::Encoded(_)) => {
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
            Err(_) => {
                close_target(core, &target);
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
            let read_started = Instant::now();
            let timestamp = u64::try_from(started.elapsed().as_micros()).unwrap_or(u64::MAX);
            let frame = match capture.poll_frame(timestamp) {
                Ok(Some(frame)) => frame,
                Ok(None) => {
                    std::thread::sleep(Duration::from_millis(10));
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
                );
            }
            // Always wait out the period: a slow read is no reason to read
            // again at once.
            if let Some(remaining) = period.checked_sub(read_started.elapsed()) {
                std::thread::sleep(remaining);
            }
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
