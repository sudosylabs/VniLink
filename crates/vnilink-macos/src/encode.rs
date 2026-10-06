//! VideoToolbox H.264 encode of BGRA pictures.
//!
//! The session emits length-prefixed NALs. SPS and PPS stay in the format
//! description, so a keyframe is rewritten as Annex-B with those sets in front.
//! This encoder does not capture the screen. Callers supply the picture.
//! Without VideoToolbox H.264 this returns unsupported.

#[cfg(target_os = "macos")]
use apple_cf::cm::CMTime;
#[cfg(target_os = "macos")]
use apple_cf::iosurface::IOSurface;
#[cfg(target_os = "macos")]
use videotoolbox::compression::{CompressionSession, FrameProperties, ProfileLevel};
#[cfg(target_os = "macos")]
use videotoolbox::session::Codec;
use vnilink_media::{
    to_annex_b, AvccAccessUnit, EncodeParams, EncodedSample, Error, RawFrame, VideoEncoder,
};

#[cfg(target_os = "macos")]
const BGRA: u32 = 0x4247_5241;

pub struct MacEncoder {
    #[cfg(target_os = "macos")]
    session: CompressionSession,
    #[cfg(target_os = "macos")]
    surface: IOSurface,
    width: u16,
    height: u16,
    #[cfg(target_os = "macos")]
    frame_rate: i32,
    #[cfg(target_os = "macos")]
    frame_index: i64,
}

impl MacEncoder {
    pub fn open(
        width: u16,
        height: u16,
        frame_rate: u8,
        bitrate_bps: u32,
        high: bool,
    ) -> Result<Self, Error> {
        if width == 0 || height == 0 {
            return Err(Error::InvalidFrame(
                "frame width and height must be non-zero",
            ));
        }
        if !width.is_multiple_of(2) || !height.is_multiple_of(2) {
            return Err(Error::InvalidFrame("H.264 width and height must be even"));
        }
        if frame_rate == 0 {
            return Err(Error::InvalidSetting("frame rate must be non-zero"));
        }
        let bitrate = i32::try_from(bitrate_bps)
            .ok()
            .filter(|bits| *bits > 0)
            .ok_or(Error::InvalidSetting(
                "bitrate does not fit in bits per second",
            ))?;
        #[cfg(target_os = "macos")]
        {
            // High when the session negotiates it; the Linux parser decodes
            // High, and VideoToolbox decodes both. Baseline stays the
            // fallback for older peers.
            let profile = if high {
                ProfileLevel::H264HighAutoLevel
            } else {
                ProfileLevel::H264ConstrainedBaselineAutoLevel
            };
            let session =
                CompressionSession::builder(i32::from(width), i32::from(height), Codec::H264)
                    .with_profile_level(profile)
                    .with_real_time(true)
                    .with_allow_frame_reordering(false)
                    .with_average_bit_rate(bitrate)
                    .with_expected_frame_rate(f64::from(frame_rate))
                    .with_max_keyframe_interval(120)
                    .build()
                    .map_err(|_| Error::Unsupported("VideoToolbox H.264"))?;
            let surface = IOSurface::create(usize::from(width), usize::from(height), BGRA, 4)
                .ok_or(Error::Unsupported("VideoToolbox H.264"))?;
            Ok(Self {
                session,
                surface,
                width,
                height,
                frame_rate: i32::from(frame_rate),
                frame_index: 0,
            })
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = (bitrate, high);
            Err(Error::Unsupported("VideoToolbox H.264"))
        }
    }
}

impl VideoEncoder for MacEncoder {
    fn encode(&mut self, frame: &RawFrame, params: &EncodeParams) -> Result<EncodedSample, Error> {
        if frame.width() != self.width || frame.height() != self.height {
            return Err(Error::InvalidFrame(
                "the frame size does not match the encoder",
            ));
        }
        #[cfg(target_os = "macos")]
        {
            paint(&self.surface, frame)?;
            let time = CMTime::new(self.frame_index, self.frame_rate);
            let properties = FrameProperties::new().with_force_key_frame(params.force_keyframe);
            let encoded = self
                .session
                .encode_with_properties(&self.surface, time, properties)
                .map_err(|_| Error::EncodeRejected("VideoToolbox rejected the frame"))?;
            if encoded.data.is_empty() {
                return Err(Error::EncodeRejected("the encoder produced no frame"));
            }
            let sample = encoded
                .cm_sample_buffer()
                .ok_or(Error::EncodeRejected("the encoder produced no frame"))?;
            let format = sample.format_description().ok_or(Error::EncodeRejected(
                "a keyframe needs SPS, PPS, and an IDR",
            ))?;
            let sets = format
                .video_parameter_sets()
                .map_err(|_| Error::EncodeRejected("a keyframe needs SPS, PPS, and an IDR"))?;
            let nal_length_size = u8::try_from(sets.nal_unit_header_length)
                .ok()
                .filter(|size| matches!(size, 1 | 2 | 4))
                .ok_or(Error::InvalidFrame(
                    "H.264 length prefix must be 1, 2, or 4 bytes",
                ))?;
            let keyframe = contains_idr(&encoded.data, usize::from(nal_length_size));
            let annex_b = to_annex_b(&AvccAccessUnit {
                nal_length_size,
                avcc: &encoded.data,
                parameter_sets: &sets.parameter_sets,
                keyframe,
            })?;
            self.frame_index = self.frame_index.saturating_add(1);
            EncodedSample::annex_b(keyframe, annex_b)
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = params;
            Err(Error::Unsupported("VideoToolbox H.264"))
        }
    }
}

#[cfg(target_os = "macos")]
fn paint(surface: &IOSurface, frame: &RawFrame) -> Result<(), Error> {
    let mut guard = surface
        .lock_read_write()
        .map_err(|_| Error::EncodeRejected("VideoToolbox rejected the frame"))?;
    let stride = guard.bytes_per_row();
    let row = usize::from(frame.width()) * 4;
    let height = usize::from(frame.height());
    if stride < row {
        return Err(Error::InvalidFrame("frame stride is shorter than one row"));
    }
    // The encoder owns this surface. VideoToolbox reads it only after the
    // guard unlocks, and this lock is the only mapping of the bytes.
    let destination = unsafe { guard.as_slice_mut() }
        .ok_or(Error::EncodeRejected("VideoToolbox rejected the frame"))?;
    let needed = stride.saturating_mul(height);
    if destination.len() < needed {
        return Err(Error::InvalidFrame("frame is missing pixels"));
    }
    for y in 0..height {
        let start = y * stride;
        destination[start..start + row].copy_from_slice(frame.row(u16::try_from(y).unwrap_or(0)));
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn contains_idr(avcc: &[u8], length_size: usize) -> bool {
    let mut rest = avcc;
    while rest.len() >= length_size {
        let nal_len = read_len(rest, length_size);
        if nal_len == 0 || rest.len() < length_size + nal_len {
            return false;
        }
        if rest[length_size] & 0x1F == 5 {
            return true;
        }
        rest = &rest[length_size + nal_len..];
    }
    false
}

#[cfg(target_os = "macos")]
fn read_len(data: &[u8], size: usize) -> usize {
    match size {
        1 => usize::from(data[0]),
        2 => usize::from(u16::from_be_bytes([data[0], data[1]])),
        _ => u32::from_be_bytes([data[0], data[1], data[2], data[3]]) as usize,
    }
}
