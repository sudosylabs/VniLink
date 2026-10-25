//! Media Foundation H.264 decode of Annex-B access units.
//!
//! A keyframe carries SPS, PPS, and an IDR. The picture size comes from the
//! SPS; those parameter sets open the decoder. Later predicted frames use the
//! same session. Output is NV12, converted to BGRA for the UI to paint in
//! the colour the SPS says, BT.709 limited range when it says none.
//! Bitstreams are screen content: do not log them.
//!
//! One picture in gives that picture out. In low-latency mode the decoder
//! keeps no picture for reordering; it may still be decoding the picture
//! when its output is asked for, and then it is asked again until the
//! picture is out. A viewer never shows the picture before the one it was
//! sent. Draining would also give the picture out, but measured on
//! Microsoft's decoder it loses the pictures after it until a keyframe.
// trace: records every ignored failure

use vnilink::EncodedFrame;
#[cfg(windows)]
use vnilink::YuvMatrix;
use vnilink_media::{
    h264_video_signal, parse_annex_b, Error, RawFrame, VideoDecoder, UNDECLARED_SIGNAL,
};
#[cfg(windows)]
use vnilink_media::{nv12_to_bgra, VideoSignal};

use crate::bitstream::picture_size;

#[cfg(windows)]
use windows::core::Interface;
#[cfg(windows)]
use windows::Win32::Media::MediaFoundation::{
    CLSID_MSH264DecoderMFT, CODECAPI_AVLowLatencyMode, ICodecAPI, IMFTransform,
    MFCreateMemoryBuffer, MFCreateSample, MFStartup, MFVideoFormat_H264, MFVideoFormat_NV12,
    MFSTARTUP_NOSOCKET, MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, MFT_MESSAGE_NOTIFY_START_OF_STREAM,
    MFT_OUTPUT_DATA_BUFFER, MFT_OUTPUT_STREAM_PROVIDES_SAMPLES, MF_E_TRANSFORM_NEED_MORE_INPUT,
    MF_E_TRANSFORM_STREAM_CHANGE, MF_LOW_LATENCY, MF_MT_SUBTYPE, MF_VERSION,
};
#[cfg(windows)]
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED,
};
#[cfg(windows)]
use windows::Win32::System::Variant::{VARIANT, VT_UI4};

#[cfg(windows)]
use crate::encode::{sample_bytes, video_type};

struct OpenDecoder {
    #[cfg(windows)]
    transform: IMFTransform,
    #[cfg(windows)]
    width: u16,
    #[cfg(windows)]
    height: u16,
    #[cfg(windows)]
    parameter_sets: Vec<Vec<u8>>,
    #[cfg(windows)]
    signal: VideoSignal,
}

pub struct WindowsDecoder {
    software: vnilink_media::SoftwareDecoderSlot,
    open: Option<OpenDecoder>,
}

// Same contract as the encoder: the transform moves between threads but is
// only ever touched through `&mut self` on one thread at a time.
#[cfg(windows)]
unsafe impl Send for WindowsDecoder {}

impl WindowsDecoder {
    pub fn new() -> Self {
        Self {
            open: None,
            software: Default::default(),
        }
    }
}

impl Default for WindowsDecoder {
    fn default() -> Self {
        Self::new()
    }
}

impl VideoDecoder for WindowsDecoder {
    fn decode(&mut self, frame: &EncodedFrame) -> Result<RawFrame, Error> {
        if let Some(result) = self.software.decode(frame) {
            return result;
        }
        let parsed = parse_annex_b(&frame.payload)?;
        let mut parameter_sets = Vec::new();
        for unit in parsed.units() {
            if unit.nal_type == 7 || unit.nal_type == 8 {
                parameter_sets.push(unit.bytes().to_vec());
            }
        }
        if frame.keyframe || parsed.has_idr() {
            if !parsed.has_sps() || !parsed.has_pps() || !parsed.has_idr() {
                return Err(Error::DecodeRejected(
                    "a keyframe needs SPS, PPS, and an IDR",
                ));
            }
            self.ensure_session(&parameter_sets)?;
        }
        let Some(open) = self.open.as_ref() else {
            return Err(Error::DecodeRejected("the decoder has not seen a keyframe"));
        };
        #[cfg(windows)]
        {
            decode_picture(&open.transform, frame, open.width, open.height, open.signal)
        }
        #[cfg(not(windows))]
        {
            let _ = open;
            Err(Error::Unsupported("Media Foundation H.264"))
        }
    }
}

impl WindowsDecoder {
    fn ensure_session(&mut self, parameter_sets: &[Vec<u8>]) -> Result<(), Error> {
        #[cfg(windows)]
        {
            if let Some(open) = &self.open {
                if open.parameter_sets == parameter_sets {
                    return Ok(());
                }
            }
        }
        let sps = parameter_sets
            .iter()
            .find(|set| !set.is_empty() && set[0] & 0x1F == 7)
            .ok_or(Error::DecodeRejected(
                "a keyframe needs SPS, PPS, and an IDR",
            ))?;
        let (width, height) = picture_size(sps)?;
        let signal = h264_video_signal(sps)?.unwrap_or(UNDECLARED_SIGNAL);
        #[cfg(windows)]
        {
            let transform = open_transform(width, height)
                .map_err(|_| Error::Unsupported("Media Foundation H.264"))?;
            self.open = Some(OpenDecoder {
                transform,
                width,
                height,
                parameter_sets: parameter_sets.to_vec(),
                signal,
            });
            Ok(())
        }
        #[cfg(not(windows))]
        {
            let _ = (width, height, signal);
            Err(Error::Unsupported("Media Foundation H.264"))
        }
    }
}

#[cfg(windows)]
fn open_transform(width: u16, height: u16) -> windows::core::Result<IMFTransform> {
    unsafe {
        ignored_hr!(
            vnilink::trace::DECODE,
            "a thread with COM already set up keeps it",
            CoInitializeEx(None, COINIT_MULTITHREADED).ok()
        );
        MFStartup(MF_VERSION, MFSTARTUP_NOSOCKET)?;
        let transform: IMFTransform =
            CoCreateInstance(&CLSID_MSH264DecoderMFT, None, CLSCTX_INPROC_SERVER)?;
        // Remote frames must be presented immediately: without low latency
        // the decoder keeps pictures for playback reordering, and returned
        // the picture before the one it was given for most of an Intel
        // encoder's stream. The attribute alone did not stop that; the
        // codec setting did.
        transform.GetAttributes()?.SetUINT32(&MF_LOW_LATENCY, 1)?;
        let api: ICodecAPI = transform.cast()?;
        let mut on = VARIANT::default();
        (*on.Anonymous.Anonymous).vt = VT_UI4;
        (*on.Anonymous.Anonymous).Anonymous.ulVal = 1;
        ignored_hr!(
            vnilink::trace::DECODE,
            "a decoder without low latency mode still decodes",
            api.SetValue(&CODECAPI_AVLowLatencyMode, &on)
        );
        // The nominal rate only fills the media type. Decoding follows input.
        // The parameter sets travel in band on keyframes, so the input type
        // carries no sequence-header blob.
        let rate = 30u32;
        let input = video_type(MFVideoFormat_H264, width, height, rate, YuvMatrix::Bt709)?;
        transform.SetInputType(0, &input, 0)?;
        let output = video_type(MFVideoFormat_NV12, width, height, rate, YuvMatrix::Bt709)?;
        transform.SetOutputType(0, &output, 0)?;
        transform.ProcessMessage(MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, 0)?;
        transform.ProcessMessage(MFT_MESSAGE_NOTIFY_START_OF_STREAM, 0)?;
        Ok(transform)
    }
}

/// Feeds one access unit and reads one NV12 picture.
#[cfg(windows)]
fn decode_picture(
    transform: &IMFTransform,
    frame: &EncodedFrame,
    width: u16,
    height: u16,
    signal: VideoSignal,
) -> Result<RawFrame, Error> {
    let bytes = feed(transform, &frame.payload, frame.timestamp_us)?;
    let (y, uv) = split_nv12(&bytes, width, height)?;
    nv12_to_bgra(width, height, frame.timestamp_us, y, uv, signal)
}

#[cfg(windows)]
fn feed(transform: &IMFTransform, annex_b: &[u8], timestamp_us: u64) -> Result<Vec<u8>, Error> {
    const REJECTED: &str = "Media Foundation rejected the frame";
    unsafe {
        let buffer = MFCreateMemoryBuffer(annex_b.len() as u32)
            .map_err(|_| Error::DecodeRejected(REJECTED))?;
        let mut data = std::ptr::null_mut();
        let mut max_length = 0u32;
        let mut current = 0u32;
        buffer
            .Lock(&mut data, Some(&mut max_length), Some(&mut current))
            .map_err(|_| Error::DecodeRejected(REJECTED))?;
        std::ptr::copy_nonoverlapping(annex_b.as_ptr(), data, annex_b.len());
        buffer
            .Unlock()
            .map_err(|_| Error::DecodeRejected(REJECTED))?;
        buffer
            .SetCurrentLength(annex_b.len() as u32)
            .map_err(|_| Error::DecodeRejected(REJECTED))?;
        let sample = MFCreateSample().map_err(|_| Error::DecodeRejected(REJECTED))?;
        sample
            .AddBuffer(&buffer)
            .map_err(|_| Error::DecodeRejected(REJECTED))?;
        sample
            .SetSampleTime(i64::try_from(timestamp_us.saturating_mul(10)).unwrap_or(i64::MAX))
            .map_err(|_| Error::DecodeRejected(REJECTED))?;
        transform
            .ProcessInput(0, &sample, 0)
            .map_err(|_| Error::DecodeRejected(REJECTED))?;
        // Everything the decoder has, the newest last: a picture it once
        // kept back comes before this one and is not shown.
        let deadline = std::time::Instant::now() + DECODE_WAIT;
        loop {
            let mut newest = None;
            while let Some(bytes) = take_output(transform)? {
                newest = Some(bytes);
            }
            if let Some(bytes) = newest {
                return Ok(bytes);
            }
            // Still decoding: measured on Intel's and Microsoft's streams,
            // the picture was out within a few milliseconds ([`DECODE_WAIT`]).
            if std::time::Instant::now() >= deadline {
                return Err(Error::DecodeRejected("the decoder produced no frame"));
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    }
}

/// How long a picture may take to come out after its input: longer than
/// any measured decode, short enough that a decoder that keeps pictures
/// fails the frame instead of stalling the stream.
///
/// Measured on one hybrid Intel + NVIDIA laptop (2026-10-22): Microsoft's
/// decoder, which decodes on the CPU, with streams from Intel's and
/// Microsoft's encoders. Streams from NVIDIA's and AMD's encoders and
/// slower CPUs take the same bound unmeasured; `decode_wait` in the decoder
/// tests measures them.
#[cfg(windows)]
pub(crate) const DECODE_WAIT: std::time::Duration = std::time::Duration::from_millis(100);

/// One picture the decoder has ready, or `None` when it needs more input.
#[cfg(windows)]
unsafe fn take_output(transform: &IMFTransform) -> Result<Option<Vec<u8>>, Error> {
    const REJECTED: &str = "Media Foundation rejected the frame";
    // A keyframe whose SPS describes the stream differently, such as its
    // colour, changes the output format: take NV12 again and ask again.
    for _ in 0..2 {
        let mut buffers = [MFT_OUTPUT_DATA_BUFFER::default()];
        let info = transform
            .GetOutputStreamInfo(0)
            .map_err(|_| Error::DecodeRejected(REJECTED))?;
        if info.dwFlags & (MFT_OUTPUT_STREAM_PROVIDES_SAMPLES.0 as u32) == 0 {
            let output = MFCreateMemoryBuffer(info.cbSize.max(1))
                .map_err(|_| Error::DecodeRejected(REJECTED))?;
            let output_sample = MFCreateSample().map_err(|_| Error::DecodeRejected(REJECTED))?;
            output_sample
                .AddBuffer(&output)
                .map_err(|_| Error::DecodeRejected(REJECTED))?;
            buffers[0].pSample = std::mem::ManuallyDrop::new(Some(output_sample));
        }
        let mut status = 0u32;
        let processed = transform.ProcessOutput(0, &mut buffers, &mut status);
        let sample = std::mem::ManuallyDrop::take(&mut buffers[0].pSample);
        let _events = std::mem::ManuallyDrop::take(&mut buffers[0].pEvents);
        match processed {
            Ok(()) => {
                let sample =
                    sample.ok_or(Error::DecodeRejected("the decoder produced no frame"))?;
                return sample_bytes(&sample)
                    .map(Some)
                    .map_err(|_| Error::DecodeRejected("the decoder produced no frame"));
            }
            Err(err) if err.code() == MF_E_TRANSFORM_NEED_MORE_INPUT => return Ok(None),
            Err(err) if err.code() == MF_E_TRANSFORM_STREAM_CHANGE => {
                choose_nv12(transform).map_err(|_| Error::DecodeRejected(REJECTED))?;
            }
            Err(_) => return Err(Error::DecodeRejected(REJECTED)),
        }
    }
    Err(Error::DecodeRejected(REJECTED))
}

/// Sets the first NV12 output type the decoder offers after a format change.
#[cfg(windows)]
unsafe fn choose_nv12(transform: &IMFTransform) -> windows::core::Result<()> {
    for index in 0.. {
        let offered = transform.GetOutputAvailableType(0, index)?;
        if offered.GetGUID(&MF_MT_SUBTYPE)? == MFVideoFormat_NV12 {
            return transform.SetOutputType(0, &offered, 0);
        }
    }
    unreachable!("GetOutputAvailableType ends with an error")
}

/// Splits one contiguous NV12 picture into its planes.
#[cfg(windows)]
///
/// The decoder returns the coded picture: 1080 rows are coded, and returned,
/// as 1088. Chroma starts after all of the coded luma rows; the rows past
/// `height` are cropped off.
fn split_nv12(bytes: &[u8], width: u16, height: u16) -> Result<(&[u8], &[u8]), Error> {
    let (w, h) = (usize::from(width), usize::from(height));
    let coded = (bytes.len() / (w * 3)) * 2;
    if coded < h || bytes.len() < w * coded * 3 / 2 {
        return Err(Error::DecodeRejected("the decoder produced no frame"));
    }
    Ok((&bytes[..w * h], &bytes[w * coded..w * coded + w * h / 2]))
}
