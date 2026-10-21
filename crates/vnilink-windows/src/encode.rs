//! Media Foundation H.264. Input is NV12. Output is Annex-B.
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
//! pictures' bytes, and the floor is cleared once its picture is drained.
//! A floor rather than a higher `AVEncVideoMaxQP`: on VideoToolbox, the only
//! encoder this was measured on, raising the ceiling left a forced keyframe
//! as large, and a floor shrank it. The transform here is synchronous and
//! drained after each input, so no other picture is encoded under the floor.

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
    MFVideoInterlace_Progressive, MFVideoTransferMatrix_BT601, MFSTARTUP_NOSOCKET,
    MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, MFT_MESSAGE_NOTIFY_START_OF_STREAM, MFT_OUTPUT_DATA_BUFFER,
    MFT_OUTPUT_STREAM_PROVIDES_SAMPLES, MF_E_TRANSFORM_NEED_MORE_INPUT, MF_LOW_LATENCY,
    MF_MT_AVG_BITRATE, MF_MT_FRAME_RATE, MF_MT_FRAME_SIZE, MF_MT_INTERLACE_MODE, MF_MT_MAJOR_TYPE,
    MF_MT_MAX_KEYFRAME_SPACING, MF_MT_MPEG_SEQUENCE_HEADER, MF_MT_SUBTYPE,
    MF_MT_VIDEO_NOMINAL_RANGE, MF_MT_YUV_MATRIX, MF_VERSION,
};
#[cfg(windows)]
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED,
};
#[cfg(windows)]
use windows::Win32::System::Variant::{VARIANT, VT_UI4};

#[cfg(windows)]
use crate::bitstream::h264_access_unit;

#[cfg(windows)]
const CLSID_MSH264_ENCODER: windows::core::GUID =
    windows::core::GUID::from_u128(0x6ca5_0344_051a_4ded_9779_a433_0516_5e35);

pub struct WindowsEncoder {
    #[cfg(windows)]
    transform: IMFTransform,
    width: u32,
    height: u32,
    #[cfg(windows)]
    provides_samples: bool,
    #[cfg(windows)]
    output_size: u32,
    /// The frame rate the transform was opened for; it cannot change.
    #[cfg(windows)]
    frame_rate: u8,
    #[cfg(windows)]
    recovery_qp: RecoveryQp,
}

// The transform is a COM pointer used through `&mut self` only, from the
// thread that opened it after `CoInitializeEx(COINIT_MULTITHREADED)`. Moving
// it between threads is safe; sharing it is refused by the borrow checker.
#[cfg(windows)]
unsafe impl Send for WindowsEncoder {}

impl WindowsEncoder {
    pub fn open(width: u16, height: u16, frame_rate: u8, bitrate_bps: u32) -> Result<Self, Error> {
        if width % 2 != 0 || height % 2 != 0 {
            return Err(Error::InvalidFrame("NV12 width and height must be even"));
        }
        #[cfg(windows)]
        {
            return unsafe { Self::open_transform(width, height, frame_rate, bitrate_bps) }
                .map_err(|_| Error::Unsupported("Media Foundation H.264"));
        }
        #[cfg(not(windows))]
        {
            let _ = (width, height, frame_rate, bitrate_bps);
            Err(Error::Unsupported("Media Foundation H.264"))
        }
    }

    #[cfg(windows)]
    unsafe fn open_transform(
        width: u16,
        height: u16,
        frame_rate: u8,
        bitrate_bps: u32,
    ) -> windows::core::Result<Self> {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        MFStartup(MF_VERSION, MFSTARTUP_NOSOCKET)?;
        Self::create_transform(width, height, frame_rate, bitrate_bps, false)
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
        Self::create_transform(width, height, frame_rate, bitrate_bps, true)
            .map_err(|_| Error::Unsupported("Media Foundation camera H.264"))
    }

    #[cfg(windows)]
    unsafe fn create_transform(
        width: u16,
        height: u16,
        frame_rate: u8,
        bitrate_bps: u32,
        camera: bool,
    ) -> windows::core::Result<Self> {
        let transform: IMFTransform =
            CoCreateInstance(&CLSID_MSH264_ENCODER, None, CLSCTX_INPROC_SERVER)?;
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
        let output = video_type(MFVideoFormat_H264, width, height, rate)?;
        output.SetUINT32(&MF_MT_AVG_BITRATE, bitrate_bps)?;
        // A desktop viewer asks for a keyframe when it needs one; the
        // periodic one is only a backstop, every ten seconds.
        let spacing = if camera { rate } else { rate * 10 };
        output.SetUINT32(&MF_MT_MAX_KEYFRAME_SPACING, spacing)?;
        if camera {
            // Baseline excludes B pictures. Prefer the explicit constrained
            // profile; older transforms accept Baseline only. The camera
            // publisher still checks emitted SPS constraint flags before use.
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
        let input = video_type(MFVideoFormat_NV12, width, height, rate)?;
        transform.SetInputType(0, &input, 0)?;
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
            width: u32::from(width),
            height: u32::from(height),
            provides_samples,
            output_size,
            frame_rate,
            recovery_qp: RecoveryQp::default(),
        })
    }
}

impl VideoEncoder for WindowsEncoder {
    /// The mean bitrate changes while encoding; a new frame rate needs a
    /// new transform.
    fn retune(&mut self, bitrate_bps: u32, frame_rate: u8) -> Result<bool, Error> {
        #[cfg(windows)]
        {
            if frame_rate != self.frame_rate || bitrate_bps == 0 {
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
        if frame.width() as u32 != self.width || frame.height() as u32 != self.height {
            return Err(Error::InvalidFrame(
                "the frame size does not match the encoder",
            ));
        }
        #[cfg(windows)]
        {
            let nv12 = bgra_to_nv12(frame)?;
            // SAFETY (floor and clear): `transform` is this encoder's live
            // transform, used from the thread that owns it.
            let floored = params.force_keyframe
                && params.recovery
                && unsafe {
                    set_codec_u32(
                        &self.transform,
                        &CODECAPI_AVEncVideoMinQP,
                        u32::from(self.recovery_qp.floor()),
                    )
                }
                .is_ok();
            let sample = unsafe {
                self.encode_nv12(
                    nv12.y_plane(),
                    nv12.uv_plane(),
                    frame.timestamp_us(),
                    params,
                )
            };
            if floored {
                let _ = unsafe { set_codec_u32(&self.transform, &CODECAPI_AVEncVideoMinQP, 0) };
            }
            let sample = sample?;
            if floored {
                self.recovery_qp.observe(sample.bytes.len(), params);
            } else if params.force_keyframe {
                self.recovery_qp.observe_full(sample.bytes.len(), params);
            }
            return h264_access_unit(&sample.bytes, &sample.header);
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
    unsafe fn encode_nv12(
        &self,
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
            if params.force_keyframe {
                force_idr(&self.transform)?;
            }
            self.transform.ProcessInput(0, &sample, 0)?;
            Ok::<_, windows::core::Error>(sample)
        })();
        prepared.map_err(|_| Error::EncodeRejected("the encoder rejected the frame"))?;
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
    // Pictures are converted to NV12 with BT.601 in limited range; say so,
    // so the bitstream does not leave decoders to guess.
    media_type.SetUINT32(&MF_MT_YUV_MATRIX, MFVideoTransferMatrix_BT601.0 as u32)?;
    media_type.SetUINT32(&MF_MT_VIDEO_NOMINAL_RANGE, MFNominalRange_16_235.0 as u32)?;
    Ok(media_type)
}

#[cfg(windows)]
fn pack_hi_lo(high: u32, low: u32) -> u64 {
    u64::from(low) | (u64::from(high) << 32)
}

impl WindowsEncoder {
    pub fn open_codec(
        w: u16,
        h: u16,
        fps: u8,
        bps: u32,
        codec: u8,
    ) -> Result<vnilink_media::CodecEncoder<Self>, Error> {
        vnilink_media::CodecEncoder::open(codec, w, h, fps, bps, |w, h, fps, bps| {
            Self::open(w, h, fps, bps)
        })
    }
}
