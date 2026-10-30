// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! Publish one monitor into a hosted Desktop service.
//!
//! The generic [`HostSession`] opens the Media Foundation encoder at the even
//! size that fits the session quality. This module only pins that choice to
//! [`crate::WindowsEncoder`] and reads Desktop Duplication frames as the
//! [`FrameSource`]. It does not reach the secure desktop, and it does not
//! draw. Frame bytes are not logged.

pub use vnilink_media::HostSession;
#[cfg(windows)]
use vnilink_media::{Error, FrameSource, FrameWait, RawFrame};

#[cfg(windows)]
impl FrameSource for crate::capture::WindowsCapture {
    fn size(&self) -> (u16, u16) {
        self.size()
    }

    /// A zero-timeout native acquire, with cached pixels for an idle desktop.
    fn poll_frame(&self, timestamp_us: u64) -> Result<Option<RawFrame>, Error> {
        self.poll_frame(timestamp_us)
    }

    /// Blocks in Desktop Duplication until the next picture or `deadline`.
    fn wait_frame(&self, deadline: std::time::Instant) -> FrameWait {
        FrameWait::from_arrived(crate::capture::WindowsCapture::wait_frame(self, deadline))
    }

    fn attested_at(&self) -> Option<std::time::Instant> {
        crate::capture::WindowsCapture::attested_at(self)
    }

    fn attested_display(&self) -> &str {
        "0"
    }

    fn cursor(&self) -> Option<vnilink::CursorState> {
        crate::capture::WindowsCapture::cursor(self)
    }
}
