//! Publish one monitor into a hosted session.
//!
//! [`HostSession`] opens the VA-API encoder at the even size that fits the
//! session quality, then paces pictures through [`vnilink_media::HostVideo`].
//! [`serve_portal`] reads a portal capture that is already open. It does not
//! show a permission dialog, and it does not draw. Frame bytes are not logged.
//! Stop is noticed between pictures. A session that is no longer active ends
//! the loop.

use std::sync::atomic::{AtomicBool, Ordering};

use vnilink::CoreEvent;
use vnilink_media::{
    encoded_size, CoreSink, Error, FrameSink, HostVideo, PublishError, PumpOutput, Quality,
    RawFrame,
};

use crate::encode::LinuxEncoder;

/// Encoder for one captured monitor.
pub struct HostSession {
    capture_width: u16,
    capture_height: u16,
    encoded_width: u16,
    encoded_height: u16,
    bitrate_bps: u32,
    frame_rate: u8,
    video: HostVideo<LinuxEncoder>,
}

impl HostSession {
    /// Open the encoder for `capture_width` by `capture_height` at `quality`.
    ///
    /// The encoder size follows the quality limit and stays even. On a machine
    /// without Constrained Baseline this returns [`Error::Unsupported`].
    pub fn open(capture_width: u16, capture_height: u16, quality: Quality) -> Result<Self, Error> {
        let (encoded_width, encoded_height) = encoded_size(
            capture_width,
            capture_height,
            quality.max_width,
            quality.max_height,
        )?;
        let bitrate_bps = bitrate_bps(quality)?;
        let video = HostVideo::new(
            quality,
            LinuxEncoder::open(
                encoded_width,
                encoded_height,
                quality.max_frame_rate,
                bitrate_bps,
            )?,
        );
        Ok(Self {
            capture_width,
            capture_height,
            encoded_width,
            encoded_height,
            bitrate_bps,
            frame_rate: quality.max_frame_rate,
            video,
        })
    }

    pub fn capture_size(&self) -> (u16, u16) {
        (self.capture_width, self.capture_height)
    }

    pub fn encoded_size(&self) -> (u16, u16) {
        (self.encoded_width, self.encoded_height)
    }

    /// A keyframe request forces the next picture. A quality change reopens
    /// the encoder when the size, frame rate, or bitrate it was opened with
    /// no longer matches.
    pub fn observe(&mut self, event: &CoreEvent) -> Result<(), Error> {
        match event {
            CoreEvent::QualityUpdated { quality, .. } => self.apply_quality(*quality),
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
        if frame.width() != self.capture_width || frame.height() != self.capture_height {
            return Err(Error::InvalidFrame("the frame size does not match the encoder").into());
        }
        self.video.publish_to(sink, session_id, frame)
    }

    fn apply_quality(&mut self, quality: Quality) -> Result<(), Error> {
        let (width, height) = encoded_size(
            self.capture_width,
            self.capture_height,
            quality.max_width,
            quality.max_height,
        )?;
        let bitrate_bps = bitrate_bps(quality)?;
        if width == self.encoded_width
            && height == self.encoded_height
            && bitrate_bps == self.bitrate_bps
            && quality.max_frame_rate == self.frame_rate
        {
            self.video.set_quality(quality);
            return Ok(());
        }
        self.video = HostVideo::new(
            quality,
            LinuxEncoder::open(width, height, quality.max_frame_rate, bitrate_bps)?,
        );
        self.encoded_width = width;
        self.encoded_height = height;
        self.bitrate_bps = bitrate_bps;
        self.frame_rate = quality.max_frame_rate;
        Ok(())
    }
}

/// Copy portal frames into `core` until `stop` is set or the hosted session ends.
///
/// The caller has already started `capture`. This function does not open a
/// portal. `stop` is read between frames, so a frame already being copied is
/// finished first.
#[cfg(target_os = "linux")]
pub fn serve_portal(
    capture: &crate::pipewire::PortalCapture,
    core: &vnilink::Core,
    session_id: u64,
    quality: Quality,
    stop: &AtomicBool,
) -> Result<(), PublishError> {
    let mut session = HostSession::open(capture.size().0, capture.size().1, quality)?;
    let events = core.subscribe()?;
    let mut sink = CoreSink::new(core);
    let started = std::time::Instant::now();
    loop {
        if stop.load(Ordering::Relaxed) {
            return Ok(());
        }
        while let Some(event) = events.try_recv() {
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

fn bitrate_bps(quality: Quality) -> Result<u32, Error> {
    quality
        .max_bitrate_kbps
        .checked_mul(1_000)
        .ok_or(Error::InvalidSetting(
            "bitrate does not fit in bits per second",
        ))
}
