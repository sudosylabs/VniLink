// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! Publish one monitor into a hosted Desktop service.
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
use vnilink_media::{Error, FrameSource, FrameWait, PublishError, RawFrame};

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

    fn wait_frame(&self, deadline: std::time::Instant) -> FrameWait {
        FrameWait::from_arrived(crate::capture::ScreenCapture::wait_frame(self, deadline))
    }

    fn attested_at(&self) -> Option<std::time::Instant> {
        crate::capture::ScreenCapture::attested_at(self)
    }
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
            MacVideoEncoder::open(width, height, fps, bps, codec, target.matrix)
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

    fn full_size(&self) -> (u16, u16) {
        self.inner.full_size()
    }

    /// ScreenCaptureKit scales the display as it captures it.
    fn deliver_at(&self, size: (u16, u16)) -> Result<bool, Error> {
        Ok(self.inner.deliver_at(size))
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

    fn wait_frame(&self, deadline: std::time::Instant) -> FrameWait {
        FrameWait::from_arrived(self.inner.wait_frame(deadline))
    }

    /// The capture's own: without it the shared capture could never tell an
    /// unchanged screen, and would encode it at the full frame rate and
    /// never send a still picture of it.
    fn attested_at(&self) -> Option<std::time::Instant> {
        self.inner.attested_at()
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
    let (codec, matrix) = (info.codec, info.matrix);
    vnilink_media::serve_desktop_capture(
        capture,
        core,
        service_id,
        generation,
        stop,
        move |w, h, fps, bps| MacVideoEncoder::open(w, h, fps, bps, codec, matrix),
    )
}
