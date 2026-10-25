//! Media Foundation H.264. Input is NV12. Output is Annex-B.
//!
//! The desktop encoder is the first H.264 transform that opens, in the
//! encoder rule's order ([`crate::transform`]): a D3D11-aware hardware
//! transform on the capture's adapter reading the converted texture (zero
//! copy), else a hardware transform reading NV12 from memory, the capture
//! adapter's first, else Microsoft's software transform. What the encoder
//! reads a picture from is reported with it ([`vnilink::EncoderInput`]).
//! The camera uses Microsoft's software transform. Hardware transforms are asynchronous: input and output wait
//! for their events ([`TransformEvents`]).
//!
//! The Microsoft transform writes 4-byte length prefixes. SPS and PPS are
//! read from the output media type and prefixed onto an IDR. A sample is a
//! keyframe only when those three NAL units are present. Forcing the next
//! picture to an IDR goes through `ICodecAPI` (`AVEncVideoForceKeyframe`)
//! before the input sample is processed. The pump rejects a forced frame
//! that is not a keyframe. Without Media Foundation H.264 this returns
//! unsupported.
//!
//! A recovery keyframe is encoded under a quantiser floor
//! (`AVEncVideoMinQP`) that [`RecoveryQp`] moves until it fits a few
//! pictures' bytes, and the floor is cleared once its picture is drained;
//! the transform is drained after each input, so no other picture is
//! encoded under the floor. Only transforms not measured to ignore the
//! floor get one ([`Vendor::takes_qp_floor`]). On those measured, a
//! keyframe is as small as the transform's constant-bitrate control makes
//! it: near the budget on the hardware ones, several times over it on
//! Microsoft's.

use vnilink::YuvMatrix;
#[cfg(windows)]
use vnilink_media::{bgra_to_nv12, RecoveryQp};
use vnilink_media::{EncodeParams, EncodedSample, Error, RawFrame, VideoEncoder};
#[cfg(windows)]
use windows::core::Interface;
#[cfg(windows)]
use windows::Win32::Media::MediaFoundation::{
    eAVEncCommonRateControlMode_CBR, eAVScenarioInfo_DisplayRemoting,
    CODECAPI_AVEncCommonBufferSize, CODECAPI_AVEncCommonMeanBitRate,
    CODECAPI_AVEncCommonRateControlMode, CODECAPI_AVEncVideoForceKeyFrame,
    CODECAPI_AVEncVideoMinQP, CODECAPI_AVScenarioInfo, ICodecAPI, IMFMediaType, IMFSample,
    IMFTransform, MFCreateMediaType, MFCreateMemoryBuffer, MFCreateSample, MFMediaType_Video,
    MFNominalRange_16_235, MFStartup, MFVideoFormat_H264, MFVideoFormat_NV12,
    MFVideoInterlace_Progressive, MFVideoPrimaries_BT709, MFVideoTransFunc_709,
    MFVideoTransferMatrix_BT601, MFVideoTransferMatrix_BT709, MFSTARTUP_NOSOCKET,
    MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, MFT_MESSAGE_NOTIFY_START_OF_STREAM, MFT_OUTPUT_DATA_BUFFER,
    MFT_OUTPUT_STREAM_PROVIDES_SAMPLES, MF_E_TRANSFORM_NEED_MORE_INPUT, MF_LOW_LATENCY,
    MF_MT_AVG_BITRATE, MF_MT_FRAME_RATE, MF_MT_FRAME_SIZE, MF_MT_INTERLACE_MODE, MF_MT_MAJOR_TYPE,
    MF_MT_MAX_KEYFRAME_SPACING, MF_MT_MPEG_SEQUENCE_HEADER, MF_MT_SUBTYPE, MF_MT_TRANSFER_FUNCTION,
    MF_MT_VIDEO_NOMINAL_RANGE, MF_MT_VIDEO_PRIMARIES, MF_MT_YUV_MATRIX, MF_VERSION,
};
#[cfg(windows)]
use windows::Win32::Media::MediaFoundation::{
    IMFDXGIDeviceManager, MFCreateDXGIDeviceManager, MFCreateDXGISurfaceBuffer,
    MFT_MESSAGE_SET_D3D_MANAGER, MF_SA_D3D11_AWARE,
};
#[cfg(windows)]
use windows::Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED};
#[cfg(windows)]
use windows::Win32::System::Variant::{VARIANT, VT_UI4};

#[cfg(windows)]
use crate::bitstream::h264_access_unit;
#[cfg(windows)]
use crate::transform::{h264_encoders, h264_encoders_for, CreatedTransform, H264Transform, Vendor};
#[cfg(windows)]
use crate::transform_events::{TransformEvents, Waited};

/// How long an asynchronous transform may take to accept a picture or
/// return one; longer than any hardware encode, short enough that a
/// transform that holds pictures back fails the frame, not the session.
#[cfg(windows)]
const EVENT_WAIT: std::time::Duration = std::time::Duration::from_millis(200);

pub struct WindowsEncoder {
    #[cfg(windows)]
    transform: CreatedTransform,
    /// Present when the transform is asynchronous.
    #[cfg(windows)]
    events: Option<TransformEvents>,
    #[cfg(windows)]
    vendor: Vendor,
    #[cfg(windows)]
    hardware: bool,
    width: u32,
    height: u32,
    /// The matrix pictures are converted to and the SPS declares.
    #[cfg(windows)]
    matrix: YuvMatrix,
    #[cfg(windows)]
    provides_samples: bool,
    #[cfg(windows)]
    output_size: u32,
    /// The frame rate the transform was opened for; it cannot change.
    #[cfg(windows)]
    frame_rate: u8,
    #[cfg(windows)]
    recovery_qp: RecoveryQp,
    /// Whether captured pictures are converted to NV12 on their GPU.
    #[cfg(windows)]
    gpu: GpuConversion,
    /// The device this transform reads textures on, when it was opened on
    /// the capture's ([`WindowsEncoder::open_on`]): converted pictures are
    /// handed to it there, never read into memory.
    #[cfg(windows)]
    textures: Option<crate::gpu::GpuDevice>,
    /// What the last picture was read from; before the first, what the
    /// encoder was opened to read.
    #[cfg(windows)]
    input: vnilink::EncoderInput,
}

/// Converting captured pictures on the GPU, for one encoder.
#[cfg(windows)]
enum GpuConversion {
    /// No captured picture yet.
    Untried,
    Ready(crate::gpu::Nv12Converter),
    /// The GPU could not convert (no video processor, as on Microsoft's
    /// basic render driver or in some virtual machines): pictures are read
    /// into memory and converted on the CPU until the encoder is reopened.
    Unavailable,
}

// The transform is a COM pointer used through `&mut self` only, from the
// thread that opened it after `CoInitializeEx(COINIT_MULTITHREADED)`. Moving
// it between threads is safe; sharing it is refused by the borrow checker.
#[cfg(windows)]
unsafe impl Send for WindowsEncoder {}

impl WindowsEncoder {
    pub fn open(
        width: u16,
        height: u16,
        frame_rate: u8,
        bitrate_bps: u32,
        matrix: YuvMatrix,
    ) -> Result<Self, Error> {
        if width % 2 != 0 || height % 2 != 0 {
            return Err(Error::InvalidFrame("NV12 width and height must be even"));
        }
        #[cfg(windows)]
        {
            return unsafe {
                Self::open_first(
                    || h264_encoders(),
                    width,
                    height,
                    frame_rate,
                    bitrate_bps,
                    matrix,
                )
            }
            .map_err(|_| Error::Unsupported("Media Foundation H.264"));
        }
        #[cfg(not(windows))]
        {
            let _ = (width, height, frame_rate, bitrate_bps, matrix);
            Err(Error::Unsupported("Media Foundation H.264"))
        }
    }

    /// The first of `candidates` that opens, reading NV12 from memory.
    /// They are listed once Media Foundation has started, and only as far as
    /// the first that opens.
    #[cfg(windows)]
    unsafe fn open_first<I: IntoIterator<Item = H264Transform>>(
        candidates: impl FnOnce() -> I,
        width: u16,
        height: u16,
        frame_rate: u8,
        bitrate_bps: u32,
        matrix: YuvMatrix,
    ) -> windows::core::Result<Self> {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        MFStartup(MF_VERSION, MFSTARTUP_NOSOCKET)?;
        let mut failure = windows::core::Error::from(windows::Win32::Foundation::E_FAIL);
        for candidate in candidates() {
            match Self::create_transform(
                &candidate,
                width,
                height,
                frame_rate,
                bitrate_bps,
                false,
                None,
                matrix,
            ) {
                Ok(encoder) => return Ok(encoder),
                Err(err) => failure = err,
            }
        }
        Err(failure)
    }

    /// Opens `candidate` only, for measuring one transform.
    #[cfg(all(windows, test))]
    pub(crate) fn open_candidate(
        candidate: &H264Transform,
        width: u16,
        height: u16,
        frame_rate: u8,
        bitrate_bps: u32,
        matrix: YuvMatrix,
    ) -> windows::core::Result<Self> {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            MFStartup(MF_VERSION, MFSTARTUP_NOSOCKET)?;
            Self::create_transform(
                candidate,
                width,
                height,
                frame_rate,
                bitrate_bps,
                false,
                None,
                matrix,
            )
        }
    }

    /// Whether the transform takes the `ICodecAPI` setting `key`.
    #[cfg(all(windows, test))]
    pub(crate) fn supports(&self, key: &windows::core::GUID) -> bool {
        self.transform
            .cast::<ICodecAPI>()
            .is_ok_and(|api| unsafe { api.IsSupported(key) }.is_ok())
    }

    /// Sets the `ICodecAPI` value `key` on the transform, for measuring.
    #[cfg(all(windows, test))]
    pub(crate) fn set_codec(&self, key: &windows::core::GUID, value: u32) -> bool {
        unsafe { set_codec_u32(&self.transform, key, value) }.is_ok()
    }

    /// Whether the open transform is a hardware encoder.
    #[cfg(windows)]
    pub(crate) fn hardware(&self) -> bool {
        self.hardware
    }

    /// The caller must own an MTA apartment and Media Foundation startup on
    /// this thread until the encoder has been dropped. Camera capture does so.
    #[cfg(windows)]
    pub(crate) unsafe fn open_camera(
        width: u16,
        height: u16,
        frame_rate: u8,
        bitrate_bps: u32,
    ) -> Result<Self, Error> {
        if width % 2 != 0 || height % 2 != 0 {
            return Err(Error::InvalidFrame("NV12 width and height must be even"));
        }
        Self::create_transform(
            &H264Transform::software(),
            width,
            height,
            frame_rate,
            bitrate_bps,
            true,
            None,
            YuvMatrix::Bt709,
        )
        .map_err(|_| Error::Unsupported("Media Foundation camera H.264"))
    }

    #[cfg(windows)]
    unsafe fn create_transform(
        candidate: &H264Transform,
        width: u16,
        height: u16,
        frame_rate: u8,
        bitrate_bps: u32,
        camera: bool,
        manager: Option<&IMFDXGIDeviceManager>,
        matrix: YuvMatrix,
    ) -> windows::core::Result<Self> {
        let transform = candidate.create()?;
        if let Some(manager) = manager {
            // Before any media type: the transform then reads textures on the
            // manager's device.
            let aware = transform
                .GetAttributes()
                .and_then(|attributes| attributes.GetUINT32(&MF_SA_D3D11_AWARE))
                .unwrap_or(0);
            if aware == 0 {
                return Err(windows::Win32::Foundation::E_NOINTERFACE.into());
            }
            transform.ProcessMessage(MFT_MESSAGE_SET_D3D_MANAGER, manager.as_raw() as usize)?;
        }
        if let Ok(attributes) = transform.GetAttributes() {
            let _ = attributes.SetUINT32(&MF_LOW_LATENCY, 1);
        }
        let rate = u32::from(frame_rate.max(1));
        if !camera {
            // Constant bitrate with one picture of buffer, so no picture,
            // a keyframe included, outruns the link; and the scenario this
            // transform tunes for screens. Settings it does not know are
            // left at its defaults.
            let _ = set_codec_u32(
                &transform,
                &CODECAPI_AVEncCommonRateControlMode,
                eAVEncCommonRateControlMode_CBR.0 as u32,
            );
            let _ = set_codec_u32(
                &transform,
                &CODECAPI_AVEncCommonBufferSize,
                bitrate_bps / rate,
            );
            let _ = set_codec_u32(
                &transform,
                &CODECAPI_AVScenarioInfo,
                eAVScenarioInfo_DisplayRemoting.0 as u32,
            );
        }
        let output = video_type(MFVideoFormat_H264, width, height, rate, matrix)?;
        output.SetUINT32(&MF_MT_AVG_BITRATE, bitrate_bps)?;
        // A desktop viewer asks for a keyframe when it needs one; the
        // periodic one is only a backstop, every ten seconds.
        let spacing = if camera { rate } else { rate * 10 };
        output.SetUINT32(&MF_MT_MAX_KEYFRAME_SPACING, spacing)?;
        if camera || candidate.hardware() {
            // Baseline excludes B pictures, and is the H.264 every viewer
            // decodes; hardware transforms default to a higher profile.
            // Prefer the explicit constrained profile; older transforms
            // accept Baseline only. The camera publisher still checks
            // emitted SPS constraint flags before use.
            use windows::Win32::Media::MediaFoundation::{
                eAVEncH264VProfile_Base, eAVEncH264VProfile_ConstrainedBase, MF_MT_MPEG2_PROFILE,
            };
            output.SetUINT32(
                &MF_MT_MPEG2_PROFILE,
                eAVEncH264VProfile_ConstrainedBase.0 as u32,
            )?;
            if transform.SetOutputType(0, &output, 0).is_err() {
                output.SetUINT32(&MF_MT_MPEG2_PROFILE, eAVEncH264VProfile_Base.0 as u32)?;
                transform.SetOutputType(0, &output, 0)?;
            }
        } else {
            transform.SetOutputType(0, &output, 0)?;
        }
        let input = video_type(MFVideoFormat_NV12, width, height, rate, matrix)?;
        transform.SetInputType(0, &input, 0)?;
        let events = if candidate.hardware() {
            Some(TransformEvents::listen(&transform)?)
        } else {
            None
        };
        transform.ProcessMessage(MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, 0)?;
        transform.ProcessMessage(MFT_MESSAGE_NOTIFY_START_OF_STREAM, 0)?;
        let info = transform.GetOutputStreamInfo(0)?;
        let provides_samples = info.dwFlags & (MFT_OUTPUT_STREAM_PROVIDES_SAMPLES.0 as u32) != 0;
        let output_size = if info.cbSize == 0 {
            u32::from(width) * u32::from(height)
        } else {
            info.cbSize
        };
        Ok(Self {
            transform,
            events,
            vendor: candidate.vendor,
            hardware: candidate.hardware(),
            width: u32::from(width),
            height: u32::from(height),
            matrix,
            provides_samples,
            output_size,
            frame_rate,
            recovery_qp: RecoveryQp::default(),
            gpu: GpuConversion::Untried,
            textures: None,
            input: if manager.is_some() {
                vnilink::EncoderInput::GpuTexture
            } else if camera {
                vnilink::EncoderInput::Cpu
            } else {
                vnilink::EncoderInput::GpuReadBack
            },
        })
    }
}

impl VideoEncoder for WindowsEncoder {
    /// The Media Foundation transform that opened, by vendor.
    fn implementation(&self) -> vnilink::EncoderImplementation {
        #[cfg(windows)]
        {
            vnilink::EncoderImplementation {
                name: self.vendor.label(),
                hardware: Some(self.hardware),
                input: Some(self.input),
            }
        }
        #[cfg(not(windows))]
        {
            vnilink::EncoderImplementation::UNKNOWN
        }
    }

    /// A captured picture is converted to NV12 and scaled on its GPU.
    fn scales_native(&self) -> bool {
        cfg!(windows)
    }

    /// Per transform, as measured ([`Vendor::refines_still_pictures`]).
    fn refines_still_pictures(&self) -> bool {
        #[cfg(windows)]
        {
            self.vendor.refines_still_pictures()
        }
        #[cfg(not(windows))]
        {
            false
        }
    }

    /// The mean bitrate changes while encoding, on a transform that honours
    /// it ([`Vendor::retunes`]); a new frame rate needs a new transform.
    fn retune(&mut self, bitrate_bps: u32, frame_rate: u8) -> Result<bool, Error> {
        #[cfg(windows)]
        {
            if frame_rate != self.frame_rate || bitrate_bps == 0 || !self.vendor.retunes() {
                return Ok(false);
            }
            let rate = u32::from(frame_rate.max(1));
            // SAFETY: `transform` is this encoder's live transform, used from
            // the thread that owns it, as every other call here.
            let taken = unsafe {
                set_codec_u32(
                    &self.transform,
                    &CODECAPI_AVEncCommonMeanBitRate,
                    bitrate_bps,
                )
            }
            .is_ok();
            if taken {
                // SAFETY: as above.
                let _ = unsafe {
                    set_codec_u32(
                        &self.transform,
                        &CODECAPI_AVEncCommonBufferSize,
                        bitrate_bps / rate,
                    )
                };
            }
            Ok(taken)
        }
        #[cfg(not(windows))]
        {
            let _ = (bitrate_bps, frame_rate);
            Ok(false)
        }
    }

    fn encode(&mut self, frame: &RawFrame, params: &EncodeParams) -> Result<EncodedSample, Error> {
        #[cfg(windows)]
        let captured = frame.native_picture().and_then(|picture| {
            picture
                .as_any()
                .downcast_ref::<crate::gpu::WindowsPicture>()
        });
        #[cfg(not(windows))]
        let captured: Option<()> = None;
        if captured.is_none()
            && (frame.width() as u32 != self.width || frame.height() as u32 != self.height)
        {
            return Err(Error::InvalidFrame(
                "the frame size does not match the encoder",
            ));
        }
        #[cfg(windows)]
        {
            // SAFETY (floor and clear): `transform` is this encoder's live
            // transform, used from the thread that owns it.
            let floored = params.force_keyframe
                && params.recovery
                && self.vendor.takes_qp_floor()
                && unsafe {
                    set_codec_u32(
                        &self.transform,
                        &CODECAPI_AVEncVideoMinQP,
                        u32::from(self.recovery_qp.floor()),
                    )
                }
                .is_ok();
            let sample = self.encode_picture(frame, captured, params);
            if floored {
                let _ = unsafe { set_codec_u32(&self.transform, &CODECAPI_AVEncVideoMinQP, 0) };
            }
            let sample = sample?;
            if floored {
                self.recovery_qp.observe(sample.bytes.len(), params);
            } else if params.force_keyframe {
                self.recovery_qp.observe_full(sample.bytes.len(), params);
            }
            return h264_access_unit(&sample.bytes, &sample.header, self.matrix);
        }
        #[cfg(not(windows))]
        {
            let _ = params;
            Err(Error::Unsupported("Media Foundation H.264"))
        }
    }
}

#[cfg(windows)]
struct MfSample {
    bytes: Vec<u8>,
    header: Vec<u8>,
}

#[cfg(windows)]
impl WindowsEncoder {
    /// This encoder's converter for `picture`, made for the first; `None`
    /// once the GPU cannot convert.
    fn converter_for(
        &mut self,
        picture: &crate::gpu::WindowsPicture,
    ) -> Option<&crate::gpu::Nv12Converter> {
        use vnilink_media::NativePicture;
        let fits = match &self.gpu {
            GpuConversion::Unavailable => return None,
            GpuConversion::Ready(converter) => converter.fits(picture),
            GpuConversion::Untried => false,
        };
        if !fits {
            let (width, height) = picture.size();
            let made = unsafe {
                crate::gpu::Nv12Converter::new(
                    picture.device(),
                    picture.context(),
                    (u32::from(width), u32::from(height)),
                    (self.width, self.height),
                    self.matrix,
                )
            };
            self.gpu = made.map_or(GpuConversion::Unavailable, GpuConversion::Ready);
        }
        match &self.gpu {
            GpuConversion::Ready(converter) => Some(converter),
            _ => None,
        }
    }

    /// A captured picture as NV12 at this encoder's size, converted on the
    /// GPU that holds it; only the NV12 picture is read into memory. `None`
    /// once the GPU cannot: the caller converts on the CPU instead.
    fn convert(&mut self, picture: &crate::gpu::WindowsPicture) -> Option<(Vec<u8>, Vec<u8>)> {
        let converted = unsafe { self.converter_for(picture)?.convert(picture) };
        converted
            .map_err(|_| self.gpu = GpuConversion::Unavailable)
            .ok()
    }

    /// One picture into the transform: as a texture where this encoder reads
    /// textures on the picture's device, else as NV12 in memory, converted
    /// on the GPU where it can be and on the CPU otherwise.
    fn encode_picture(
        &mut self,
        frame: &RawFrame,
        captured: Option<&crate::gpu::WindowsPicture>,
        params: &EncodeParams,
    ) -> Result<MfSample, Error> {
        let on_device = captured.filter(|picture| {
            self.textures
                .as_ref()
                .is_some_and(|device| device.device().as_raw() == picture.device().as_raw())
        });
        if let Some(picture) = on_device {
            let converted = self
                .converter_for(picture)
                .map(|converter| unsafe { converter.convert_texture(picture) });
            match converted {
                Some(Ok(texture)) => {
                    match unsafe { self.encode_texture(&texture, frame.timestamp_us(), params) } {
                        Ok(sample) => {
                            self.input = vnilink::EncoderInput::GpuTexture;
                            return Ok(sample);
                        }
                        // This transform takes memory too: the rest go that way.
                        Err(_) => self.textures = None,
                    }
                }
                Some(Err(_)) => self.gpu = GpuConversion::Unavailable,
                None => {}
            }
        }
        let (y_plane, uv_plane) = match captured.and_then(|picture| self.convert(picture)) {
            Some(planes) => {
                self.input = vnilink::EncoderInput::GpuReadBack;
                planes
            }
            None => {
                self.input = vnilink::EncoderInput::Cpu;
                self.convert_on_cpu(frame)?
            }
        };
        unsafe { self.encode_nv12(&y_plane, &uv_plane, frame.timestamp_us(), params) }
    }

    /// `frame` as NV12 converted on the CPU, scaled first if it is a captured
    /// picture of another size; reading a captured picture copies it out.
    fn convert_on_cpu(&self, frame: &RawFrame) -> Result<(Vec<u8>, Vec<u8>), Error> {
        let size = (u32::from(frame.width()), u32::from(frame.height()));
        if size == (self.width, self.height) {
            return Ok(bgra_to_nv12(frame, self.matrix)?.into_planes());
        }
        let scaled = vnilink_media::scale_bgra(frame, self.width as u16, self.height as u16)?;
        Ok(bgra_to_nv12(&scaled, self.matrix)?.into_planes())
    }

    /// Whether captured pictures are converted on the GPU: `None` before the
    /// first one.
    #[cfg(test)]
    pub(crate) fn converts_on_gpu(&self) -> Option<bool> {
        match self.gpu {
            GpuConversion::Untried => None,
            GpuConversion::Ready(_) => Some(true),
            GpuConversion::Unavailable => Some(false),
        }
    }

    #[cfg(test)]
    pub(crate) fn vendor(&self) -> Vendor {
        self.vendor
    }

    /// As if the GPU could not convert, to exercise the CPU path.
    #[cfg(test)]
    pub(crate) fn refuse_gpu_conversion(&mut self) {
        self.gpu = GpuConversion::Unavailable;
    }

    unsafe fn encode_nv12(
        &mut self,
        y_plane: &[u8],
        uv_plane: &[u8],
        timestamp_us: u64,
        params: &EncodeParams,
    ) -> Result<MfSample, Error> {
        let prepared = (|| unsafe {
            let size = (y_plane.len() + uv_plane.len()) as u32;
            let buffer = MFCreateMemoryBuffer(size)?;
            let mut data = std::ptr::null_mut();
            let mut max_length = 0u32;
            let mut current = 0u32;
            buffer.Lock(&mut data, Some(&mut max_length), Some(&mut current))?;
            std::ptr::copy_nonoverlapping(y_plane.as_ptr(), data, y_plane.len());
            std::ptr::copy_nonoverlapping(
                uv_plane.as_ptr(),
                data.add(y_plane.len()),
                uv_plane.len(),
            );
            buffer.Unlock()?;
            buffer.SetCurrentLength(size)?;
            let sample = MFCreateSample()?;
            sample.AddBuffer(&buffer)?;
            sample.SetSampleTime(
                i64::try_from(timestamp_us.saturating_mul(10)).unwrap_or(i64::MAX),
            )?;
            let duration = 10_000_000i64 / i64::from(params.frame_rate.max(1));
            sample.SetSampleDuration(duration)?;
            Ok::<_, windows::core::Error>(sample)
        })();
        let sample =
            prepared.map_err(|_| Error::EncodeRejected("the encoder rejected the frame"))?;
        self.submit(&sample, params)
    }

    /// A converted picture on the device this transform reads textures on.
    unsafe fn encode_texture(
        &mut self,
        texture: &windows::Win32::Graphics::Direct3D11::ID3D11Texture2D,
        timestamp_us: u64,
        params: &EncodeParams,
    ) -> Result<MfSample, Error> {
        let prepared = (|| unsafe {
            let buffer = MFCreateDXGISurfaceBuffer(
                &windows::Win32::Graphics::Direct3D11::ID3D11Texture2D::IID,
                texture,
                0,
                false,
            )?;
            let sample = MFCreateSample()?;
            sample.AddBuffer(&buffer)?;
            sample.SetSampleTime(
                i64::try_from(timestamp_us.saturating_mul(10)).unwrap_or(i64::MAX),
            )?;
            sample.SetSampleDuration(10_000_000i64 / i64::from(params.frame_rate.max(1)))?;
            Ok::<_, windows::core::Error>(sample)
        })();
        let sample =
            prepared.map_err(|_| Error::EncodeRejected("the encoder rejected the frame"))?;
        self.submit(&sample, params)
    }

    /// Hands `sample` to the transform and takes the picture it makes.
    unsafe fn submit(
        &mut self,
        sample: &IMFSample,
        params: &EncodeParams,
    ) -> Result<MfSample, Error> {
        if let Some(events) = &mut self.events {
            if events.take_input(std::time::Instant::now() + EVENT_WAIT) != Waited::Ready {
                return Err(Error::EncodeRejected("the encoder did not take the frame"));
            }
        }
        let processed = (|| unsafe {
            if params.force_keyframe {
                force_idr(&self.transform)?;
            }
            self.transform.ProcessInput(0, sample, 0)
        })();
        processed.map_err(rejected)?;
        if let Some(events) = &mut self.events {
            if events.take_output(std::time::Instant::now() + EVENT_WAIT) != Waited::Ready {
                return Err(Error::EncodeRejected("the encoder produced no frame"));
            }
        }
        self.drain()
    }

    unsafe fn drain(&self) -> Result<MfSample, Error> {
        let mut buffers = [MFT_OUTPUT_DATA_BUFFER::default()];
        if !self.provides_samples {
            let prepared = (|| unsafe {
                let buffer = MFCreateMemoryBuffer(self.output_size)?;
                let sample = MFCreateSample()?;
                sample.AddBuffer(&buffer)?;
                buffers[0].pSample = std::mem::ManuallyDrop::new(Some(sample));
                Ok::<_, windows::core::Error>(())
            })();
            if let Err(err) = prepared {
                let _sample = std::mem::ManuallyDrop::take(&mut buffers[0].pSample);
                return Err(rejected(err));
            }
        }
        let mut status = 0u32;
        let processed = self.transform.ProcessOutput(0, &mut buffers, &mut status);
        let sample = std::mem::ManuallyDrop::take(&mut buffers[0].pSample);
        let _events = std::mem::ManuallyDrop::take(&mut buffers[0].pEvents);
        let sample = match processed {
            Ok(()) => sample.ok_or(Error::EncodeRejected("the encoder produced no frame"))?,
            Err(err) if err.code() == MF_E_TRANSFORM_NEED_MORE_INPUT => {
                return Err(Error::EncodeRejected("the encoder produced no frame"));
            }
            Err(err) => return Err(rejected(err)),
        };
        let bytes = sample_bytes(&sample).map_err(rejected)?;
        let header = sequence_header(&self.transform).unwrap_or_default();
        Ok(MfSample { bytes, header })
    }
}

#[cfg(windows)]
fn rejected(err: windows::core::Error) -> Error {
    let _ = err;
    Error::EncodeRejected("the encoder rejected the frame")
}

/// A DXGI device manager over `device`, for a transform to read its textures.
#[cfg(windows)]
unsafe fn device_manager(
    device: &crate::gpu::GpuDevice,
) -> windows::core::Result<IMFDXGIDeviceManager> {
    let mut token = 0u32;
    let mut manager = None;
    MFCreateDXGIDeviceManager(&mut token, &mut manager)?;
    let manager =
        manager.ok_or_else(|| windows::core::Error::from(windows::Win32::Foundation::E_FAIL))?;
    manager.ResetDevice(device.device(), token)?;
    Ok(manager)
}

/// Asks the encoder to make its next output picture an IDR, before the input
/// sample is processed. CODECAPI_AVEncVideoForceKeyFrame requires a ULONG
/// (VT_UI4); a boolean variant is rejected by the Microsoft encoder.
#[cfg(windows)]
unsafe fn force_idr(transform: &IMFTransform) -> windows::core::Result<()> {
    set_codec_u32(transform, &CODECAPI_AVEncVideoForceKeyFrame, 1)
}

/// Sets one `ICodecAPI` value of type ULONG (VT_UI4) on the transform.
#[cfg(windows)]
unsafe fn set_codec_u32(
    transform: &IMFTransform,
    key: &windows::core::GUID,
    value: u32,
) -> windows::core::Result<()> {
    let api: ICodecAPI = transform.cast()?;
    let mut variant = VARIANT::default();
    (*variant.Anonymous.Anonymous).vt = VT_UI4;
    (*variant.Anonymous.Anonymous).Anonymous.ulVal = value;
    api.SetValue(key, &variant)
}

#[cfg(windows)]
#[cfg(windows)]
pub(crate) unsafe fn sample_bytes(sample: &IMFSample) -> windows::core::Result<Vec<u8>> {
    let buffer = sample.GetBufferByIndex(0)?;
    let mut data = std::ptr::null_mut();
    let mut max_length = 0u32;
    let mut current = 0u32;
    buffer.Lock(&mut data, Some(&mut max_length), Some(&mut current))?;
    let bytes = if data.is_null() || current == 0 {
        Vec::new()
    } else {
        std::slice::from_raw_parts(data, current as usize).to_vec()
    };
    buffer.Unlock()?;
    Ok(bytes)
}

#[cfg(windows)]
unsafe fn sequence_header(transform: &IMFTransform) -> windows::core::Result<Vec<u8>> {
    let media_type = transform.GetOutputCurrentType(0)?;
    let size = media_type.GetBlobSize(&MF_MT_MPEG_SEQUENCE_HEADER)?;
    if size == 0 {
        return Ok(Vec::new());
    }
    let mut bytes = vec![0u8; size as usize];
    let mut written = 0u32;
    media_type.GetBlob(&MF_MT_MPEG_SEQUENCE_HEADER, &mut bytes, Some(&mut written))?;
    let written = usize::try_from(written).unwrap_or(0).min(bytes.len());
    bytes.truncate(written);
    Ok(bytes)
}

#[cfg(windows)]
pub(crate) unsafe fn video_type(
    subtype: windows::core::GUID,
    width: u16,
    height: u16,
    frame_rate: u32,
    matrix: YuvMatrix,
) -> windows::core::Result<IMFMediaType> {
    let media_type = MFCreateMediaType()?;
    media_type.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
    media_type.SetGUID(&MF_MT_SUBTYPE, &subtype)?;
    media_type.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)?;
    media_type.SetUINT64(
        &MF_MT_FRAME_SIZE,
        // Width travels in the high word, height in the low word.
        pack_hi_lo(u32::from(width), u32::from(height)),
    )?;
    media_type.SetUINT64(
        &MF_MT_FRAME_RATE,
        // Numerator travels in the high word, denominator in the low word.
        pack_hi_lo(frame_rate, 1),
    )?;
    // Pictures are converted to NV12 in the session's matrix, in limited
    // range (`vnilink_media::VideoSignal::stream`). Every keyframe's SPS says
    // so whatever the transform writes; the type tells it the same.
    media_type.SetUINT32(
        &MF_MT_YUV_MATRIX,
        match matrix {
            YuvMatrix::Bt709 => MFVideoTransferMatrix_BT709,
            YuvMatrix::Bt601 => MFVideoTransferMatrix_BT601,
        }
        .0 as u32,
    )?;
    media_type.SetUINT32(&MF_MT_VIDEO_PRIMARIES, MFVideoPrimaries_BT709.0 as u32)?;
    media_type.SetUINT32(&MF_MT_TRANSFER_FUNCTION, MFVideoTransFunc_709.0 as u32)?;
    media_type.SetUINT32(&MF_MT_VIDEO_NOMINAL_RANGE, MFNominalRange_16_235.0 as u32)?;
    Ok(media_type)
}

#[cfg(windows)]
fn pack_hi_lo(high: u32, low: u32) -> u64 {
    u64::from(low) | (u64::from(high) << 32)
}

/// Media Foundation encodes H.264 here; other codecs use software.
fn h264_only(codec: vnilink::VideoCodec) -> Result<(), Error> {
    if codec == vnilink::VideoCodec::H264 {
        Ok(())
    } else {
        Err(Error::Unsupported("Media Foundation for this codec"))
    }
}

impl WindowsEncoder {
    pub fn open_codec(
        w: u16,
        h: u16,
        fps: u8,
        bps: u32,
        codec: u8,
        matrix: YuvMatrix,
    ) -> Result<vnilink_media::CodecEncoder<Self>, Error> {
        vnilink_media::CodecEncoder::open(codec, matrix, w, h, fps, bps, |codec, w, h, fps, bps| {
            h264_only(codec)?;
            Self::open(w, h, fps, bps, matrix)
        })
    }

    /// [`Self::open_codec`] for pictures that live on `device`.
    #[cfg(windows)]
    pub fn open_codec_on(
        w: u16,
        h: u16,
        fps: u8,
        bps: u32,
        codec: u8,
        matrix: YuvMatrix,
        device: &crate::gpu::GpuDevice,
    ) -> Result<vnilink_media::CodecEncoder<Self>, Error> {
        vnilink_media::CodecEncoder::open(codec, matrix, w, h, fps, bps, |codec, w, h, fps, bps| {
            h264_only(codec)?;
            Self::open_on(w, h, fps, bps, matrix, device)
        })
    }

    /// An encoder for pictures captured on `device`. The first that opens of:
    /// a D3D11-aware hardware transform on `device`'s adapter, which reads
    /// the converted pictures as textures there and none is read into
    /// memory (path 1); then a transform reading them from memory, the
    /// hardware ones on `device`'s adapter first (path 3, or 4 where the
    /// GPU cannot convert). Sharing a texture with another adapter's encoder
    /// (path 2) is not tried: D3D11 refuses it between an integrated and a
    /// discrete GPU.
    #[cfg(windows)]
    pub fn open_on(
        width: u16,
        height: u16,
        frame_rate: u8,
        bitrate_bps: u32,
        matrix: YuvMatrix,
        device: &crate::gpu::GpuDevice,
    ) -> Result<Self, Error> {
        if width % 2 != 0 || height % 2 != 0 {
            return Err(Error::InvalidFrame("NV12 width and height must be even"));
        }
        if let Some(encoder) = unsafe {
            Self::open_reading_textures(width, height, frame_rate, bitrate_bps, matrix, device)
        } {
            return Ok(encoder);
        }
        unsafe {
            let candidates = || h264_encoders_for(device.adapter().ok());
            Self::open_first(candidates, width, height, frame_rate, bitrate_bps, matrix)
        }
        .map_err(|_| Error::Unsupported("Media Foundation H.264"))
    }

    #[cfg(windows)]
    unsafe fn open_reading_textures(
        width: u16,
        height: u16,
        frame_rate: u8,
        bitrate_bps: u32,
        matrix: YuvMatrix,
        device: &crate::gpu::GpuDevice,
    ) -> Option<Self> {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        MFStartup(MF_VERSION, MFSTARTUP_NOSOCKET).ok()?;
        let manager = device_manager(device).ok()?;
        let adapter = device.adapter().ok()?;
        crate::transform::hardware_encoders_on(adapter)
            .iter()
            .find_map(|candidate| {
                Self::create_transform(
                    candidate,
                    width,
                    height,
                    frame_rate,
                    bitrate_bps,
                    false,
                    Some(&manager),
                    matrix,
                )
                .ok()
            })
            .map(|mut encoder| {
                encoder.textures = Some(device.clone());
                encoder
            })
    }

    /// Whether this encoder reads converted pictures as textures.
    #[cfg(windows)]
    pub fn reads_textures(&self) -> bool {
        self.textures.is_some()
    }
}
