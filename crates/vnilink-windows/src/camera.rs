//! Locally approved Media Foundation camera capture. OS samples are copied in
//! asynchronous callbacks; no COM sample or borrowed native buffer crosses threads.
use crate::{
    camera_format::{choose_mode, Format},
    WindowsEncoder,
};
use std::sync::{
    mpsc::{self, SyncSender},
    Arc, Mutex,
};
use std::time::{Duration, Instant};
use vnilink::{
    camera::{
        CameraApproval, CameraBackend, CameraConfig, CameraDevice, CameraSource, HostCameraFactory,
    },
    BackendStartup, Error,
};
use vnilink_media::{EncodeParams, RawFrame, VideoEncoder};
use windows::core::Interface;
use windows::core::{implement, Ref, GUID, HRESULT, PWSTR};
use windows::Win32::Media::MediaFoundation::*;
use windows::Win32::System::Com::{
    CoInitializeEx, CoTaskMemFree, CoUninitialize, COINIT_MULTITHREADED,
};

struct Foundation;
impl Foundation {
    fn open() -> Result<Self, Error> {
        unsafe {
            CoInitializeEx(None, COINIT_MULTITHREADED)
                .ok()
                .map_err(|_| Error::Unsupported("camera COM apartment"))?;
            if MFStartup(MF_VERSION, MFSTARTUP_NOSOCKET).is_err() {
                CoUninitialize();
                return Err(Error::Unsupported("Media Foundation camera startup"));
            }
        }
        Ok(Self)
    }
}
impl Drop for Foundation {
    fn drop(&mut self) {
        unsafe {
            let _ = MFShutdown();
            CoUninitialize();
        }
    }
}
fn native<T>(result: windows::core::Result<T>) -> Result<T, Error> {
    result.map_err(|_| Error::Unsupported("Media Foundation camera operation"))
}
unsafe fn attributes(size: u32) -> Result<IMFAttributes, Error> {
    let mut attributes = None;
    native(MFCreateAttributes(&mut attributes, size))?;
    attributes.ok_or(Error::Unsupported("camera attributes"))
}
unsafe fn string(attributes: &IMFAttributes, key: &GUID) -> Result<String, Error> {
    let mut pointer = PWSTR::null();
    let mut length = 0;
    native(attributes.GetAllocatedString(key, &mut pointer, &mut length))?;
    let result = if pointer.is_null() || length > 4096 {
        Err(Error::Unsupported("camera metadata is out of range"))
    } else {
        String::from_utf16(std::slice::from_raw_parts(pointer.0, length as usize))
            .map_err(|_| Error::Unsupported("camera metadata encoding"))
    };
    CoTaskMemFree(Some(pointer.0.cast()));
    result
}
struct Activations {
    pointer: *mut Option<IMFActivate>,
    count: u32,
}
impl Drop for Activations {
    fn drop(&mut self) {
        unsafe {
            if !self.pointer.is_null() {
                for item in std::slice::from_raw_parts_mut(self.pointer, self.count as usize) {
                    drop(item.take());
                }
                CoTaskMemFree(Some(self.pointer.cast()));
            }
        }
    }
}
unsafe fn devices() -> Result<Vec<(CameraDevice, IMFActivate)>, Error> {
    let attrs = attributes(1)?;
    native(attrs.SetGUID(
        &MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE,
        &MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_GUID,
    ))?;
    let mut owned = Activations {
        pointer: std::ptr::null_mut(),
        count: 0,
    };
    native(MFEnumDeviceSources(
        &attrs,
        &mut owned.pointer,
        &mut owned.count,
    ))?;
    if owned.count > 256 || (owned.count != 0 && owned.pointer.is_null()) {
        return Err(Error::Unsupported("too many camera devices"));
    }
    let mut result = Vec::new();
    if owned.count == 0 {
        return Ok(result);
    }
    for item in std::slice::from_raw_parts_mut(owned.pointer, owned.count as usize) {
        let Some(activation) = item.take() else {
            continue;
        };
        let id = string(
            &activation,
            &MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_SYMBOLIC_LINK,
        )?;
        if id.is_empty() || id.len() > 256 {
            continue;
        }
        let name = string(&activation, &MF_DEVSOURCE_ATTRIBUTE_FRIENDLY_NAME)?
            .chars()
            .take(256)
            .collect();
        result.push((CameraDevice { id, name }, activation));
    }
    Ok(result)
}
pub fn camera_devices() -> Result<Vec<CameraDevice>, Error> {
    // Native UI callers may use an STA. Enumeration owns a dedicated MTA
    // instead of changing or leaking the caller's COM apartment.
    std::thread::Builder::new()
        .name("vnilink-camera-enumerate".into())
        .spawn(|| {
            let _foundation = Foundation::open()?;
            unsafe { devices().map(|devices| devices.into_iter().map(|(info, _)| info).collect()) }
        })
        .map_err(|_| Error::Unsupported("camera enumeration worker"))?
        .join()
        .map_err(|_| Error::Unsupported("camera enumeration failed"))?
}
pub struct WindowsCameraFactory {
    approved: CameraApproval,
}
impl WindowsCameraFactory {
    pub fn new(approved: CameraApproval) -> Result<Self, Error> {
        approved.validate()?;
        Ok(Self { approved })
    }
}
impl HostCameraFactory for WindowsCameraFactory {
    fn devices(&self) -> Result<Vec<CameraDevice>, Error> {
        Ok(camera_devices()?
            .into_iter()
            .filter(|device| self.approved.allows(&device.id))
            .collect())
    }
    fn open(&self, config: CameraConfig, startup: BackendStartup) -> Result<CameraBackend, Error> {
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
            .name("vnilink-camera-mf".into())
            .spawn(move || {
                if run(config, source.clone(), &ready, &worker_startup).is_err() {
                    let _ =
                        ready.try_send(Err(Error::Unsupported("Windows camera capture failed")));
                    source.fail();
                }
                source.close();
            })
            .map_err(|_| Error::Unsupported("camera worker"))?;
        backend.config = startup.receive(&result)??;
        Ok(backend)
    }
}
#[implement(IMFSourceReaderCallback)]
struct Callback {
    format: Arc<Mutex<Option<Format>>>,
    samples: SyncSender<Result<Option<RawFrame>, Error>>,
    source: Arc<CameraSource>,
    origin: Instant,
}
impl IMFSourceReaderCallback_Impl for Callback_Impl {
    fn OnReadSample(
        &self,
        status: HRESULT,
        _: u32,
        flags: u32,
        _: i64,
        sample: Ref<'_, IMFSample>,
    ) -> windows::core::Result<()> {
        if self.source.is_cancelled() {
            return Ok(());
        }
        let result = if status.is_err()
            || flags
                & (MF_SOURCE_READERF_ERROR.0
                    | MF_SOURCE_READERF_ENDOFSTREAM.0
                    | MF_SOURCE_READERF_CURRENTMEDIATYPECHANGED.0
                    | MF_SOURCE_READERF_NATIVEMEDIATYPECHANGED.0) as u32
                != 0
        {
            Err(Error::Unsupported("camera device ended or changed format"))
        } else if let Some(sample) = sample.as_ref() {
            unsafe {
                copy_sample(
                    sample,
                    *self.format.lock().unwrap_or_else(|p| p.into_inner()),
                    self.origin.elapsed().as_micros().min(u64::MAX as u128) as u64,
                )
                .map(Some)
            }
        } else {
            Ok(None)
        };
        if self.samples.try_send(result).is_err() {
            self.source.fail();
        }
        Ok(())
    }
    fn OnFlush(&self, _: u32) -> windows::core::Result<()> {
        Ok(())
    }
    fn OnEvent(&self, _: u32, event: Ref<'_, IMFMediaEvent>) -> windows::core::Result<()> {
        if let Some(event) = event.as_ref() {
            if unsafe { event.GetStatus() }.map_or(true, |status| status.is_err()) {
                self.source.fail();
            }
        }
        Ok(())
    }
}
unsafe fn copy_sample(
    sample: &IMFSample,
    format: Option<Format>,
    timestamp: u64,
) -> Result<RawFrame, Error> {
    let format = format.ok_or(Error::Unsupported("camera format is not ready"))?;
    // A 2D buffer says where its top row is and which way rows run; the
    // media type's stride is only a fallback, and its default for RGB
    // (bottom-up) turned pictures upside down.
    if native(sample.GetBufferCount())? == 1 {
        if let Ok(buffer) = native(sample.GetBufferByIndex(0))?.cast::<IMF2DBuffer2>() {
            let mut scan0 = std::ptr::null_mut();
            let mut pitch = 0;
            let mut start = std::ptr::null_mut();
            let mut length = 0;
            native(buffer.Lock2DSize(
                MF2DBuffer_LockFlags_Read,
                &mut scan0,
                &mut pitch,
                &mut start,
                &mut length,
            ))?;
            let copied = if start.is_null() || scan0 < start || length > 64 * 1024 * 1024 {
                Err(Error::Unsupported("camera buffer size"))
            } else {
                format
                    .copy_pitched(
                        std::slice::from_raw_parts(start, length as usize),
                        scan0.offset_from(start) as usize,
                        pitch as isize,
                        timestamp,
                    )
                    .map_err(|_| Error::Unsupported("camera RGB32 sample"))
            };
            let unlocked = native(buffer.Unlock2D());
            unlocked?;
            return copied;
        }
    }
    let buffer = native(sample.ConvertToContiguousBuffer())?;
    let mut pointer = std::ptr::null_mut();
    let mut length = 0;
    native(buffer.Lock(&mut pointer, None, Some(&mut length)))?;
    let copied = if pointer.is_null() || length > 64 * 1024 * 1024 {
        Err(Error::Unsupported("camera buffer size"))
    } else {
        format
            .copy(
                std::slice::from_raw_parts(pointer, length as usize),
                timestamp,
            )
            .map_err(|_| Error::Unsupported("camera RGB32 sample"))
    };
    let unlocked = native(buffer.Unlock());
    unlocked?;
    copied
}
struct OwnedSource(IMFMediaSource, IMFActivate);
impl Drop for OwnedSource {
    fn drop(&mut self) {
        unsafe {
            let _ = self.0.Shutdown();
            let _ = self.1.ShutdownObject();
        }
    }
}
struct Capture {
    reader: IMFSourceReader,
    source: Option<OwnedSource>,
}
impl Drop for Capture {
    fn drop(&mut self) {
        unsafe {
            let _ = self
                .reader
                .Flush(MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32);
            drop(self.source.take());
        }
    }
}
fn run(
    mut config: CameraConfig,
    source: Arc<CameraSource>,
    ready: &SyncSender<Result<CameraConfig, Error>>,
    startup: &BackendStartup,
) -> Result<(), Error> {
    let _foundation = Foundation::open()?;
    startup.check()?;
    let (_, activation) = unsafe { devices()? }
        .into_iter()
        .find(|(device, _)| device.id == config.device_id)
        .ok_or(Error::Unsupported("approved camera is unavailable"))?;
    startup.check()?;
    let media_source: IMFMediaSource = unsafe { native(activation.ActivateObject())? };
    // Own activation immediately, including failures before reader creation.
    let partial = OwnedSource(media_source, activation);
    startup.check()?;
    let (samples, receiver) = mpsc::sync_channel(2);
    let format = Arc::new(Mutex::new(None));
    let callback: IMFSourceReaderCallback = Callback {
        format: format.clone(),
        samples,
        source: source.clone(),
        origin: Instant::now(),
    }
    .into();
    let reader = unsafe {
        let attrs = attributes(3)?;
        native(attrs.SetUINT32(&MF_SOURCE_READER_DISCONNECT_MEDIASOURCE_ON_SHUTDOWN, 1))?;
        native(attrs.SetUnknown(&MF_SOURCE_READER_ASYNC_CALLBACK, &callback))?;
        native(attrs.SetUINT32(&MF_SOURCE_READER_ENABLE_ADVANCED_VIDEO_PROCESSING, 1))?;
        native(MFCreateSourceReaderFromMediaSource(&partial.0, &attrs))?
    };
    let capture = Capture {
        reader,
        source: Some(partial),
    };
    startup.check()?;
    unsafe {
        // Open one of the camera's own modes in its sensor's shape and ask
        // for that shape: other sizes come scaled and letterboxed.
        let stream = MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32;
        let mut formats = Vec::new();
        for index in 0..256 {
            let Ok(native_type) = capture.reader.GetNativeMediaType(stream, index) else {
                break;
            };
            // One entry per native index, so the choice names the mode to open.
            let (Ok(size), Ok(rate)) = (
                native_type.GetUINT64(&MF_MT_FRAME_SIZE),
                native_type.GetUINT64(&MF_MT_FRAME_RATE),
            ) else {
                formats.push((0, 0, 0));
                continue;
            };
            let denominator = u64::from(rate as u32).max(1);
            let fps = ((rate >> 32) + denominator / 2) / denominator;
            formats.push((
                (size >> 32) as u32,
                size as u32,
                if rate as u32 == 0 {
                    0
                } else {
                    u32::try_from(fps).unwrap_or(0)
                },
            ));
        }
        if let Some(mode) = choose_mode(&formats, config.width, config.fps) {
            let native_type = native(
                capture
                    .reader
                    .GetNativeMediaType(stream, mode.native as u32),
            )?;
            native(
                capture
                    .reader
                    .SetCurrentMediaType(stream, None, &native_type),
            )?;
            config.width = mode.width;
            config.height = mode.height;
            config.fps = mode.fps;
        }
        let mt = native(MFCreateMediaType())?;
        native(mt.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video))?;
        native(mt.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_RGB32))?;
        native(mt.SetUINT64(
            &MF_MT_FRAME_SIZE,
            (u64::from(config.width) << 32) | u64::from(config.height),
        ))?;
        native(mt.SetUINT64(&MF_MT_FRAME_RATE, (u64::from(config.fps) << 32) | 1))?;
        native(capture.reader.SetCurrentMediaType(
            MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32,
            None,
            &mt,
        ))?;
        let actual = native(
            capture
                .reader
                .GetCurrentMediaType(MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32),
        )?;
        if native(actual.GetGUID(&MF_MT_SUBTYPE))? != MFVideoFormat_RGB32 {
            return Err(Error::Unsupported("camera RGB32 conversion"));
        }
        let size = native(actual.GetUINT64(&MF_MT_FRAME_SIZE))?;
        config.width = u16::try_from(size >> 32).map_err(|_| Error::Unsupported("camera width"))?;
        config.height =
            u16::try_from(size as u32).map_err(|_| Error::Unsupported("camera height"))?;
        let rate = native(actual.GetUINT64(&MF_MT_FRAME_RATE))?;
        let denominator = rate as u32;
        if denominator == 0 {
            return Err(Error::Unsupported("camera frame rate"));
        }
        config.fps =
            u16::try_from(((rate >> 32) + u64::from(denominator) / 2) / u64::from(denominator))
                .map_err(|_| Error::Unsupported("camera frame rate"))?;
        config.validate()?;
        let stride = match actual.GetUINT32(&MF_MT_DEFAULT_STRIDE) {
            Ok(stride) => stride as i32,
            Err(_) => native(MFGetStrideForBitmapInfoHeader(
                MFVideoFormat_RGB32.data1,
                u32::from(config.width),
            ))?,
        };
        *format.lock().unwrap() = Some(Format {
            width: config.width,
            height: config.height,
            stride,
        });
    }
    startup.check()?;
    // This worker owns COM and MF for the encoder's entire lifetime. Avoid
    // acquiring an additional apartment/startup reference for each camera.
    let mut encoder = unsafe {
        WindowsEncoder::open_camera(config.width, config.height, config.fps as u8, 4_000_000)
    }
    .map_err(|_| Error::Unsupported("Windows camera encoder"))?;
    let mut announced = false;
    let mut last_frame = Instant::now();
    loop {
        if source.is_cancelled() {
            return Ok(());
        }
        if !announced {
            startup.check()?;
        }
        unsafe {
            native(capture.reader.ReadSample(
                MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32,
                0,
                None,
                None,
                None,
                None,
            ))?;
        }
        let sample = loop {
            if source.is_cancelled() {
                return Ok(());
            }
            if !announced {
                startup.check()?;
            }
            if last_frame.elapsed() >= Duration::from_secs(5) {
                return Err(Error::Timeout);
            }
            match receiver.recv_timeout(Duration::from_millis(10)) {
                Ok(result) => break result?,
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(_) => return Err(Error::Unsupported("camera callback ended")),
            }
        };
        let Some(frame) = sample else {
            continue;
        };
        last_frame = Instant::now();
        let keyframe = !announced || source.take_keyframe_request();
        let encoded = encoder
            .encode(
                &frame,
                &EncodeParams {
                    bitrate_bps: 4_000_000,
                    frame_rate: config.fps as u8,
                    force_keyframe: keyframe,
                },
            )
            .map_err(|_| Error::Unsupported("Windows camera encoding"))?;
        if !announced && !encoded.keyframe() {
            continue;
        }
        if encoded.keyframe() {
            crate::bitstream::camera_access_unit(encoded.bytes())
                .map_err(|_| Error::Unsupported("camera H.264 constrained baseline"))?;
        }
        source.publish(vnilink::EncodedFrame::h264(
            encoded.keyframe(),
            frame.timestamp_us(),
            encoded.bytes().to_vec(),
        )?)?;
        if !announced {
            startup.check()?;
            ready
                .try_send(Ok(config.clone()))
                .map_err(|_| Error::NoSession)?;
            announced = true;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_callback_copies_owned_rows_and_reports_device_end_without_a_camera() {
        let _foundation = Foundation::open().unwrap();
        let source = CameraSource::new();
        let (samples, receiver) = mpsc::sync_channel(2);
        let callback: IMFSourceReaderCallback = Callback {
            format: Arc::new(Mutex::new(Some(Format {
                width: 2,
                height: 2,
                stride: 8,
            }))),
            samples,
            source: source.clone(),
            origin: Instant::now(),
        }
        .into();
        unsafe {
            let buffer = MFCreateMemoryBuffer(16).unwrap();
            let mut pointer = std::ptr::null_mut();
            buffer.Lock(&mut pointer, None, None).unwrap();
            std::ptr::copy_nonoverlapping([1u8; 16].as_ptr(), pointer, 16);
            buffer.Unlock().unwrap();
            buffer.SetCurrentLength(16).unwrap();
            let sample = MFCreateSample().unwrap();
            sample.AddBuffer(&buffer).unwrap();
            callback.OnReadSample(HRESULT(0), 0, 0, 0, &sample).unwrap();
            drop(sample);
            drop(buffer);
            assert_eq!(
                receiver.recv().unwrap().unwrap().unwrap().pixels(),
                &[1; 16]
            );
            callback
                .OnReadSample(
                    HRESULT(0),
                    0,
                    MF_SOURCE_READERF_ENDOFSTREAM.0 as u32,
                    0,
                    None,
                )
                .unwrap();
            assert!(receiver.recv().unwrap().is_err());
            source.close();
            callback.OnReadSample(HRESULT(0), 0, 0, 0, None).unwrap();
            assert!(receiver.try_recv().is_err());
        }
    }

    #[test]
    #[ignore = "requires explicit VNilink test camera ID and Windows camera consent"]
    fn approved_physical_camera_starts_after_first_keyframe_and_closes() {
        let id = std::env::var("VNILINK_TEST_CAMERA_ID")
            .expect("select an enumerated camera explicitly; no automatic camera activation");
        let factory = WindowsCameraFactory::new(CameraApproval::Only(vec![id.clone()])).unwrap();
        assert!(factory
            .devices()
            .unwrap()
            .iter()
            .any(|device| device.id == id));
        let backend = factory
            .open(
                CameraConfig {
                    device_id: id,
                    width: 640,
                    height: 480,
                    fps: 30,
                },
                BackendStartup::new(Duration::from_secs(15)),
            )
            .unwrap();
        assert!(!backend.source.is_cancelled());
        backend.source.close();
        assert!(backend.source.is_cancelled());
        drop(backend);
    }
}
