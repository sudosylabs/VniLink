// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! One hosted monitor: encoder plus pacing plus publish.
//!
//! [`HostSession`] opens the platform encoder at the settings its
//! [`RateController`] derives from the session quality, then paces pictures
//! through [`HostVideo`]. [`serve_desktop_capture`] reads a capture that is
//! already open for one hosted Desktop service. It does not ask for capture
//! permission and does not draw. Frame bytes are not logged. Stop is noticed
//! between bounded reads, even before the first picture. A Desktop that is no
//! longer active ends the loop.
// trace: records every ignored failure

use std::sync::atomic::{AtomicBool, Ordering};

use vnilink::Core;

use crate::error::{Error, PublishError};
use crate::frame::RawFrame;
use crate::pump::{FrameSink, HostVideo, PumpOutput, VideoEncoder};
use crate::rate::{RateChange, RateController};
use crate::Quality;

/// How [`FrameSource::wait_frame`] ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameWait {
    /// A picture the last poll did not return is ready.
    Arrived,
    /// The deadline passed with none.
    TimedOut,
    /// This source cannot wait for pictures; it returned at once.
    Unsupported,
}

impl FrameWait {
    /// What a source's wait says when it can wait: whether a picture came.
    pub fn from_arrived(arrived: bool) -> Self {
        if arrived {
            Self::Arrived
        } else {
            Self::TimedOut
        }
    }
}

/// Where paced pictures come from. Both portal and ScreenCaptureKit captures satisfy it.
pub trait FrameSource {
    fn size(&self) -> (u16, u16);
    /// One bounded native read. `None` means no first picture yet, not failure.
    /// An idle source may return its cached picture with the new timestamp.
    /// Implementations must not wait indefinitely for desktop damage: owners
    /// and shutdown are checked between reads. Healthy native reads finish
    /// within 100 ms; native API/device failure is returned as an error.
    fn poll_frame(&self, timestamp_us: u64) -> Result<Option<RawFrame>, Error>;
    /// Waits until a picture the last [`Self::poll_frame`] did not return
    /// is ready, or until `deadline`. A source that is told of each picture
    /// as it arrives wakes then, so the next poll finds it at once. Waits end
    /// at `deadline` at the latest, so the caller's stop flags are seen
    /// between them. The default does not wait: the caller times its polls.
    fn wait_frame(&self, deadline: std::time::Instant) -> FrameWait {
        let _ = deadline;
        FrameWait::Unsupported
    }
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
    /// The largest pictures this source delivers: the display's own pixels.
    fn full_size(&self) -> (u16, u16) {
        self.size()
    }
    /// Asks the platform to deliver pictures of `size`, at most
    /// [`Self::full_size`], scaled as it captures them, so that nothing
    /// scales them again before encoding. `Ok(true)` once later pictures
    /// come at that size and [`Self::size`] says so; `Ok(false)` from a
    /// source that delivers only its own size, or whose platform refused.
    fn deliver_at(&self, size: (u16, u16)) -> Result<bool, Error> {
        let _ = size;
        Ok(false)
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

    /// Whether an unchanged picture is still to be encoded at the frame
    /// rate, for the encoder to refine the latest keyframe.
    pub fn refining(&self) -> bool {
        self.video.refining()
    }

    /// The open encoder; see [`VideoEncoder::implementation`].
    pub fn implementation(&self) -> vnilink::EncoderImplementation {
        self.video.implementation()
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

    /// The viewer lost pictures: the next one is a recovery keyframe.
    pub fn request_recovery_keyframe(&mut self) {
        self.video.request_recovery_keyframe();
    }

    /// Follows the viewer's new limits. A change at the same size retunes
    /// the running encoder, without a keyframe; a new size, or an encoder
    /// that cannot retune, opens it again.
    pub fn set_quality(&mut self, quality: Quality) -> Result<(), Error> {
        self.capped_bps = None;
        let reopen = match self.control.update(quality)? {
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
                quality,
                (self.open_encoder)(
                    settings.width,
                    settings.height,
                    settings.frame_rate,
                    settings.bitrate_bps,
                )?,
            );
            self.reopens += 1;
        } else {
            self.video.set_quality(quality);
        }
        Ok(())
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
            // starts from a recovery keyframe, at a quarter less bitrate.
            Err(PublishError::Media(error)) if error.is_too_large() => {
                tracing::debug!(
                    target: vnilink::trace::ENCODE,
                    "an encoded picture was too large to send: the next is a smaller keyframe"
                );
                self.shrink()?;
                self.video.request_recovery_keyframe();
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

/// Records a screen read that failed, which ends the pipeline reading it.
fn read_failed(error: &Error) {
    tracing::warn!(
        target: vnilink::trace::CAPTURE,
        failure = vnilink::trace::Failure::failure_name(error),
        reason = vnilink::trace::Failure::failure_reason(error),
        "the screen could not be read: capture ends"
    );
}

fn offer_snapshot(core: &Core, capture: &impl FrameSource, frame: &RawFrame) {
    let Some(at) = capture.attested_at() else {
        return;
    };
    // Reading a platform-held picture's pixels copies it: only when wanted.
    if !core.wants_attested_picture(at) {
        return;
    }
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
    let _engine = core.trace_span().entered();
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
            session.set_quality(current.quality)?;
            info = current;
        }
        if core
            .take_desktop_keyframe_request(service_id, generation)
            .unwrap_or(false)
        {
            session.request_recovery_keyframe();
        }
        let capture_started = std::time::Instant::now();
        let timestamp = started.elapsed().as_micros().try_into().unwrap_or(u64::MAX);
        let Some(mut frame) = capture.poll_frame(timestamp).inspect_err(read_failed)? else {
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
                frame = crate::cursor::composite(&frame, &cursor).inspect_err(|error| {
                    tracing::warn!(
                        target: vnilink::trace::CAPTURE,
                        failure = vnilink::trace::Failure::failure_name(error),
                        reason = vnilink::trace::Failure::failure_reason(error),
                        "the pointer could not be drawn into the picture: capture ends"
                    );
                })?;
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
