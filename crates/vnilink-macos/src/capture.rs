//! One-monitor ScreenCaptureKit configuration.
//!
//! [`ScreenPlan`] only builds the stream configuration. [`ScreenCapture::start`]
//! lists shareable content and starts a stream, which asks for Screen Recording
//! permission. Tests construct the plan and do not call `start`. FileVault
//! preboot is not a logged-in display and is not captured.

use apple_cf::cm::CMTime;
use screencapturekit::shareable_content::SCShareableContent;
use screencapturekit::stream::configuration::{PixelFormat, SCStreamConfiguration};
use screencapturekit::stream::content_filter::SCContentFilter;
use screencapturekit::stream::SCStream;
use vnilink_media::{CapturePlan, CursorPolicy, Error};

/// FileVault preboot is not the logged-in display this adapter captures.
pub const FILEVAULT_PREBOOT_SUPPORTED: bool = false;

/// The one-monitor capture settings, before a stream exists.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScreenPlan {
    width: u16,
    height: u16,
    frame_rate: u8,
    shows_cursor: bool,
}

impl ScreenPlan {
    pub fn from_capture(plan: &CapturePlan) -> Result<Self, Error> {
        if plan.monitor_index() != 0 {
            return Err(Error::Unsupported("only one monitor is supported"));
        }
        if plan.frame_rate() == 0 {
            return Err(Error::InvalidSetting("frame rate must be non-zero"));
        }
        Ok(Self {
            width: plan.width(),
            height: plan.height(),
            frame_rate: plan.frame_rate(),
            shows_cursor: plan.cursor() == CursorPolicy::InFrame,
        })
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

    pub fn shows_cursor(&self) -> bool {
        self.shows_cursor
    }

    /// BGRA, one monitor, cursor hidden unless the plan asks for it.
    pub fn configuration(&self) -> SCStreamConfiguration {
        let interval = CMTime::new(1, i32::from(self.frame_rate));
        SCStreamConfiguration::new()
            .with_width(u32::from(self.width))
            .with_height(u32::from(self.height))
            .with_pixel_format(PixelFormat::BGRA)
            .with_shows_cursor(self.shows_cursor)
            .with_minimum_frame_interval(&interval)
    }
}

/// A started ScreenCaptureKit stream. Tests do not construct this.
pub struct ScreenCapture {
    _stream: SCStream,
    width: u16,
    height: u16,
}

impl ScreenCapture {
    /// List displays and start capture. This can prompt for Screen Recording.
    pub fn start(plan: &ScreenPlan) -> Result<Self, Error> {
        let content = SCShareableContent::get()
            .map_err(|_| Error::CaptureRejected("Screen Recording permission was not granted"))?;
        let displays = content.displays();
        let display = displays
            .first()
            .ok_or(Error::CaptureRejected("no display is available"))?;
        let filter = SCContentFilter::create()
            .with_display(display)
            .with_excluding_windows(&[])
            .build()
            .map_err(|_| Error::CaptureRejected("the display filter could not be built"))?;
        let stream = SCStream::new(&filter, &plan.configuration())
            .map_err(|_| Error::CaptureRejected("ScreenCaptureKit did not start"))?;
        stream
            .start_capture()
            .map_err(|_| Error::CaptureRejected("ScreenCaptureKit did not start"))?;
        Ok(Self {
            _stream: stream,
            width: plan.width(),
            height: plan.height(),
        })
    }

    pub fn size(&self) -> (u16, u16) {
        (self.width, self.height)
    }
}
