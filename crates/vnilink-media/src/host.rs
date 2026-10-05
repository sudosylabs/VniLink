//! One hosted monitor: encoder plus pacing plus publish.
//!
//! [`HostSession`] opens the platform encoder at the settings its
//! [`RateController`] derives from the session quality, then paces pictures
//! through [`HostVideo`]. [`serve_capture`] reads a capture that is already
//! open. It does not ask for capture permission and does not draw. Frame bytes
//! are not logged. Stop is noticed between pictures. A session that is no
//! longer active ends the loop.

use std::sync::atomic::{AtomicBool, Ordering};

use vnilink::Core;
use vnilink::CoreEvent;

use crate::error::{Error, PublishError};
use crate::frame::RawFrame;
use crate::pump::{CoreSink, FrameSink, HostVideo, PumpOutput, VideoEncoder};
use crate::rate::RateController;
use crate::Quality;

/// Where paced pictures come from. Both portal and ScreenCaptureKit captures satisfy it.
pub trait FrameSource {
    fn size(&self) -> (u16, u16);
    fn next_frame(&self, timestamp_us: u64) -> Result<RawFrame, Error>;
}

/// Platform encoder constructor, kept so a later quality change can reopen the
/// encoder at the new size. Both platform encoders satisfy it as a function.
pub type OpenEncoder<E> = Box<dyn Fn(u16, u16, u8, u32) -> Result<E, Error> + Send>;

/// Encoder for one captured monitor.
pub struct HostSession<E> {
    control: RateController,
    open_encoder: OpenEncoder<E>,
    video: HostVideo<E>,
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
        })
    }

    pub fn capture_size(&self) -> (u16, u16) {
        self.control.capture_size()
    }

    pub fn encoded_size(&self) -> (u16, u16) {
        self.control.encoded_size()
    }

    /// A keyframe request forces the next picture. A quality change reopens
    /// the encoder when the controller decides the settings no longer match.
    pub fn observe(&mut self, event: &CoreEvent) -> Result<(), Error> {
        match event {
            CoreEvent::QualityUpdated { quality, .. } => {
                if self.control.observe(event)? {
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
        self.video.publish_to(sink, session_id, frame)
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
        | CoreEvent::TransferCompleted { id } => Some(*id),
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
    let started = std::time::Instant::now();
    loop {
        if stop.load(Ordering::Relaxed) {
            return Ok(());
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
        let timestamp_us = u64::try_from(started.elapsed().as_micros()).unwrap_or(u64::MAX);
        let frame = capture.next_frame(timestamp_us)?;
        match session.publish(&frame, &mut sink, session_id) {
            Ok(_) => {}
            Err(PublishError::Session(vnilink::Error::NoSession)) => return Ok(()),
            Err(err) => return Err(err),
        }
    }
}
