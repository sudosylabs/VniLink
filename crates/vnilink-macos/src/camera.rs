//! AVFoundation camera capture into a bounded callback queue and VideoToolbox.
use crate::MacEncoder;
use dispatch2::DispatchQueue;
use objc2::{
    define_class, msg_send,
    rc::{autoreleasepool, Retained},
    runtime::{AnyObject, ProtocolObject},
    AnyThread, DeclaredClass,
};
use objc2_av_foundation::*;
use objc2_core_media::{CMSampleBuffer, CMTime, CMVideoFormatDescriptionGetDimensions};
use objc2_core_video::*;
use objc2_foundation::{NSArray, NSDictionary, NSNumber, NSObject, NSObjectProtocol, NSString};
use std::{
    sync::{
        mpsc::{self, SyncSender},
        Arc,
    },
    time::{Duration, Instant},
};
use vnilink::YuvMatrix;
use vnilink::{
    camera::{
        CameraApproval, CameraBackend, CameraConfig, CameraDevice, CameraSource, HostCameraFactory,
    },
    Error,
};
use vnilink_media::{EncodeParams, RawFrame, VideoEncoder};
const IDLE: Duration = Duration::from_secs(5);

/// Non-prompting check. Only a local native permission flow may request access.
pub fn microphone_authorized() -> bool {
    unsafe {
        AVMediaTypeAudio.is_some_and(|kind| {
            AVCaptureDevice::authorizationStatusForMediaType(kind)
                == AVAuthorizationStatus::Authorized
        })
    }
}
const BITRATE: u32 = 4_000_000;

fn discover() -> Result<Retained<NSArray<AVCaptureDevice>>, Error> {
    unsafe {
        let media = AVMediaTypeVideo.ok_or(Error::Unsupported("AVFoundation video"))?;
        let types = NSArray::from_slice(&[
            AVCaptureDeviceTypeBuiltInWideAngleCamera,
            AVCaptureDeviceTypeExternal,
            AVCaptureDeviceTypeContinuityCamera,
        ]);
        Ok(
            AVCaptureDeviceDiscoverySession::discoverySessionWithDeviceTypes_mediaType_position(
                &types,
                Some(media),
                AVCaptureDevicePosition::Unspecified,
            )
            .devices(),
        )
    }
}
pub fn camera_devices() -> Result<Vec<CameraDevice>, Error> {
    autoreleasepool(|_| unsafe {
        let devices = discover()?;
        Ok(devices
            .iter()
            .map(|d| CameraDevice {
                id: d.uniqueID().to_string(),
                name: d.localizedName().to_string(),
            })
            .collect())
    })
}
pub struct MacCameraFactory {
    approved: CameraApproval,
}
impl MacCameraFactory {
    pub fn new(approved: CameraApproval) -> Result<Self, Error> {
        approved.validate()?;
        Ok(Self { approved })
    }
}
impl HostCameraFactory for MacCameraFactory {
    fn devices(&self) -> Result<Vec<CameraDevice>, Error> {
        Ok(camera_devices()?
            .into_iter()
            .filter(|d| self.approved.allows(&d.id))
            .collect())
    }
    fn open(
        &self,
        config: CameraConfig,
        startup: vnilink::BackendStartup,
    ) -> Result<CameraBackend, Error> {
        startup.check()?;
        config.validate()?;
        if !self.approved.allows(&config.device_id) {
            return Err(Error::Unauthorized);
        }
        let source = CameraSource::new();
        let mut backend = CameraBackend::new(config.clone(), source.clone())?;
        let (ready, result) = mpsc::sync_channel(1);
        let worker_startup = startup.clone();
        std::thread::Builder::new()
            .name("vnilink-camera-avf".into())
            .spawn(move || {
                let run =
                    autoreleasepool(|_| capture(config, source.clone(), &ready, &worker_startup));
                if let Err(error) = run {
                    // The cause, such as missing permission or no usable format, reaches availability.
                    let _ = ready.try_send(Err(error));
                    source.fail();
                }
                source.close();
            })
            .map_err(|_| Error::Unsupported("camera worker"))?;
        backend.config = startup.receive(&result)??;
        Ok(backend)
    }
}
struct CallbackState {
    frames: SyncSender<RawFrame>,
    source: Arc<CameraSource>,
    start: Instant,
}
define_class!(
    // NSObject permits subclassing; ivars are thread-safe and no Drop is implemented.
    #[unsafe(super = NSObject)]
    #[thread_kind = AnyThread]
    #[name = "VniLinkCameraOutput"]
    #[ivars = CallbackState]
    struct CameraOutput;
    unsafe impl NSObjectProtocol for CameraOutput {}
    unsafe impl AVCaptureVideoDataOutputSampleBufferDelegate for CameraOutput {
        #[unsafe(method(captureOutput:didOutputSampleBuffer:fromConnection:))]
        fn output(
            &self,
            _output: &AVCaptureOutput,
            sample: &CMSampleBuffer,
            _connection: &AVCaptureConnection,
        ) {
            let state = self.ivars();
            if state.source.is_cancelled() {
                return;
            }
            // The borrowed sample remains alive for this callback. Copy pixels
            // while locked; nothing referencing the OS buffer crosses threads.
            match unsafe {
                copy_sample(
                    sample,
                    state.start.elapsed().as_micros().min(u64::MAX as u128) as u64,
                )
            } {
                Ok(frame) => {
                    let _ = state.frames.try_send(frame);
                }
                Err(_) => state.source.fail(),
            }
        }
    }
);
unsafe fn copy_sample(sample: &CMSampleBuffer, time: u64) -> Result<RawFrame, Error> {
    let buffer =
        unsafe { sample.image_buffer() }.ok_or(Error::Unsupported("camera pixel buffer"))?;
    unsafe { copy_pixel_buffer(&buffer, time) }
}
unsafe fn copy_pixel_buffer(buffer: &CVPixelBuffer, time: u64) -> Result<RawFrame, Error> {
    if CVPixelBufferGetPixelFormatType(buffer) != kCVPixelFormatType_32BGRA {
        return Err(Error::Unsupported("camera pixel format"));
    }
    let width = u16::try_from(CVPixelBufferGetWidth(buffer)).map_err(|_| Error::Malformed)?;
    let height = u16::try_from(CVPixelBufferGetHeight(buffer)).map_err(|_| Error::Malformed)?;
    if width == 0 || height == 0 || width > 4096 || height > 4096 {
        return Err(Error::Malformed);
    }
    let flags = CVPixelBufferLockFlags::ReadOnly;
    if unsafe { CVPixelBufferLockBaseAddress(buffer, flags) } != 0 {
        return Err(Error::Unsupported("camera buffer lock"));
    }
    struct Unlock<'a>(&'a CVPixelBuffer);
    impl Drop for Unlock<'_> {
        fn drop(&mut self) {
            unsafe {
                CVPixelBufferUnlockBaseAddress(self.0, CVPixelBufferLockFlags::ReadOnly);
            }
        }
    }
    let _unlock = Unlock(buffer);
    let stride = CVPixelBufferGetBytesPerRow(buffer);
    let needed = stride
        .checked_mul(usize::from(height))
        .filter(|n| *n <= 128 * 1024 * 1024)
        .ok_or(Error::Malformed)?;
    let ptr = CVPixelBufferGetBaseAddress(buffer);
    if ptr.is_null() || stride < usize::from(width) * 4 || CVPixelBufferGetDataSize(buffer) < needed
    {
        return Err(Error::Malformed);
    }
    let pixels = unsafe { std::slice::from_raw_parts(ptr.cast::<u8>(), needed) }.to_vec();
    RawFrame::with_stride(width, height, stride, time, pixels).map_err(|_| Error::Malformed)
}
struct CaptureGuard {
    session: Retained<AVCaptureSession>,
    output: Retained<AVCaptureVideoDataOutput>,
}
impl Drop for CaptureGuard {
    fn drop(&mut self) {
        // A session left mid-configuration by a refused setting may throw
        // again here; the engine outlives a camera that could not stop.
        let _ = objc2::exception::catch(std::panic::AssertUnwindSafe(|| unsafe {
            self.session.stopRunning();
            self.output.setSampleBufferDelegate_queue(None, None);
        }));
    }
}
/// The whole frame rate a range offers nearest `requested`. Ranges are
/// reported in floating point (30 fps as 30.00003), so the bounds are
/// rounded with a small tolerance before a rate is taken from them.
fn frame_rate_in(min: f64, max: f64, requested: u16) -> Option<u16> {
    let min = (min - 0.01).ceil().max(1.0);
    let max = (max + 0.01).floor().min(60.0);
    (min.is_finite() && max.is_finite() && min <= max)
        .then(|| f64::from(requested).clamp(min, max) as u16)
}
/// Makes `format` the device's format at a steady frame `duration`, one of
/// the format's own ranges.
unsafe fn set_format(
    device: &AVCaptureDevice,
    format: &AVCaptureDeviceFormat,
    duration: CMTime,
) -> bool {
    if unsafe { device.lockForConfiguration() }.is_err() {
        return false;
    }
    unsafe {
        device.setActiveFormat(format);
        device.setActiveVideoMinFrameDuration(duration);
        device.setActiveVideoMaxFrameDuration(duration);
        device.unlockForConfiguration();
    }
    true
}
fn capture(
    mut config: CameraConfig,
    source: Arc<CameraSource>,
    ready: &SyncSender<Result<CameraConfig, Error>>,
    startup: &vnilink::BackendStartup,
) -> Result<(), Error> {
    startup.check()?;
    // Never request consent implicitly from a remote peer. Native permission UI
    // must obtain consent before this factory is enabled.
    unsafe {
        if AVCaptureDevice::authorizationStatusForMediaType(
            AVMediaTypeVideo.ok_or(Error::Unsupported("AVFoundation video"))?,
        ) != AVAuthorizationStatus::Authorized
        {
            return Err(Error::Unsupported("macOS camera permission"));
        }
    }
    let device = discover()?
        .iter()
        .find(|d| unsafe { d.uniqueID().to_string() == config.device_id })
        .ok_or(Error::Unsupported("camera device"))?;
    let mut choices = Vec::new();
    for format in unsafe { device.formats() }.iter() {
        let dimensions =
            unsafe { CMVideoFormatDescriptionGetDimensions(&format.formatDescription()) };
        let Ok(width) = u16::try_from(dimensions.width) else {
            continue;
        };
        let Ok(height) = u16::try_from(dimensions.height) else {
            continue;
        };
        if width == 0
            || height == 0
            || width > 4096
            || height > 4096
            || width % 2 != 0
            || height % 2 != 0
        {
            continue;
        }
        for range in unsafe { format.videoSupportedFrameRateRanges() }.iter() {
            let Some(fps) = frame_rate_in(
                unsafe { range.minFrameRate() },
                unsafe { range.maxFrameRate() },
                config.fps,
            ) else {
                continue;
            };
            let score = u32::from(width.abs_diff(config.width))
                + u32::from(height.abs_diff(config.height))
                + u32::from(fps.abs_diff(config.fps)) * 8;
            // The device accepts only durations inside the range it reports,
            // and throws for any other: at a bound, use the bound itself.
            let duration = unsafe {
                if f64::from(fps) >= range.maxFrameRate() - 0.01 {
                    range.minFrameDuration()
                } else if f64::from(fps) <= range.minFrameRate() + 0.01 {
                    range.maxFrameDuration()
                } else {
                    CMTime::new(1, i32::from(fps))
                }
            };
            choices.push((score, format.clone(), width, height, fps, duration));
        }
    }
    let (_, format, width, height, fps, duration) = choices
        .into_iter()
        .min_by_key(|choice| choice.0)
        .ok_or(Error::Unsupported("camera resolution or frame rate"))?;
    config.width = width;
    config.height = height;
    config.fps = fps;
    let mut encoder = MacEncoder::open(
        config.width,
        config.height,
        config.fps as u8,
        BITRATE,
        false,
        YuvMatrix::Bt709,
    )
    .map_err(|_| Error::Unsupported("camera H.264 encoder"))?;
    let (frames, receiver) = mpsc::sync_channel(2);
    let allocated = CameraOutput::alloc().set_ivars(CallbackState {
        frames,
        source: source.clone(),
        start: Instant::now(),
    });
    let delegate: Retained<CameraOutput> = unsafe { msg_send![super(allocated), init] };
    let queue = DispatchQueue::new("vnilink.camera.capture", None);
    let session = unsafe { AVCaptureSession::new() };
    let output = unsafe { AVCaptureVideoDataOutput::new() };
    let guard = CaptureGuard { session, output };
    // AVFoundation throws for a setting a device refuses; that must fail
    // this camera, not unwind through Rust and abort the engine.
    let configured =
        objc2::exception::catch(std::panic::AssertUnwindSafe(|| -> Result<bool, Error> {
            unsafe {
                let input = AVCaptureDeviceInput::deviceInputWithDevice_error(&device)
                    .map_err(|_| Error::Unsupported("camera input"))?;
                guard.session.beginConfiguration();
                if !guard.session.canAddInput(&input) || !guard.session.canAddOutput(&guard.output)
                {
                    guard.session.commitConfiguration();
                    return Err(Error::Unsupported("camera session"));
                }
                guard.session.addInput(&input);
                guard.session.addOutput(&guard.output);
                // Setting the format makes the session's preset follow it
                // (macOS refuses an explicit InputPriority preset); the
                // format is applied again after start, which can reset it.
                if !set_format(&device, &format, duration) {
                    guard.session.commitConfiguration();
                    return Err(Error::Unsupported("camera configuration lock"));
                }
                let settings: Retained<NSDictionary<NSString, AnyObject>> =
                    NSDictionary::from_slices(
                        &[&*NSString::from_str("PixelFormatType")],
                        &[&*NSNumber::new_u32(kCVPixelFormatType_32BGRA) as &AnyObject],
                    );
                guard.output.setVideoSettings(Some(&settings));
                guard.output.setAlwaysDiscardsLateVideoFrames(true);
                guard.output.setSampleBufferDelegate_queue(
                    Some(ProtocolObject::from_ref(&*delegate)),
                    Some(&queue),
                );
                guard.session.commitConfiguration();
                if source.is_cancelled() {
                    return Ok(false);
                }
                startup.check()?;
                guard.session.startRunning();
                startup.check()?;
                if !guard.session.isRunning() {
                    return Err(Error::Unsupported("camera startup"));
                }
                // Starting may still reset the device; apply the format again if so.
                if device.activeFormat() != format && !set_format(&device, &format, duration) {
                    return Err(Error::Unsupported("camera configuration lock"));
                }
            }
            Ok(true)
        }));
    match configured {
        Ok(Ok(true)) => {}
        Ok(Ok(false)) => return Ok(()),
        Ok(Err(error)) => return Err(error),
        Err(_) => return Err(Error::Unsupported("camera refused its configuration")),
    }
    let mut reported = false;
    let mut last = Instant::now();
    while !source.is_cancelled() {
        if !reported {
            startup.check()?;
        }
        match receiver.recv_timeout(Duration::from_millis(25)) {
            Ok(frame) => {
                last = Instant::now();
                // The camera's buffers decide the picture size: a device may
                // deliver another size than the format chosen above. The
                // stream reports its size with the first picture, so only a
                // change after that is refused.
                if (frame.width(), frame.height()) != (config.width, config.height) {
                    if reported || frame.width() % 2 != 0 || frame.height() % 2 != 0 {
                        return Err(Error::Unsupported("camera picture size changed"));
                    }
                    config.width = frame.width();
                    config.height = frame.height();
                    config.validate()?;
                    encoder = MacEncoder::open(
                        config.width,
                        config.height,
                        config.fps as u8,
                        BITRATE,
                        false,
                        YuvMatrix::Bt709,
                    )
                    .map_err(|_| Error::Unsupported("camera H.264 encoder"))?;
                }
                let sample = encoder
                    .encode(
                        &frame,
                        &EncodeParams {
                            force_keyframe: source.take_keyframe_request(),
                            recovery: false,
                            bitrate_bps: BITRATE,
                            frame_rate: config.fps as u8,
                        },
                    )
                    .map_err(|_| Error::Unsupported("camera encoding"))?;
                source.publish(vnilink::EncodedFrame::h264(
                    sample.keyframe(),
                    frame.timestamp_us(),
                    sample.bytes().to_vec(),
                )?)?;
                if !reported {
                    ready
                        .try_send(Ok(config.clone()))
                        .map_err(|_| Error::NoSession)?;
                    reported = true;
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) if last.elapsed() < IDLE => {}
            Err(_) => return Err(Error::Unsupported("camera stopped delivering frames")),
        }
    }
    // Stop capture before releasing the delegate and callback queue.
    drop(guard);
    drop(delegate);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn frame_rates_reported_in_floating_point_keep_their_whole_rate() {
        // A Logitech C270 reports its fixed 30 fps as 30.00003 and 7.5 fps exactly.
        assert_eq!(frame_rate_in(30.00003000003, 30.00003000003, 30), Some(30));
        assert_eq!(
            frame_rate_in(15.000015000015, 15.000015000015, 30),
            Some(15)
        );
        assert_eq!(frame_rate_in(1.0, 60.0, 30), Some(30));
        assert_eq!(
            frame_rate_in(7.500001875000469, 7.500001875000469, 30),
            None,
            "no whole rate"
        );
        assert_eq!(frame_rate_in(90.0, 120.0, 30), None);
    }
    #[test]
    fn native_camera_pixel_copy_and_baseline_encode_without_capture() {
        use objc2_core_foundation::CFRetained;
        use std::ptr::NonNull;
        let mut raw = std::ptr::null_mut();
        // Core Video allocates the synthetic buffer. No capture device exists.
        assert_eq!(
            unsafe {
                CVPixelBufferCreate(
                    None,
                    64,
                    64,
                    kCVPixelFormatType_32BGRA,
                    None,
                    NonNull::from(&mut raw),
                )
            },
            0
        );
        let buffer = unsafe { CFRetained::from_raw(NonNull::new(raw).unwrap()) };
        assert_eq!(
            unsafe { CVPixelBufferLockBaseAddress(&buffer, CVPixelBufferLockFlags::empty()) },
            0
        );
        let stride = CVPixelBufferGetBytesPerRow(&buffer);
        let data = unsafe {
            std::slice::from_raw_parts_mut(
                CVPixelBufferGetBaseAddress(&buffer).cast::<u8>(),
                stride * 64,
            )
        };
        for row in data.chunks_exact_mut(stride) {
            for pixel in row[..256].chunks_exact_mut(4) {
                pixel.copy_from_slice(&[20, 80, 180, 255]);
            }
        }
        assert_eq!(
            unsafe { CVPixelBufferUnlockBaseAddress(&buffer, CVPixelBufferLockFlags::empty()) },
            0
        );
        let frame = unsafe { copy_pixel_buffer(&buffer, 123) }.unwrap();
        assert_eq!(
            (frame.width(), frame.height(), frame.timestamp_us()),
            (64, 64, 123)
        );
        for y in 0..64 {
            assert_eq!(frame.row(y), [20, 80, 180, 255].repeat(64));
        }
        let mut encoder = MacEncoder::open(64, 64, 30, BITRATE, false, YuvMatrix::Bt709).unwrap();
        let sample = encoder
            .encode(
                &frame,
                &EncodeParams {
                    force_keyframe: true,
                    recovery: false,
                    bitrate_bps: BITRATE,
                    frame_rate: 30,
                },
            )
            .unwrap();
        let parsed = vnilink_media::parse_annex_b(sample.bytes()).unwrap();
        assert!(sample.keyframe() && parsed.has_sps() && parsed.has_pps() && parsed.has_idr());
        // The H.264 profile byte follows the SPS NAL header.
        let sps = parsed.units().iter().find(|n| n.nal_type == 7).unwrap();
        assert_eq!(sps.bytes()[1], 66);
    }
    #[test]
    fn native_camera_factory_refuses_unapproved_device_without_capture() {
        let factory = MacCameraFactory::new(CameraApproval::Only(vec![])).unwrap();
        assert!(factory.devices().unwrap().is_empty());
        assert!(matches!(
            factory.open(
                CameraConfig {
                    device_id: "unapproved-camera".into(),
                    width: 640,
                    height: 480,
                    fps: 30
                },
                vnilink::BackendStartup::new(Duration::from_secs(5))
            ),
            Err(Error::Unauthorized)
        ));
    }
}
