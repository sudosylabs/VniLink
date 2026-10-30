// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! What the Windows adapter will open, without opening it.
//!
//! Desktop Duplication reads the interactive desktop of this session. The
//! secure UAC desktop is a different desktop and is not supported. Ordinary
//! input injection does not reach it either.

use vnilink_media::{CapturePlan, Error};

/// The secure credential and UAC desktop is never this adapter.
pub const SECURE_DESKTOP_SUPPORTED: bool = false;

/// Screenshot display. Windows capture reads the primary display only.
pub const SNAPSHOT_DISPLAY_ID: &str = "0";

/// Windows has no screen-capture consent call. Allow retries the primary display.
pub fn snapshot_prompts_for_consent(_prompt: bool) -> bool {
    false
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WindowsPlan {
    display_id: u64,
    width: u16,
    height: u16,
    frame_rate: u8,
    bitrate_bps: u32,
}

impl WindowsPlan {
    pub fn from_capture(plan: &CapturePlan, bitrate_kbps: u32) -> Result<Self, Error> {
        let bitrate_bps = bitrate_kbps
            .checked_mul(1_000)
            .ok_or(Error::InvalidSetting(
                "bitrate does not fit in bits per second",
            ))?;
        Ok(Self {
            display_id: plan.display_id(),
            width: plan.width(),
            height: plan.height(),
            frame_rate: plan.frame_rate(),
            bitrate_bps,
        })
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

    pub fn bitrate_bps(&self) -> u32 {
        self.bitrate_bps
    }

    pub fn secure_desktop_supported(&self) -> bool {
        SECURE_DESKTOP_SUPPORTED
    }
}

/// One captured desktop must fit the frame type the session already uses.
pub(crate) fn frame_size(width: u32, height: u32) -> Result<(u16, u16), Error> {
    let width = u16::try_from(width)
        .map_err(|_| Error::Unsupported("the desktop is larger than one frame"))?;
    let height = u16::try_from(height)
        .map_err(|_| Error::Unsupported("the desktop is larger than one frame"))?;
    if width == 0 || height == 0 {
        return Err(Error::CaptureRejected(
            "Desktop Duplication returned an empty frame",
        ));
    }
    Ok((width, height))
}
