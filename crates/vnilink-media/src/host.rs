//! One hosted monitor: encoder plus pacing plus publish.
//!
//! [`HostSession`] opens the platform encoder at the settings its
//! [`RateController`] derives from the session quality, then paces pictures
//! through [`HostVideo`]. [`serve_capture`] reads a capture that is already
//! open. It does not ask for capture permission and does not draw. Frame bytes
//! are not logged. Stop is noticed between bounded reads, even before the
//! first picture. A session that is no
//! longer active ends the loop.

use std::sync::atomic::{AtomicBool, Ordering};

use vnilink::Core;
use vnilink::CoreEvent;

use crate::error::{Error, PublishError};
use crate::frame::RawFrame;
use crate::pump::{CoreSink, FrameSink, HostVideo, PumpOutput, VideoEncoder};
use crate::rate::{RateChange, RateController};
use crate::Quality;

/// Where paced pictures come from. Both portal and ScreenCaptureKit captures satisfy it.
pub trait FrameSource {
    fn size(&self) -> (u16, u16);
    /// One bounded native read. `None` means no first picture yet, not failure.
    /// An idle source may return its cached picture with the new timestamp.
    /// Implementations must not wait indefinitely for desktop damage: owners
    /// and shutdown are checked between reads. Healthy native reads finish
    /// within 100 ms; native API/device failure is returned as an error.
    fn poll_frame(&self, timestamp_us: u64) -> Result<Option<RawFrame>, Error>;
    fn cursor(&self) -> Option<vnilink::CursorState> {
        None
    }
    /// When the OS last attested these pixels. `None` means this source cannot
    /// tell a new acquisition from a retimestamped cache.
    fn attested_at(&self) -> Option<std::time::Instant> {
        None
    }
    fn attested_display(&self) -> &str {
        ""
    }
    /// Shared topology revision the polled pictures attest, read live
    /// for every published frame. 0 means unknown: the fanout stamps the
    /// current topology revision instead. Captures whose geometry is
    /// re-attested without reopening share this with their supervisor.
    fn capture_revision(&self) -> std::sync::Arc<std::sync::atomic::AtomicU64> {
        std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0))
    }
}

/// Platform encoder constructor, kept so a later quality change can reopen the
/// encoder at the new size. Both platform encoders satisfy it as a function.
pub type OpenEncoder<E> = Box<dyn Fn(u16, u16, u8, u32) -> Result<E, Error> + Send>;

/// Encoder for one captured monitor.
pub struct HostSession<E> {
    control: RateController,
    open_encoder: OpenEncoder<E>,
    video: HostVideo<E>,
    reopens: u64,
    /// A bitrate below the quality's, after a frame came out too large;
    /// cleared by the next quality update.
    capped_bps: Option<u32>,
}

impl<E: VideoEncoder> HostSession<E> {
    /// Open the encoder for `capture_width` by `capture_height` at `quality`.
    ///
    /// The encoder size follows the quality limit and stays even. `open_encoder`
    /// is the platform constructor; it is kept so a later quality change can
    /// reopen the encoder at the new size.
    pub fn open(
        capture_width: u16,
        capture_height: u16,
        quality: Quality,
        open_encoder: impl Fn(u16, u16, u8, u32) -> Result<E, Error> + Send + 'static,
    ) -> Result<Self, Error> {
        let control = RateController::open(capture_width, capture_height, quality)?;
        let settings = control.settings();
        let video = HostVideo::new(
            quality,
            open_encoder(
                settings.width,
                settings.height,
                settings.frame_rate,
                settings.bitrate_bps,
            )?,
        );
        Ok(Self {
            control,
            open_encoder: Box::new(open_encoder),
            video,
            reopens: 0,
            capped_bps: None,
        })
    }

    /// Whether the next encoded picture must be a keyframe.
    pub fn keyframe_pending(&self) -> bool {
        self.video.keyframe_pending()
    }

    /// How many times a quality change opened the encoder again.
    pub fn reopens(&self) -> u64 {
        self.reopens
    }

    pub fn capture_size(&self) -> (u16, u16) {
        self.control.capture_size()
    }

    pub fn encoded_size(&self) -> (u16, u16) {
        self.control.encoded_size()
    }

    /// A keyframe request forces the next picture. A quality change at the
    /// same size retunes the running encoder, without a keyframe; a new size,
    /// or an encoder that cannot retune, opens it again.
    pub fn observe(&mut self, event: &CoreEvent) -> Result<(), Error> {
        match event {
            CoreEvent::QualityUpdated { quality, .. } => {
                self.capped_bps = None;
                let reopen = match self.control.observe(event)? {
                    RateChange::Same => false,
                    RateChange::Retune => {
                        let next = self.control.settings();
                        !self.video.retune(next.bitrate_bps, next.frame_rate)?
                    }
                    RateChange::Reopen => true,
                };
                if reopen {
                    let settings = self.control.settings();
                    self.video = HostVideo::new(
                        *quality,
                        (self.open_encoder)(
                            settings.width,
                            settings.height,
                            settings.frame_rate,
                            settings.bitrate_bps,
                        )?,
                    );
                    self.reopens += 1;
                } else {
                    self.video.set_quality(*quality);
                }
                Ok(())
            }
            other => {
                self.video.observe(other);
                Ok(())
            }
        }
    }

    /// Pace `frame` and publish it when it is due.
    ///
    /// `frame` must be the capture size passed to [`Self::open`]. A picture
    /// that arrives faster than the quality frame rate is skipped, unless a
    /// keyframe is still owed.
    pub fn publish(
        &mut self,
        frame: &RawFrame,
        sink: &mut dyn FrameSink,
        session_id: u64,
    ) -> Result<PumpOutput, PublishError> {
        let (width, height) = self.control.capture_size();
        if frame.width() != width || frame.height() != height {
            return Err(Error::InvalidFrame("the frame size does not match the encoder").into());
        }
        match self.video.publish_to(sink, session_id, frame) {
            // A frame larger than one may be is dropped, not fatal: the next
            // starts from a keyframe, at a quarter less bitrate.
            Err(PublishError::Media(error)) if error.is_too_large() => {
                self.shrink()?;
                self.video.request_keyframe();
                Ok(PumpOutput::Skipped)
            }
            other => other,
        }
    }

    /// Aims the encoder a quarter lower, down to 100 kb/s, as it runs if it
    /// can, else by opening it again.
    fn shrink(&mut self) -> Result<(), Error> {
        let settings = self.control.settings();
        let current = self.capped_bps.unwrap_or(settings.bitrate_bps);
        let lower = (current / 4 * 3).max(100_000);
        self.capped_bps = Some(lower);
        if !self.video.retune(lower, settings.frame_rate)? {
            let quality = self.video.quality();
            self.video = HostVideo::new(
                quality,
                (self.open_encoder)(settings.width, settings.height, settings.frame_rate, lower)?,
            );
            self.reopens += 1;
        }
        Ok(())
    }
}

/// The session an event belongs to, when it belongs to one. Events without
/// an owner (hosting changes, pairing, address updates) are observed by
/// every serve loop.
pub(crate) fn event_session(event: &CoreEvent) -> Option<u64> {
    match event {
        CoreEvent::QualityUpdated { id, .. }
        | CoreEvent::KeyframeRequested { id }
        | CoreEvent::InputFailed { id, .. }
        | CoreEvent::ClipboardReceived { id }
        | CoreEvent::ClipboardFilesReceived { id } => Some(*id),
        CoreEvent::SessionChanged { id, .. } => Some(*id),
        _ => None,
    }
}

/// Copy capture frames into `core` until `stop` is set or the hosted session ends.
///
/// The caller has already started `capture`. This function does not open a
/// capture. `stop` is read between frames, so a frame already being copied is
/// finished first.
pub fn serve_capture<C, E>(
    capture: &C,
    core: &Core,
    session_id: u64,
    quality: Quality,
    stop: &AtomicBool,
    open_encoder: impl Fn(u16, u16, u8, u32) -> Result<E, Error> + Send + 'static,
) -> Result<(), PublishError>
where
    C: FrameSource,
    E: VideoEncoder,
{
    let mut session = HostSession::open(capture.size().0, capture.size().1, quality, open_encoder)?;
    let events = core.subscribe()?;
    let mut sink = CoreSink::new(core);
    let separate_cursor = core.separate_cursor(session_id, vnilink::SessionRole::Host)?;
    let generation = core
        .snapshot()?
        .incoming
        .filter(|s| s.id == session_id)
        .ok_or(vnilink::Error::NoSession)?
        .generation;
    let started = std::time::Instant::now();
    loop {
        if stop.load(Ordering::Relaxed) {
            return Ok(());
        }
        match core.host_desktop_active(session_id, generation) {
            Ok(true) => {}
            Ok(false) | Err(vnilink::Error::ShutDown) => return Ok(()),
            Err(error) => return Err(error.into()),
        }
        while let Some(event) = events.try_recv() {
            // One installation can host while it controls another: a quality
            // or keyframe event for the outgoing session must not reconfigure
            // this hosted one.
            if event_session(&event).is_some_and(|id| id != session_id) {
                continue;
            }
            session.observe(&event)?;
        }
        let capture_started = std::time::Instant::now();
        let frame_period = std::time::Duration::from_micros(
            1_000_000 / u64::from(session.control.settings().frame_rate),
        );
        let timestamp_us = u64::try_from(started.elapsed().as_micros()).unwrap_or(u64::MAX);
        let Some(mut frame) = capture.poll_frame(timestamp_us)? else {
            std::thread::sleep(std::time::Duration::from_millis(10));
            continue;
        };
        offer_snapshot(core, capture, &frame);
        if let Some(cursor) = capture.cursor() {
            if separate_cursor {
                match core.publish_cursor(session_id, generation, cursor) {
                    Ok(()) => {}
                    Err(
                        vnilink::Error::NoSession
                        | vnilink::Error::StaleSession
                        | vnilink::Error::ShutDown,
                    ) => return Ok(()),
                    Err(error) => return Err(error.into()),
                }
            } else {
                frame = crate::cursor::composite(&frame, &cursor)?;
            }
        }
        match session.publish(&frame, &mut sink, session_id) {
            Ok(_) => {}
            Err(PublishError::Session(
                vnilink::Error::NoSession | vnilink::Error::StaleSession | vnilink::Error::ShutDown,
            )) => return Ok(()),
            Err(err) => return Err(err),
        }
        // A cached idle picture can be returned immediately. Pace requests,
        // rather than copying and discarding that picture in a busy loop.
        if let Some(remaining) = frame_period.checked_sub(capture_started.elapsed()) {
            std::thread::sleep(remaining);
        }
    }
}

fn offer_snapshot(core: &Core, capture: &impl FrameSource, frame: &RawFrame) {
    let Some(at) = capture.attested_at() else {
        return;
    };
    core.offer_attested_picture(
        at,
        frame.width(),
        frame.height(),
        frame.stride(),
        frame.pixels(),
        capture.attested_display(),
    );
}

/// Capture/encode for one neutral Desktop instance, never its parent session.
/// The caller owns the OS capture source and chooses the negotiated codec from
/// hosted_desktop_info before constructing its encoder factory.
pub fn serve_desktop_capture<C, E>(
    capture: &C,
    core: &Core,
    service_id: u64,
    generation: u64,
    stop: &AtomicBool,
    open_encoder: impl Fn(u16, u16, u8, u32) -> Result<E, Error> + Send + 'static,
) -> Result<(), PublishError>
where
    C: FrameSource,
    E: VideoEncoder,
{
    let mut info = core.hosted_desktop_info(service_id, generation)?;
    let mut session = HostSession::open(
        capture.size().0,
        capture.size().1,
        info.quality,
        open_encoder,
    )?;
    struct Sink<'a> {
        core: &'a Core,
        generation: u64,
    }
    impl FrameSink for Sink<'_> {
        fn publish_frame(
            &mut self,
            id: u64,
            frame: vnilink::EncodedFrame,
        ) -> Result<(), vnilink::Error> {
            self.core.publish_desktop_video(id, self.generation, frame)
        }
    }
    let mut sink = Sink { core, generation };
    let started = std::time::Instant::now();
    loop {
        if stop.load(Ordering::Relaxed) {
            return Ok(());
        }
        let current = match core.hosted_desktop_info(service_id, generation) {
            Ok(info) => info,
            Err(
                vnilink::Error::NoSession | vnilink::Error::StaleSession | vnilink::Error::ShutDown,
            ) => return Ok(()),
            Err(error) => return Err(error.into()),
        };
        if current.quality != info.quality {
            session.observe(&CoreEvent::QualityUpdated {
                id: service_id,
                quality: current.quality,
            })?;
            info = current;
        }
        if core
            .take_desktop_keyframe_request(service_id, generation)
            .unwrap_or(false)
        {
            session.observe(&CoreEvent::KeyframeRequested { id: service_id })?;
        }
        let capture_started = std::time::Instant::now();
        let timestamp = started.elapsed().as_micros().try_into().unwrap_or(u64::MAX);
        let Some(mut frame) = capture.poll_frame(timestamp)? else {
            std::thread::sleep(std::time::Duration::from_millis(10));
            continue;
        };
        offer_snapshot(core, capture, &frame);
        if let Some(cursor) = capture.cursor() {
            if info.separate_cursor {
                match core.publish_desktop_cursor(service_id, generation, cursor) {
                    Ok(()) => {}
                    Err(
                        vnilink::Error::NoSession
                        | vnilink::Error::StaleSession
                        | vnilink::Error::ShutDown,
                    ) => return Ok(()),
                    Err(error) => return Err(error.into()),
                }
            } else {
                frame = crate::cursor::composite(&frame, &cursor)?;
            }
        }
        match session.publish(&frame, &mut sink, service_id) {
            Ok(_) => {}
            Err(PublishError::Session(
                vnilink::Error::NoSession | vnilink::Error::StaleSession | vnilink::Error::ShutDown,
            )) => return Ok(()),
            Err(error) => return Err(error),
        }
        let period = std::time::Duration::from_micros(
            1_000_000 / u64::from(session.control.settings().frame_rate),
        );
        if let Some(remaining) = period.checked_sub(capture_started.elapsed()) {
            std::thread::sleep(remaining);
        }
    }
}
