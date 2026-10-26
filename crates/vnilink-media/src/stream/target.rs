//! The shared capture's consumers: which video targets a capture serves,
//! one encoding thread per target, and how a target's failure ends it alone.
// trace: records every ignored failure

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::ScopedJoinHandle;
use std::time::{Duration, Instant};

use vnilink::{Core, CoreEvent, StreamFailure, VideoTarget};

use super::capture::{capture_target, PictureSlot, Reader, Sample};
use super::changes::{pointer_bounds, small_change, TargetSink};
use super::stills::Stills;
use crate::cursor::{composite, for_picture};
use crate::error::{Error, PublishError};
use crate::frame::encoded_size;
use crate::host::HostSession;
use crate::pump::{FrameSink, VideoEncoder};
use crate::FrameSource;

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

fn ended(error: &vnilink::Error) -> bool {
    matches!(
        error,
        vnilink::Error::NoSession | vnilink::Error::StaleSession
    )
}

/// A failure for records: its name and fixed reason, never its text.
fn named(error: &dyn vnilink::trace::Failure) -> (&'static str, Option<&'static str>) {
    (error.failure_name(), error.failure_reason())
}

fn named_publish(error: &PublishError) -> (&'static str, Option<&'static str>) {
    match error {
        PublishError::Media(error) => named(error),
        PublishError::Session(error) => named(error),
    }
}

/// Ends one consumer because its producer failed, recording `why` and what
/// failed: a desktop service tells its viewer what failed; a legacy session
/// peer is stopped.
fn fail_target(
    core: &Core,
    target: &VideoTarget,
    failure: StreamFailure,
    why: &'static str,
    error: Option<(&'static str, Option<&'static str>)>,
) {
    tracing::warn!(
        target: vnilink::trace::STREAM,
        service = target.id,
        part = ?failure,
        why,
        failure = error.map(|(name, _)| name),
        reason = error.and_then(|(_, reason)| reason),
        "the picture stream failed"
    );
    if target.service {
        vnilink::ignored!(
            vnilink::trace::STREAM,
            "a viewer already gone needs no failure",
            core.fail_hosted_service(target.id, target.generation, failure)
        );
    } else {
        vnilink::ignored!(
            vnilink::trace::STREAM,
            "a peer already gone needs no stop",
            core.stop_hosted_peer(target.id)
        );
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
        Err(error) => {
            fail_target(
                core,
                &target,
                StreamFailure::Encoder,
                "no encoder opened",
                Some(named(&error)),
            );
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
        pending: None,
        capture: capture_size,
        picture: capture_size,
    };
    // Where the pointer drawn into the picture last was.
    let mut drawn_pointer: Option<vnilink::Region> = None;
    // The encoder as last reported: at open, at every reopen, and whenever
    // a picture took another input than the one before.
    let mut reported = session.implementation();
    if target.service {
        vnilink::ignored!(
            vnilink::trace::STREAM,
            "a report about an ended service is not needed",
            core.report_desktop_encoder(target.id, target.generation, reported)
        );
    }
    let mut applied_quality = target.quality;
    let mut seen = 0u64;
    let mut stills = Stills::new(target.id, Instant::now());
    while !control.cancel.load(Ordering::Acquire) {
        let reopens = session.reopens();
        if let Some(quality) = control
            .quality
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .take()
        {
            if let Err(error) = session.observe(&CoreEvent::QualityUpdated {
                id: target.id,
                quality,
            }) {
                fail_target(
                    core,
                    &target,
                    StreamFailure::Encoder,
                    "the encoder did not take the new quality",
                    Some(named(&error)),
                );
                return;
            }
            applied_quality = quality;
        }
        if target.service {
            match core.hosted_desktop_info(target.id, target.generation) {
                Ok(info) => {
                    stills.agreed = info.stills;
                    if info.quality != applied_quality {
                        if let Err(error) = session.observe(&CoreEvent::QualityUpdated {
                            id: target.id,
                            quality: info.quality,
                        }) {
                            fail_target(
                                core,
                                &target,
                                StreamFailure::Encoder,
                                "the encoder did not take the viewer's quality",
                                Some(named(&error)),
                            );
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
                vnilink::ignored!(
                    vnilink::trace::STREAM,
                    "a keyframe the encoder could not start comes at the next request",
                    session.observe(&CoreEvent::KeyframeRequested { id: target.id })
                );
            }
        } else if control.keyframe.swap(false, Ordering::AcqRel) {
            vnilink::ignored!(
                vnilink::trace::STREAM,
                "a keyframe the encoder could not start comes at the next request",
                session.observe(&CoreEvent::KeyframeRequested { id: target.id })
            );
        }
        if target.service && session.reopens() != reopens {
            tracing::info!(target: vnilink::trace::ENCODE, service = target.id, "encoder reopened");
            vnilink::ignored!(
                vnilink::trace::STREAM,
                "a report about an ended service is not needed",
                core.report_desktop_encoder_reopen(target.id, target.generation)
            );
            reported = session.implementation();
            vnilink::ignored!(
                vnilink::trace::STREAM,
                "a report about an ended service is not needed",
                core.report_desktop_encoder(target.id, target.generation, reported)
            );
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
        if stills.due(session.keyframe_pending(), Instant::now()) {
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
        // Pictures missed between two read here changed the screen where
        // none can say.
        if seen != 0 && generation > seen.wrapping_add(1) {
            sink.pending = None;
        }
        seen = generation;
        sink.captured_at = captured_at;
        sink.unchanged = unchanged;
        if !unchanged {
            sink.add(
                frame
                    .changed()
                    .and_then(|changes| changes.regions.as_deref()),
            );
            if small_change(&frame) {
                stills.changed_a_little(Instant::now());
            } else {
                stills.changed(Instant::now());
            }
        }
        sink.picture = encoded_size(
            capture_size.0,
            capture_size.1,
            applied_quality.max_width,
            applied_quality.max_height,
        )
        .unwrap_or(capture_size);
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
                let cursor =
                    match picture.and_then(|picture| for_picture(&cursor, capture_size, picture)) {
                        Ok(cursor) => cursor,
                        Err(error) => {
                            fail_target(
                                core,
                                &target,
                                StreamFailure::Capture,
                                "the pointer could not be placed on the picture",
                                Some(named(&error)),
                            );
                            return;
                        }
                    };
                let published = if target.service {
                    core.publish_desktop_cursor(target.id, target.generation, cursor)
                } else {
                    core.publish_cursor(target.id, target.generation, cursor)
                };
                match published {
                    Ok(()) => {}
                    Err(error) if ended(&error) => return,
                    Err(error) => {
                        fail_target(
                            core,
                            &target,
                            StreamFailure::Capture,
                            "the pointer could not be sent",
                            Some(named(&error)),
                        );
                        return;
                    }
                }
            }
        } else if let Some(cursor) = cursor.as_ref() {
            // A pointer drawn into the picture changes it where it was and where it is.
            let pointer = pointer_bounds(cursor);
            if pointer != drawn_pointer {
                let places: Vec<_> = drawn_pointer.into_iter().chain(pointer).collect();
                sink.add(Some(&places));
                drawn_pointer = pointer;
            }
            match composite(&frame, cursor) {
                Ok(composited) => frame = composited,
                Err(error) => {
                    fail_target(
                        core,
                        &target,
                        StreamFailure::Capture,
                        "the pointer could not be drawn into the picture",
                        Some(named(&error)),
                    );
                    return;
                }
            }
        }
        let encode_started = Instant::now();
        match session.publish(&frame, &mut sink, target.id) {
            Ok(crate::PumpOutput::Encoded(_)) => {
                if target.service && session.implementation() != reported {
                    reported = session.implementation();
                    vnilink::ignored!(
                        vnilink::trace::STREAM,
                        "a report about an ended service is not needed",
                        core.report_desktop_encoder(target.id, target.generation, reported)
                    );
                }
                if target.service {
                    vnilink::ignored!(
                        vnilink::trace::STREAM,
                        "a report about an ended service is not needed",
                        core.report_desktop_producer_duration(
                            target.id,
                            target.generation,
                            capture_us,
                            encode_started.elapsed().as_micros().min(5_000_000) as u64,
                        )
                    );
                }
            }
            Ok(_) => {}
            Err(PublishError::Session(error)) if ended(&error) => return,
            // The encoder failed, or what it made could not be sent.
            Err(error) => {
                fail_target(
                    core,
                    &target,
                    StreamFailure::Encoder,
                    "the encoder failed or its picture could not be sent",
                    Some(named_publish(&error)),
                );
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
    // What this pipeline records says which engine it serves.
    let engine = core.trace_span();
    let _engine = engine.enter();
    let open_encoder = Arc::new(open_encoder);
    let slot = PictureSlot::new();
    let events = core.subscribe().ok();
    let mut noted: Vec<VideoTarget> = Vec::new();
    let mut reader = Reader::new();
    // A size the source would not deliver is not asked for again.
    let mut refused = None;
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
                let worker_engine = engine.clone();
                let handle = scope.spawn(move || {
                    let _engine = worker_engine.enter();
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
            if let Err(error) = reader.turn(capture, core, &slot, capture_display, &targets) {
                break Err(error);
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
