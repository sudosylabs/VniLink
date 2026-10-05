//! One controlling session: decode ordered frames for the UI to paint.
//!
//! The mirror of [`crate::HostSession`]. The transport delivers encoded
//! frames in order; the platform decoder turns them into BGRA pictures. A
//! `DecodeRejected` frame is skipped — the decoder has not seen a keyframe
//! yet or the frame is corrupt — while `Unsupported` ends the loop so the
//! UI can name the missing codec instead of showing a black screen.
//!
//! The loop takes a `next` closure rather than a
//! [`vnilink::VideoSubscription`] directly so tests can feed vectors. UI
//! callers pass `|timeout| subscription.recv_timeout(timeout)`.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use vnilink::EncodedFrame;

use crate::error::Error;
use crate::frame::RawFrame;
use crate::pump::VideoDecoder;

/// What the controller did with one encoded frame.
#[derive(Debug)]
pub enum ControllerOutput {
    Decoded(RawFrame),
    Skipped,
}

/// Decoder for one controlling session.
pub struct ControllerVideo<D> {
    decoder: D,
    decoded: u64,
    skipped: u64,
}

impl<D: VideoDecoder> ControllerVideo<D> {
    pub fn new(decoder: D) -> Self {
        Self {
            decoder,
            decoded: 0,
            skipped: 0,
        }
    }

    /// Decodes one frame. `DecodeRejected` becomes `Skipped`; any other
    /// error is returned so the caller can report it.
    pub fn push(&mut self, frame: &EncodedFrame) -> Result<ControllerOutput, Error> {
        match self.decoder.decode(frame) {
            Ok(picture) => {
                self.decoded += 1;
                Ok(ControllerOutput::Decoded(picture))
            }
            Err(Error::DecodeRejected(_)) => {
                self.skipped += 1;
                Ok(ControllerOutput::Skipped)
            }
            Err(error) => Err(error),
        }
    }

    pub fn decoded(&self) -> u64 {
        self.decoded
    }

    pub fn skipped(&self) -> u64 {
        self.skipped
    }
}

/// Pulls encoded frames until `stop` is set, decodes each, and hands BGRA
/// pictures to `paint`.
///
/// `paint` runs on the caller's thread; it must copy or hand off the pixels
/// without blocking the loop for long.
pub fn serve_controller<D, N>(
    next: &mut N,
    controller: &mut ControllerVideo<D>,
    stop: &AtomicBool,
    mut paint: impl FnMut(&RawFrame),
) -> Result<(), Error>
where
    D: VideoDecoder,
    N: FnMut(Duration) -> Option<EncodedFrame>,
{
    loop {
        if stop.load(Ordering::Relaxed) {
            return Ok(());
        }
        let Some(frame) = next(Duration::from_millis(200)) else {
            continue;
        };
        match controller.push(&frame)? {
            ControllerOutput::Decoded(picture) => paint(&picture),
            ControllerOutput::Skipped => {}
        }
    }
}
