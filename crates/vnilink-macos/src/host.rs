//! Publish one monitor into a hosted session.
//!
//! The generic [`HostSession`] opens the VideoToolbox encoder at the even size
//! that fits the session quality. This module only pins that choice to
//! [`MacEncoder`], reads ScreenCaptureKit frames as the [`FrameSource`], and
//! serves an already-open capture. It does not show a permission dialog, and it
//! does not draw. Frame bytes are not logged.

#[cfg(target_os = "macos")]
use std::sync::atomic::{AtomicBool, Ordering};

pub use vnilink_media::HostSession;
#[cfg(target_os = "macos")]
use vnilink_media::{Error, FrameSource, PublishError, Quality, RawFrame};

#[cfg(target_os = "macos")]
use crate::encode::MacVideoEncoder;

#[cfg(target_os = "macos")]
impl FrameSource for crate::capture::ScreenCapture {
    fn size(&self) -> (u16, u16) {
        self.size()
    }

    fn poll_frame(&self, timestamp_us: u64) -> Result<Option<RawFrame>, Error> {
        self.poll_frame(timestamp_us)
    }
}

/// Copy ScreenCaptureKit frames into `core` until `stop` is set or the hosted
/// session ends.
///
/// The caller has already started `capture`. This function does not ask for
/// Screen Recording permission. `stop` is read between frames, so a frame
/// already being copied is finished first. The encoder profile follows the
/// negotiated session codec.
#[cfg(target_os = "macos")]
pub fn serve_stream(
    capture: &crate::capture::ScreenCapture,
    core: &vnilink::Core,
    session_id: u64,
    quality: Quality,
    stop: &AtomicBool,
) -> Result<(), PublishError> {
    let codec = core.host_codec(session_id).unwrap_or(vnilink::CODEC_H264);
    vnilink_media::serve_capture(
        capture,
        core,
        session_id,
        quality,
        stop,
        move |w, h, fps, bps| MacVideoEncoder::open(w, h, fps, bps, codec),
    )
}

/// Encode every live viewer from one ScreenCaptureKit stream.
///
/// `exit_when_idle` returns once no consumer remains so the caller can drop
/// the capture. Each viewer still has its own encoder. The cursor flag is
/// updated when the set of consumers changes: an embedded-cursor viewer burns
/// the cursor into the shared pictures, and a metadata viewer does not.
#[cfg(target_os = "macos")]
pub fn serve_shared(
    capture: &crate::capture::ScreenCapture,
    core: &vnilink::Core,
    stop: &AtomicBool,
    exit_when_idle: bool,
    capture_display: u64,
) -> Result<(), PublishError> {
    let embedded = std::sync::Arc::new(AtomicBool::new(false));
    let source = SharedSource {
        inner: capture,
        embedded: std::sync::Arc::clone(&embedded),
        applied: AtomicBool::new(false),
        known: AtomicBool::new(false),
    };
    vnilink_media::serve_shared_capture(
        &source,
        core,
        stop,
        exit_when_idle,
        capture_display,
        move |targets| {
            embedded.store(
                targets.iter().any(|target| !target.separate_cursor),
                Ordering::Relaxed,
            );
        },
        |target, width, height, fps, bps| {
            let codec = target.codec;
            MacVideoEncoder::open(width, height, fps, bps, codec)
        },
    )
}

#[cfg(target_os = "macos")]
struct SharedSource<'a> {
    inner: &'a crate::capture::ScreenCapture,
    embedded: std::sync::Arc<AtomicBool>,
    applied: AtomicBool,
    known: AtomicBool,
}

#[cfg(target_os = "macos")]
impl FrameSource for SharedSource<'_> {
    fn size(&self) -> (u16, u16) {
        self.inner.size()
    }

    fn poll_frame(&self, timestamp_us: u64) -> Result<Option<RawFrame>, Error> {
        let embedded = self.embedded.load(Ordering::Relaxed);
        let first = !self.known.swap(true, Ordering::Relaxed);
        let previous = self.applied.swap(embedded, Ordering::Relaxed);
        if first || previous != embedded {
            self.inner.set_shows_cursor(embedded)?;
        }
        self.inner.poll_frame(timestamp_us)
    }
}

/// Encode for one admitted Desktop service, stopping when that owner ends.
/// The caller owns ScreenCaptureKit permission and the capture resource.
#[cfg(target_os = "macos")]
pub fn serve_desktop(
    capture: &crate::capture::ScreenCapture,
    core: &vnilink::Core,
    service_id: u64,
    generation: u64,
    stop: &AtomicBool,
) -> Result<(), PublishError> {
    let info = core.hosted_desktop_info(service_id, generation)?;
    let codec = info.codec;
    vnilink_media::serve_desktop_capture(
        capture,
        core,
        service_id,
        generation,
        stop,
        move |w, h, fps, bps| MacVideoEncoder::open(w, h, fps, bps, codec),
    )
}
