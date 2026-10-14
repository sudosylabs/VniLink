//! VideoToolbox H.264 decode of Annex-B access units.
//!
//! A keyframe carries SPS, PPS, and an IDR. Those parameter sets open the
//! session. Later predicted frames use the same session. Output is BGRA for
//! the UI to paint. Bitstreams are screen content: do not log them.

use std::sync::{Arc, Mutex};

use apple_cf::cf::{CFDictionary, CFNumber, CFString};
use apple_cf::cm::{
    CMBlockBuffer, CMFormatDescription, CMSampleBuffer, CMSampleTimingInfo, CMTime,
};
use apple_cf::cv::CVPixelBuffer;
use videotoolbox::{DecodedFrame, DecompressionSession};
use vnilink::EncodedFrame;
use vnilink_media::{parse_annex_b, Error, RawFrame, VideoDecoder};

const BGRA: u32 = 0x4247_5241;

struct OpenDecoder {
    codec: vnilink::VideoCodec,
    session: DecompressionSession,
    slot: Arc<Mutex<Option<DecodedFrame>>>,
    parameter_sets: Vec<Vec<u8>>,
}

pub struct MacDecoder {
    software: Option<(vnilink::VideoCodec, vnilink_media::SoftwareDecoder)>,
    open: Option<OpenDecoder>,
}

impl MacDecoder {
    pub fn new() -> Self {
        Self {
            open: None,
            software: None,
        }
    }
}

impl Default for MacDecoder {
    fn default() -> Self {
        Self::new()
    }
}

impl VideoDecoder for MacDecoder {
    fn decode(&mut self, frame: &EncodedFrame) -> Result<RawFrame, Error> {
        if frame.codec == vnilink::VideoCodec::H265
            && self
                .software
                .as_ref()
                .is_none_or(|(codec, _)| *codec != frame.codec)
        {
            match self.decode_hevc(frame) {
                Ok(picture) => return Ok(picture),
                Err(error) if !frame.keyframe => return Err(error),
                Err(_) => {
                    self.open = None;
                }
            }
        }
        if frame.codec != vnilink::VideoCodec::H264 {
            if self
                .software
                .as_ref()
                .is_none_or(|(codec, _)| *codec != frame.codec)
            {
                self.software = Some((
                    frame.codec,
                    vnilink_media::SoftwareDecoder::open(frame.codec)?,
                ));
            }
            return self.software.as_mut().unwrap().1.decode(frame);
        }
        let parsed = parse_annex_b(&frame.payload)?;
        let mut parameter_sets = Vec::new();
        let mut picture = Vec::new();
        for unit in parsed.units() {
            if unit.nal_type == 7 || unit.nal_type == 8 {
                parameter_sets.push(unit.bytes().to_vec());
            } else {
                picture.push(unit.bytes().to_vec());
            }
        }
        if frame.keyframe || parsed.has_idr() {
            if !parsed.has_sps() || !parsed.has_pps() || !parsed.has_idr() {
                return Err(Error::DecodeRejected(
                    "a keyframe needs SPS, PPS, and an IDR",
                ));
            }
            self.ensure_session(&parameter_sets, vnilink::VideoCodec::H264)?;
        }
        let open = self
            .open
            .as_ref()
            .ok_or(Error::DecodeRejected("the decoder has not seen a keyframe"))?;
        let sample = sample_buffer(&picture, &open_format(open)?, frame.timestamp_us)?;
        {
            let mut slot = open
                .slot
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            *slot = None;
        }
        open.session
            .decode(&sample)
            .map_err(|_| Error::DecodeRejected("VideoToolbox rejected the frame"))?;
        open.session
            .wait_for_async_frames()
            .map_err(|_| Error::DecodeRejected("VideoToolbox rejected the frame"))?;
        let decoded = open
            .slot
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take()
            .ok_or(Error::DecodeRejected("the decoder produced no frame"))?;
        if decoded.status != 0 {
            return Err(Error::DecodeRejected("VideoToolbox rejected the frame"));
        }
        let image = decoded
            .image_buffer
            .ok_or(Error::DecodeRejected("the decoder produced no frame"))?;
        raw_frame(&image, frame.timestamp_us)
    }
}

impl MacDecoder {
    fn decode_hevc(&mut self, frame: &EncodedFrame) -> Result<RawFrame, Error> {
        let units = vnilink_media::hevc_units(&frame.payload)?;
        let mut sets = Vec::new();
        let mut picture = Vec::new();
        for nal in units {
            let kind = (nal[0] >> 1) & 63;
            if [32, 33, 34].contains(&kind) {
                sets.push(nal.to_vec());
            } else {
                picture.push(nal.to_vec());
            }
        }
        if frame.keyframe {
            if ![32, 33, 34]
                .into_iter()
                .all(|kind| sets.iter().any(|n| (n[0] >> 1) & 63 == kind))
            {
                return Err(Error::DecodeRejected("HEVC keyframe needs parameter sets"));
            }
            self.ensure_session(&sets, vnilink::VideoCodec::H265)?;
        }
        let open = self
            .open
            .as_ref()
            .filter(|open| open.codec == vnilink::VideoCodec::H265)
            .ok_or(Error::DecodeRejected("HEVC decoder needs keyframe"))?;
        let format = open_format(open)?;
        let sample = sample_buffer(&picture, &format, frame.timestamp_us)?;
        open.session
            .decode(&sample)
            .map_err(|_| Error::DecodeRejected("VideoToolbox HEVC frame"))?;
        open.session
            .wait_for_async_frames()
            .map_err(|_| Error::DecodeRejected("VideoToolbox HEVC frame"))?;
        let decoded = open
            .slot
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .take()
            .ok_or(Error::DecodeRejected("HEVC decoder produced no frame"))?;
        if decoded.status != 0 {
            return Err(Error::DecodeRejected("VideoToolbox HEVC frame"));
        }
        raw_frame(
            &decoded
                .image_buffer
                .ok_or(Error::DecodeRejected("HEVC decoder produced no frame"))?,
            frame.timestamp_us,
        )
    }
    fn ensure_session(
        &mut self,
        parameter_sets: &[Vec<u8>],
        codec: vnilink::VideoCodec,
    ) -> Result<(), Error> {
        if let Some(open) = &self.open {
            if open.parameter_sets == parameter_sets && open.codec == codec {
                return Ok(());
            }
        }
        let format = format_description(parameter_sets, codec)?;
        let (width, height) = format
            .video_dimensions()
            .ok_or(Error::DecodeRejected("missing video dimensions"))?;
        if !(1..=4096).contains(&width) || !(1..=4096).contains(&height) {
            return Err(Error::DecodeRejected("video exceeds decoder dimensions"));
        }
        let slot = Arc::new(Mutex::new(None));
        let callback_slot = Arc::clone(&slot);
        let attributes = bgra_attributes()?;
        let session = DecompressionSession::new_with_image_buffer_attributes(
            &format,
            Some(&attributes),
            move |frame| {
                let mut slot = callback_slot
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                *slot = Some(frame);
            },
        )
        .map_err(|_| Error::Unsupported("VideoToolbox H.264"))?;
        self.open = Some(OpenDecoder {
            codec,
            session,
            slot,
            parameter_sets: parameter_sets.to_vec(),
        });
        Ok(())
    }
}

fn open_format(open: &OpenDecoder) -> Result<CMFormatDescription, Error> {
    format_description(&open.parameter_sets, open.codec)
}

fn format_description(
    parameter_sets: &[Vec<u8>],
    codec: vnilink::VideoCodec,
) -> Result<CMFormatDescription, Error> {
    if parameter_sets.is_empty() || parameter_sets.iter().any(|set| set.is_empty()) {
        return Err(Error::DecodeRejected(
            "a keyframe needs SPS, PPS, and an IDR",
        ));
    }
    let pointers: Vec<*const u8> = parameter_sets.iter().map(|set| set.as_ptr()).collect();
    let sizes: Vec<usize> = parameter_sets.iter().map(|set| set.len()).collect();
    let mut description: apple_cf::raw::CMFormatDescriptionRef = std::ptr::null();
    // The vectors stay alive for the call. The created description is +1 and
    // `from_raw` adopts that retain.
    let status = unsafe {
        if codec == vnilink::VideoCodec::H265 {
            apple_cf::raw::CMVideoFormatDescriptionCreateFromHEVCParameterSets(
                apple_cf::raw::kCFAllocatorDefault,
                parameter_sets.len(),
                pointers.as_ptr(),
                sizes.as_ptr(),
                4,
                std::ptr::null(),
                &raw mut description,
            )
        } else {
            apple_cf::raw::CMVideoFormatDescriptionCreateFromH264ParameterSets(
                apple_cf::raw::kCFAllocatorDefault,
                parameter_sets.len(),
                pointers.as_ptr(),
                sizes.as_ptr(),
                4,
                &raw mut description,
            )
        }
    };
    if status != 0 {
        return Err(Error::DecodeRejected(
            "VideoToolbox rejected the parameter sets",
        ));
    }
    unsafe { CMFormatDescription::from_raw(description.cast_mut().cast()) }.ok_or(
        Error::DecodeRejected("VideoToolbox rejected the parameter sets"),
    )
}

fn sample_buffer(
    picture: &[Vec<u8>],
    format: &CMFormatDescription,
    timestamp_us: u64,
) -> Result<CMSampleBuffer, Error> {
    if picture.is_empty() {
        return Err(Error::InvalidFrame("encoded frame is empty"));
    }
    let mut avcc = Vec::new();
    for nal in picture {
        if nal.is_empty() {
            return Err(Error::InvalidFrame("an H.264 NAL unit is empty"));
        }
        let len = u32::try_from(nal.len())
            .map_err(|_| Error::InvalidFrame("encoded frame is larger than 1 MiB"))?;
        avcc.extend_from_slice(&len.to_be_bytes());
        avcc.extend_from_slice(nal);
    }
    let block = CMBlockBuffer::create(&avcc)
        .ok_or(Error::DecodeRejected("VideoToolbox rejected the frame"))?;
    let ticks = i64::try_from(timestamp_us / 1000).unwrap_or(i64::MAX);
    let timing = raw_timing(CMSampleTimingInfo::with_times(
        CMTime::new(1, 1_000),
        CMTime::new(ticks, 1_000),
        CMTime::new(ticks, 1_000),
    ));
    let sample_size = avcc.len();
    let mut sample: apple_cf::raw::CMSampleBufferRef = std::ptr::null_mut();
    // `CMSampleBufferCreateReady` copies the timing and retains the block and
    // format description. `from_raw` adopts the returned +1 sample.
    let status = unsafe {
        apple_cf::raw::CMSampleBufferCreateReady(
            apple_cf::raw::kCFAllocatorDefault,
            block.as_ptr().cast(),
            format.as_ptr().cast_const().cast(),
            1,
            1,
            &raw const timing,
            1,
            &raw const sample_size,
            &raw mut sample,
        )
    };
    if status != 0 {
        return Err(Error::DecodeRejected("VideoToolbox rejected the frame"));
    }
    unsafe { CMSampleBuffer::from_raw(sample.cast()) }
        .ok_or(Error::DecodeRejected("VideoToolbox rejected the frame"))
}

fn raw_timing(timing: CMSampleTimingInfo) -> apple_cf::raw::CMSampleTimingInfo {
    apple_cf::raw::CMSampleTimingInfo {
        duration: raw_time(timing.duration),
        presentationTimeStamp: raw_time(timing.presentation_time_stamp),
        decodeTimeStamp: raw_time(timing.decode_time_stamp),
    }
}

fn raw_time(time: CMTime) -> apple_cf::raw::CMTime {
    apple_cf::raw::CMTime {
        value: time.value,
        timescale: time.timescale,
        flags: time.flags,
        epoch: time.epoch,
    }
}

fn bgra_attributes() -> Result<CFDictionary, Error> {
    let key_ptr =
        unsafe { std::ptr::addr_of!(apple_cf::raw::kCVPixelBufferPixelFormatTypeKey).read() };
    if key_ptr.is_null() {
        return Err(Error::Unsupported("VideoToolbox H.264"));
    }
    // The key is a process-lifetime constant. The wrapper retains its own copy.
    let key = unsafe { CFString::from_raw_borrowed(key_ptr.cast_mut().cast()) }
        .ok_or(Error::Unsupported("VideoToolbox H.264"))?;
    let value = CFNumber::from_u64(u64::from(BGRA));
    Ok(CFDictionary::from_pairs(&[(&key, &value)]))
}

fn raw_frame(buffer: &CVPixelBuffer, timestamp_us: u64) -> Result<RawFrame, Error> {
    if buffer.is_planar() || buffer.pixel_format() != BGRA {
        return Err(Error::DecodeRejected("the decoder did not return BGRA"));
    }
    let width = u16::try_from(buffer.width())
        .map_err(|_| Error::InvalidFrame("frame width and height must be non-zero"))?;
    let height = u16::try_from(buffer.height())
        .map_err(|_| Error::InvalidFrame("frame width and height must be non-zero"))?;
    if width == 0 || height == 0 {
        return Err(Error::InvalidFrame(
            "frame width and height must be non-zero",
        ));
    }
    let guard = buffer
        .lock_read_only()
        .map_err(|_| Error::DecodeRejected("the decoder did not return BGRA"))?;
    let stride = guard.bytes_per_row();
    let row = usize::from(width) * 4;
    if stride < row {
        return Err(Error::InvalidFrame("frame stride is shorter than one row"));
    }
    // The decoder has finished this buffer, this guard is the only mapping,
    // and the copy below only reads.
    let source = unsafe { guard.as_slice() }
        .ok_or(Error::DecodeRejected("the decoder did not return BGRA"))?;
    let mut bgra = vec![0u8; row * usize::from(height)];
    for y in 0..usize::from(height) {
        let src_start = y * stride;
        let dst_start = y * row;
        let end = src_start + row;
        if end > source.len() {
            return Err(Error::InvalidFrame("frame is missing pixels"));
        }
        bgra[dst_start..dst_start + row].copy_from_slice(&source[src_start..end]);
    }
    RawFrame::bgra(width, height, timestamp_us, bgra)
}
