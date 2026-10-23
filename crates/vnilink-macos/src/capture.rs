//! One-monitor ScreenCaptureKit configuration and frame delivery.
//!
//! [`ScreenPlan`] only builds the stream configuration. [`ScreenCapture::start`]
//! lists shareable content and starts a stream, which asks for Screen Recording
//! permission. The stream handler pushes each copied picture into a
//! [`FrameChannel`]; [`ScreenCapture::poll_frame`] returns current pixels
//! immediately, including a cached idle picture. Tests construct the plan and do not call `start`.
//! FileVault preboot is not a logged-in display and is not captured.

use vnilink_media::{CapturePlan, ChannelSlot, CursorPolicy, Error, FrameChannel};

#[cfg(target_os = "macos")]
use std::sync::Arc;

#[cfg(target_os = "macos")]
use screencapturekit::cm::{CMSampleBuffer, CMSampleBufferExt, CMSampleBufferSCExt};
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

/// FileVault preboot is not the logged-in display this adapter captures.
pub const FILEVAULT_PREBOOT_SUPPORTED: bool = false;

/// Pixel width divided by the point width, in percent. A missing or absurd
/// ratio stays at 100 so one odd display does not fail enumeration by itself.
pub fn display_scale(pixel_width: u32, point_width: f64) -> u16 {
    if !point_width.is_finite() || point_width <= 0.0 {
        return 100;
    }
    let computed = f64::from(pixel_width) / point_width * 100.0;
    if !computed.is_finite() {
        return 100;
    }
    let rounded = computed.round();
    if !(25.0..=800.0).contains(&rounded) {
        return 100;
    }
    rounded as u16
}

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

    /// A short, cursor-free stream sized for one thumbnail. The caller picks
    /// a fitted size so the output keeps the display's aspect ratio.
    pub fn snapshot(width: u16, height: u16) -> Result<Self, Error> {
        if width == 0
            || height == 0
            || width > vnilink::snapshot::MAX_SNAPSHOT_WIDTH
            || height > vnilink::snapshot::MAX_SNAPSHOT_HEIGHT
        {
            return Err(Error::InvalidSetting("snapshot size is out of range"));
        }
        Ok(Self {
            width,
            height,
            frame_rate: 5,
            shows_cursor: false,
        })
    }

    /// BGRA, one monitor, cursor hidden unless the plan asks for it.
    ///
    /// Five buffers in the stream's queue: pictures are held in their
    /// IOSurfaces (the newest in the channel, the shared capture's latest,
    /// one being encoded), and the stream still needs free ones to fill.
    #[cfg(target_os = "macos")]
    pub fn configuration(&self) -> SCStreamConfiguration {
        let interval = CMTime::new(1, i32::from(self.frame_rate));
        SCStreamConfiguration::new()
            .with_queue_depth(5)
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
    /// The picture last pushed: a frame with the same pixels is not a new
    /// one. Without this an unchanged screen would read as changing at the
    /// frame rate (see `pixels::same_picture`).
    previous: std::sync::Mutex<Option<apple_cf::cv::CVPixelBuffer>>,
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
        // The same pixels again leave the channel as it is: its picture and
        // when the source attested it do not change, so the shared capture
        // sees an unchanged screen.
        if let Some(kept) = sample.pixel_buffer() {
            let mut previous = self
                .previous
                .lock()
                .unwrap_or_else(|poison| poison.into_inner());
            if previous
                .as_ref()
                .is_some_and(|previous| crate::pixels::same_picture(previous, &kept))
            {
                return;
            }
            *previous = Some(kept);
        }
        // The stream timestamp is applied by `poll_frame`; the handler keeps
        // the pixels and when the display showed them.
        let captured_at = sample
            .display_time()
            .and_then(crate::clock::host_time_instant);
        // The picture stays in its IOSurface: VideoToolbox encodes it there,
        // and whatever else reads it copies it out once.
        match crate::pixels::MacPicture::new(pixel_buffer) {
            Ok(picture) => {
                use vnilink_media::NativePicture;
                let (width, height) = picture.size();
                self.channel.push(ChannelSlot {
                    width,
                    height,
                    stride: usize::from(width) * 4,
                    bytes: vnilink_media::Pixels::native(Box::new(picture)),
                    captured_at,
                });
            }
            Err(Error::Unsupported(reason)) => self.channel.fail(reason),
            Err(_) => self
                .channel
                .fail("ScreenCaptureKit did not deliver a frame"),
        }
    }
}

/// Displays ScreenCaptureKit can see. This asks for Screen Recording, the same
/// permission as starting a stream. One invalid display fails the whole list.
/// Width and height are post-rotation pixels. Rotation stays 0 until Core
/// Graphics orientation is already available on this path.
#[cfg(target_os = "macos")]
pub fn list_displays(revision: u64) -> Result<Vec<vnilink::DisplayInfo>, Error> {
    if revision == 0 {
        return Err(Error::InvalidSetting("a display revision is invalid"));
    }
    let content = SCShareableContent::get()
        .map_err(|_| Error::CaptureRejected("Screen Recording permission was not granted"))?;
    let mut displays = Vec::new();
    for display in content.displays() {
        let frame = display.frame();
        let width = u16::try_from(display.width())
            .map_err(|_| Error::InvalidSetting("a display size is out of range"))?;
        let height = u16::try_from(display.height())
            .map_err(|_| Error::InvalidSetting("a display size is out of range"))?;
        let origin_x = display_coord(frame.origin.x)?;
        let origin_y = display_coord(frame.origin.y)?;
        displays.push(vnilink::DisplayInfo {
            id: u64::from(display.display_id()),
            origin_x,
            origin_y,
            width,
            height,
            rotation_deg: 0,
            scale_percent: display_scale(display.width(), frame.size.width),
            revision,
        });
    }
    if displays.is_empty() {
        return Err(Error::CaptureRejected("no display is available"));
    }
    vnilink::validate_topology(&displays)
        .map_err(|_| Error::InvalidSetting("a display size is out of range"))?;
    Ok(displays)
}

#[cfg(target_os = "macos")]
fn display_coord(value: f64) -> Result<i32, Error> {
    if !value.is_finite() {
        return Err(Error::InvalidSetting("a display origin is out of range"));
    }
    let rounded = value.round();
    if rounded < f64::from(i32::MIN) || rounded > f64::from(i32::MAX) {
        return Err(Error::InvalidSetting("a display origin is out of range"));
    }
    Ok(rounded as i32)
}

/// A started ScreenCaptureKit stream. Tests do not construct this.
#[cfg(target_os = "macos")]
pub struct ScreenCapture {
    stream: SCStream,
    plan: ScreenPlan,
    width: u16,
    height: u16,
    channel: Arc<FrameChannel>,
}

#[cfg(target_os = "macos")]
impl ScreenCapture {
    /// List displays and start capture. This can prompt for Screen Recording.
    pub fn start(plan: &ScreenPlan, startup: &vnilink::BackendStartup) -> Result<Self, Error> {
        Self::start_display(plan, None, startup)
    }

    /// Capture the selected main display rather than assuming enumeration order.
    pub fn start_display(
        plan: &ScreenPlan,
        display_id: Option<u32>,
        startup: &vnilink::BackendStartup,
    ) -> Result<Self, Error> {
        let check = || {
            startup
                .check()
                .map_err(|_| Error::CaptureRejected("desktop capture startup cancelled or expired"))
        };
        check()?;
        let content = SCShareableContent::get()
            .map_err(|_| Error::CaptureRejected("Screen Recording permission was not granted"))?;
        check()?;
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
        let channel = Arc::new(FrameChannel::new());
        let delegate = failure_delegate(&channel);
        let mut stream = SCStream::new_with_delegate(&filter, &plan.configuration(), delegate)
            .map_err(|_| Error::CaptureRejected("ScreenCaptureKit did not start"))?;
        stream
            .add_output_handler(
                Handler {
                    channel: Arc::clone(&channel),
                    previous: std::sync::Mutex::new(None),
                },
                SCStreamOutputType::Screen,
            )
            .map_err(|_| Error::CaptureRejected("ScreenCaptureKit did not start"))?;
        check()?;
        stream
            .start_capture()
            .map_err(|_| Error::CaptureRejected("ScreenCaptureKit did not start"))?;
        let capture = Self {
            stream,
            plan: *plan,
            width: plan.width(),
            height: plan.height(),
            channel,
        };
        // Own the stream before checking so late success runs stop_capture.
        check()?;
        Ok(capture)
    }

    /// Switch only the cursor policy at session admission. Legacy viewers keep
    /// ScreenCaptureKit's embedded cursor; metadata-aware viewers draw locally.
    pub fn set_shows_cursor(&self, shows_cursor: bool) -> Result<(), Error> {
        let mut plan = self.plan;
        plan.shows_cursor = shows_cursor;
        self.stream
            .update_configuration(&plan.configuration())
            .map_err(|_| Error::CaptureRejected("the cursor capture policy could not be updated"))
    }

    pub fn size(&self) -> (u16, u16) {
        (self.width, self.height)
    }

    /// When ScreenCaptureKit last delivered a picture whose pixels changed.
    /// Idle polls and frames with the same pixels do not move this.
    pub fn attested_at(&self) -> Option<std::time::Instant> {
        self.channel.pushed_at()
    }

    /// Current pixels without waiting for screen damage. Failure is checked
    /// before cached pixels, so a stopped capture cannot look healthy forever.
    pub fn poll_frame(&self, timestamp_us: u64) -> Result<Option<vnilink_media::RawFrame>, Error> {
        poll_channel(&self.channel, timestamp_us)
    }

    /// Opens the primary display at a fitted thumbnail size. The first polled
    /// frame is a new acquisition; drop the capture to stop the stream.
    pub fn open_snapshot(startup: &vnilink::BackendStartup) -> Result<(Self, String), Error> {
        startup
            .check()
            .map_err(|_| Error::CaptureRejected("desktop capture startup cancelled or expired"))?;
        let content = SCShareableContent::get()
            .map_err(|_| Error::CaptureRejected("Screen Recording permission was not granted"))?;
        startup
            .check()
            .map_err(|_| Error::CaptureRejected("desktop capture startup cancelled or expired"))?;
        // SAFETY: CGMainDisplayID takes no pointer and returns the main display id.
        let primary = unsafe { CGMainDisplayID() };
        let displays = content.displays();
        let display = displays
            .iter()
            .find(|display| display.display_id() == primary)
            .or_else(|| displays.first())
            .ok_or(Error::CaptureRejected("no display is available"))?;
        let (width, height) = vnilink::snapshot::fitted_size(display.width(), display.height())
            .ok_or(Error::CaptureRejected("no display is available"))?;
        let plan = ScreenPlan::snapshot(width, height)?;
        let display_id = display.display_id();
        let capture = Self::start_display(&plan, Some(display_id), startup)?;
        Ok((capture, display_id.to_string()))
    }
}

#[cfg(target_os = "macos")]
#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGMainDisplayID() -> u32;
}

#[cfg(target_os = "macos")]
fn failure_delegate(
    channel: &Arc<FrameChannel>,
) -> screencapturekit::stream::delegate_trait::StreamCallbacks {
    let failed = Arc::clone(channel);
    screencapturekit::stream::delegate_trait::StreamCallbacks::new()
        .on_error(move |_| failed.fail("ScreenCaptureKit stopped capture"))
}

#[cfg(target_os = "macos")]
fn poll_channel(
    channel: &FrameChannel,
    timestamp_us: u64,
) -> Result<Option<vnilink_media::RawFrame>, Error> {
    channel
        .try_latest()?
        .map(|slot| {
            vnilink_media::RawFrame::shared(
                slot.width,
                slot.height,
                slot.stride,
                timestamp_us,
                slot.bytes,
            )
            .map(|frame| frame.with_captured_at(slot.captured_at))
        })
        .transpose()
}

#[cfg(all(test, target_os = "macos"))]
mod idle_tests {
    use super::*;
    #[test]
    fn idle_repeats_the_last_picture_without_waiting_but_failure_does_not() {
        let channel = Arc::new(FrameChannel::new());
        assert!(poll_channel(&channel, 1).unwrap().is_none());
        let shown = std::time::Instant::now();
        channel.push(ChannelSlot {
            width: 2,
            height: 2,
            stride: 8,
            bytes: vec![19; 16].into(),
            captured_at: Some(shown),
        });
        let first = poll_channel(&channel, 2).unwrap().unwrap();
        let repeated = poll_channel(&channel, 3).unwrap().unwrap();
        assert_eq!(first.pixels(), repeated.pixels());
        // A repeat shares the picture the handler copied; it is not copied again.
        assert_eq!(first.pixels().as_ptr(), repeated.pixels().as_ptr());
        assert_eq!(repeated.timestamp_us(), 3);
        // A repeat is the same capture: it keeps when the display showed it.
        assert_eq!(first.captured_at(), Some(shown));
        assert_eq!(repeated.captured_at(), Some(shown));
        use screencapturekit::stream::delegate_trait::SCStreamDelegateTrait;
        failure_delegate(&channel)
            .did_stop_with_error(screencapturekit::error::SCError::internal_error("fixture"));
        assert!(matches!(
            poll_channel(&channel, 4),
            Err(Error::Unsupported("ScreenCaptureKit stopped capture"))
        ));
    }
}

#[cfg(target_os = "macos")]
impl Drop for ScreenCapture {
    fn drop(&mut self) {
        let _ = self.stream.stop_capture();
    }
}
