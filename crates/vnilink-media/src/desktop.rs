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
                "A portal session is one compositor's permission. It is not universal unattended access."
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

/// One monitor, at the even size and frame rate taken from [`Quality`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CapturePlan {
    monitor_index: u32,
    width: u16,
    height: u16,
    frame_rate: u8,
    cursor: CursorPolicy,
}

impl CapturePlan {
    pub fn one_monitor(quality: Quality) -> Result<Self, Error> {
        Self::for_monitor(0, quality)
    }

    pub fn for_monitor(monitor_index: u32, quality: Quality) -> Result<Self, Error> {
        if monitor_index != 0 {
            return Err(Error::Unsupported("only one monitor is supported"));
        }
        let (width, height) = encoded_size(
            quality.max_width,
            quality.max_height,
            quality.max_width,
            quality.max_height,
        )?;
        Ok(Self {
            monitor_index,
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

    pub fn monitor_index(&self) -> u32 {
        self.monitor_index
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
