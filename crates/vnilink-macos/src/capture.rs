//! One-monitor ScreenCaptureKit configuration and frame delivery.
//!
//! [`ScreenPlan`] only builds the stream configuration. [`ScreenCapture::start`]
//! lists shareable content and starts a stream, which asks for Screen Recording
//! permission. The stream handler pushes each copied picture into a
//! [`FrameChannel`]; [`ScreenCapture::next_frame`] receives the newest one it
//! has not seen yet. Tests construct the plan and do not call `start`.
//! FileVault preboot is not a logged-in display and is not captured.

use vnilink_media::{CapturePlan, ChannelSlot, CursorPolicy, Error, FrameChannel};

#[cfg(target_os = "macos")]
use std::sync::Arc;
#[cfg(target_os = "macos")]
use std::time::Duration;

#[cfg(target_os = "macos")]
use screencapturekit::cm::{CMSampleBuffer, CMSampleBufferExt};
#[cfg(target_os = "macos")]
use screencapturekit::shareable_content::SCShareableContent;
#[cfg(target_os = "macos")]
use screencapturekit::stream::content_filter::SCContentFilter;
#[cfg(target_os = "macos")]
use screencapturekit::stream::output_trait::SCStreamOutputTrait;
#[cfg(target_os = "macos")]
use screencapturekit::stream::output_type::SCStreamOutputType;
#[cfg(target_os = "macos")]
use screencapturekit::stream::SCStream;

#[cfg(target_os = "macos")]
use apple_cf::cm::CMTime;
#[cfg(target_os = "macos")]
use screencapturekit::stream::configuration::{PixelFormat, SCStreamConfiguration};

#[cfg(target_os = "macos")]
const FRAME_TIMEOUT: Duration = Duration::from_secs(2);

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
    #[cfg(target_os = "macos")]
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

#[cfg(target_os = "macos")]
struct Handler {
    channel: Arc<FrameChannel>,
}

#[cfg(target_os = "macos")]
impl SCStreamOutputTrait for Handler {
    fn did_output_sample_buffer(&self, sample: CMSampleBuffer, of_type: SCStreamOutputType) {
        if of_type != SCStreamOutputType::Screen {
            return;
        }
        let Some(pixel_buffer) = sample.pixel_buffer() else {
            // Idle/status samples have no picture. They do not end capture.
            return;
        };
        // Timestamp is applied by `next_frame`; the handler only keeps pixels.
        match crate::pixels::raw_frame_from_pixel_buffer(&pixel_buffer, 0) {
            Ok(frame) => self.channel.push(ChannelSlot {
                width: frame.width(),
                height: frame.height(),
                stride: usize::from(frame.width()) * 4,
                bytes: frame.pixels().to_vec(),
            }),
            Err(Error::Unsupported(reason)) => self.channel.fail(reason),
            Err(_) => self
                .channel
                .fail("ScreenCaptureKit did not deliver a frame"),
        }
    }
}

/// A started ScreenCaptureKit stream. Tests do not construct this.
#[cfg(target_os = "macos")]
pub struct ScreenCapture {
    stream: SCStream,
    width: u16,
    height: u16,
    channel: Arc<FrameChannel>,
    last: std::sync::Mutex<Option<ChannelSlot>>,
}

#[cfg(target_os = "macos")]
impl ScreenCapture {
    /// List displays and start capture. This can prompt for Screen Recording.
    pub fn start(plan: &ScreenPlan) -> Result<Self, Error> {
        Self::start_display(plan, None)
    }

    /// Capture the selected main display rather than assuming enumeration order.
    pub fn start_display(plan: &ScreenPlan, display_id: Option<u32>) -> Result<Self, Error> {
        let content = SCShareableContent::get()
            .map_err(|_| Error::CaptureRejected("Screen Recording permission was not granted"))?;
        let displays = content.displays();
        let display = displays
            .iter()
            .find(|display| display_id.is_none_or(|id| display.display_id() == id))
            .ok_or(Error::CaptureRejected("no display is available"))?;
        let filter = SCContentFilter::create()
            .with_display(display)
            .with_excluding_windows(&[])
            .build()
            .map_err(|_| Error::CaptureRejected("the display filter could not be built"))?;
        let mut stream = SCStream::new(&filter, &plan.configuration())
            .map_err(|_| Error::CaptureRejected("ScreenCaptureKit did not start"))?;
        let channel = Arc::new(FrameChannel::new());
        stream
            .add_output_handler(
                Handler {
                    channel: Arc::clone(&channel),
                },
                SCStreamOutputType::Screen,
            )
            .map_err(|_| Error::CaptureRejected("ScreenCaptureKit did not start"))?;
        stream
            .start_capture()
            .map_err(|_| Error::CaptureRejected("ScreenCaptureKit did not start"))?;
        Ok(Self {
            stream,
            width: plan.width(),
            height: plan.height(),
            channel,
            last: std::sync::Mutex::new(None),
        })
    }

    pub fn size(&self) -> (u16, u16) {
        (self.width, self.height)
    }

    /// Block until a frame newer than the last one is copied.
    pub fn next_frame(&self, timestamp_us: u64) -> Result<vnilink_media::RawFrame, Error> {
        let received = self.channel.recv_timeout(FRAME_TIMEOUT, || {
            std::thread::sleep(Duration::from_millis(10));
        });
        // ScreenCaptureKit may stop producing pictures while the desktop is idle.
        // Keep the last picture available for keyframe requests and new sessions.
        let slot = repeat_idle_frame(received, &self.last)?;
        crate::pixels::raw_frame_from_bgra(
            slot.width,
            slot.height,
            slot.stride,
            timestamp_us,
            &slot.bytes,
        )
    }
}

#[cfg(target_os = "macos")]
fn repeat_idle_frame(
    received: Result<ChannelSlot, Error>,
    last: &std::sync::Mutex<Option<ChannelSlot>>,
) -> Result<ChannelSlot, Error> {
    let mut last = last.lock().unwrap_or_else(|e| e.into_inner());
    match received {
        Ok(slot) => {
            *last = Some(slot.clone());
            Ok(slot)
        }
        Err(Error::CaptureRejected(reason)) => last.clone().ok_or(Error::CaptureRejected(reason)),
        Err(error) => Err(error),
    }
}

#[cfg(all(test, target_os = "macos"))]
mod idle_tests {
    use super::*;
    #[test]
    fn idle_repeats_the_last_picture_but_failure_does_not() {
        let last = std::sync::Mutex::new(None);
        assert!(repeat_idle_frame(Err(Error::CaptureRejected("timeout")), &last).is_err());
        repeat_idle_frame(
            Ok(ChannelSlot {
                width: 2,
                height: 2,
                stride: 8,
                bytes: vec![19; 16],
            }),
            &last,
        )
        .unwrap();
        let repeated = repeat_idle_frame(Err(Error::CaptureRejected("timeout")), &last).unwrap();
        assert_eq!(repeated.bytes, vec![19; 16]);
        assert!(repeat_idle_frame(Err(Error::Unsupported("capture failed")), &last).is_err());
    }
}

#[cfg(target_os = "macos")]
impl Drop for ScreenCapture {
    fn drop(&mut self) {
        let _ = self.stream.stop_capture();
    }
}
