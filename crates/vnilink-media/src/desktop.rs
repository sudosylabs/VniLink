// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! Which desktop this process can host, and the one-monitor capture plan.
//!
//! Opening a capture still belongs to the platform crate. This module only
//! records the choice and the limits the encoder will use.

use vnilink::HostCapture;
use vnilink::Quality;

use crate::error::Error;
use crate::frame::encoded_size;

/// The desktop session a platform adapter knows how to capture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DesktopKind {
    MacOs,
    Windows,
    LinuxX11,
    LinuxWayland,
}

impl DesktopKind {
    /// Desktops this process can build an adapter for.
    ///
    /// Linux lists X11 and the Wayland portal separately. One of them working
    /// does not imply the other.
    pub fn on_this_process() -> &'static [DesktopKind] {
        #[cfg(target_os = "macos")]
        {
            &[DesktopKind::MacOs]
        }
        #[cfg(target_os = "windows")]
        {
            &[DesktopKind::Windows]
        }
        #[cfg(target_os = "linux")]
        {
            &[DesktopKind::LinuxX11, DesktopKind::LinuxWayland]
        }
        #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
        {
            &[]
        }
    }

    /// What this adapter does not claim, even after a successful session.
    pub fn limitation(self) -> &'static str {
        match self {
            Self::MacOs => {
                "ScreenCaptureKit captures one logged-in display. FileVault preboot is not supported."
            }
            Self::Windows => {
                "Desktop Duplication captures the interactive desktop of this session. The secure UAC desktop is not supported."
            }
            Self::LinuxX11 => "X11 capture uses the current X session. It does not see a Wayland session.",
            Self::LinuxWayland => {
                "The portal grants one source and does not enumerate other monitors. It is not universal unattended access."
            }
        }
    }
}

/// Whether the cursor is drawn into the captured picture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CursorPolicy {
    /// The picture does not include the cursor. The product can send it separately.
    Hidden,
    /// The cursor is burned into the frame.
    InFrame,
}

/// One selected display, at the even size and frame rate taken from [`Quality`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CapturePlan {
    display_id: u64,
    width: u16,
    height: u16,
    frame_rate: u8,
    cursor: CursorPolicy,
}

impl CapturePlan {
    /// Capture settings for one stable display id. Id zero is not a display.
    pub fn for_display(display_id: u64, quality: Quality) -> Result<Self, Error> {
        if display_id == 0 {
            return Err(Error::InvalidSetting("a display id is invalid"));
        }
        let (width, height) = encoded_size(
            quality.max_width,
            quality.max_height,
            quality.max_width,
            quality.max_height,
        )?;
        Ok(Self {
            display_id,
            width,
            height,
            frame_rate: quality.max_frame_rate,
            cursor: CursorPolicy::Hidden,
        })
    }

    pub fn with_cursor(mut self, cursor: CursorPolicy) -> Self {
        self.cursor = cursor;
        self
    }

    pub fn display_id(&self) -> u64 {
        self.display_id
    }

    pub fn width(&self) -> u16 {
        self.width
    }

    pub fn height(&self) -> u16 {
        self.height
    }

    pub fn frame_rate(&self) -> u8 {
        self.frame_rate
    }

    pub fn cursor(&self) -> CursorPolicy {
        self.cursor
    }
}

/// Advertises a monitor size to [`vnilink::Core::set_capture`]. It does not capture.
#[derive(Debug, Clone, Copy)]
pub struct MonitorSize {
    width: u16,
    height: u16,
}

impl MonitorSize {
    pub fn new(width: u16, height: u16) -> Self {
        Self { width, height }
    }

    pub fn from_plan(plan: &CapturePlan) -> Self {
        Self {
            width: plan.width,
            height: plan.height,
        }
    }
}

impl HostCapture for MonitorSize {
    fn monitor(&self) -> (u16, u16) {
        (self.width, self.height)
    }
}
