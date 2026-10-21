//! VideoToolbox H.264 encode of BGRA pictures.
//!
//! The session emits length-prefixed NALs. SPS and PPS stay in the format
//! description, so a keyframe is rewritten as Annex-B with those sets in front.
//! This encoder does not capture the screen. Callers supply the picture.
//! Without VideoToolbox H.264 this returns unsupported.
//!
//! A recovery keyframe is encoded under a quantiser floor that
//! [`RecoveryQp`] moves until it fits a few pictures' bytes. VideoToolbox
//! takes no byte limit for one picture: on this kind of session, neither the
//! data-rate limit nor `MaxAllowedFrameQP` shrank a forced keyframe, and
//! `MinAllowedFrameQP` did. The floor is a session property, set for that
//! one picture and cleared after it; `encode_with_properties` waits for the
//! picture, so no other picture is encoded under it.

#[cfg(target_os = "macos")]
use apple_cf::cm::CMTime;
#[cfg(target_os = "macos")]
use apple_cf::iosurface::IOSurface;
#[cfg(target_os = "macos")]
use videotoolbox::compression::{CompressionSession, FrameProperties, ProfileLevel};
#[cfg(target_os = "macos")]
use videotoolbox::session::Codec;
#[cfg(target_os = "macos")]
use vnilink_media::RecoveryQp;
use vnilink_media::{
    to_annex_b, AvccAccessUnit, EncodeParams, EncodedSample, Error, RawFrame, VideoEncoder,
};

#[cfg(target_os = "macos")]
const BGRA: u32 = 0x4247_5241;

pub struct MacEncoder {
    codec: vnilink::VideoCodec,
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
    #[cfg(target_os = "macos")]
    recovery_qp: RecoveryQp,
}

impl MacEncoder {
    /// Inspect the actual session rather than assuming native means hardware.
    pub fn hardware(&self) -> bool {
        #[cfg(target_os = "macos")]
        unsafe {
            self.session.copy_property(videotoolbox::ffi::kVTCompressionPropertyKey_UsingHardwareAcceleratedVideoEncoder)
                .ok().flatten().is_some_and(|value| {
                    value.type_id() as u64 == apple_cf::raw::CFBooleanGetTypeID()
                        && apple_cf::raw::CFBooleanGetValue(value.as_ptr().cast()) != 0
                })
        }
        #[cfg(not(target_os = "macos"))]
        {
            false
        }
    }
    pub fn open(
        width: u16,
        height: u16,
        frame_rate: u8,
        bitrate_bps: u32,
        high: bool,
    ) -> Result<Self, Error> {
        Self::open_codec(
            width,
            height,
            frame_rate,
            bitrate_bps,
            vnilink::VideoCodec::H264,
            high,
        )
    }
    pub fn open_codec(
        width: u16,
        height: u16,
        frame_rate: u8,
        bitrate_bps: u32,
        codec: vnilink::VideoCodec,
        high: bool,
    ) -> Result<Self, Error> {
        if !matches!(codec, vnilink::VideoCodec::H264 | vnilink::VideoCodec::H265) {
            return Err(Error::Unsupported("VideoToolbox encoder codec"));
        }
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
            let profile = if codec == vnilink::VideoCodec::H265 {
                ProfileLevel::HEVCMainAutoLevel
            } else if high {
                ProfileLevel::H264HighAutoLevel
            } else {
                ProfileLevel::H264ConstrainedBaselineAutoLevel
            };
            let builder = || {
                CompressionSession::builder(
                    i32::from(width),
                    i32::from(height),
                    if codec == vnilink::VideoCodec::H265 {
                        Codec::HEVC
                    } else {
                        Codec::H264
                    },
                )
                .with_profile_level(profile)
                .with_real_time(true)
                .with_allow_frame_reordering(false)
                .with_average_bit_rate(bitrate)
                .with_expected_frame_rate(f64::from(frame_rate))
            };
            // Low-latency rate control encodes each picture as it comes,
            // one in and one out, and keeps sizes near the target; keyframes
            // then come only when asked for, as the viewer does after a loss.
            // An encoder that refuses it keeps the ordinary rate control with
            // a keyframe every 120 pictures.
            let session = builder()
                .with_low_latency_rate_control(true)
                .build()
                .or_else(|_| builder().with_max_keyframe_interval(120).build())
                .map_err(|_| Error::Unsupported("VideoToolbox H.264"))?;
            low_latency(&session);
            set_rate(&session, bitrate, frame_rate);
            let surface = IOSurface::create(usize::from(width), usize::from(height), BGRA, 4)
                .ok_or(Error::Unsupported("VideoToolbox H.264"))?;
            Ok(Self {
                codec,
                session,
                surface,
                width,
                height,
                frame_rate: i32::from(frame_rate),
                frame_index: 0,
                recovery_qp: RecoveryQp::default(),
            })
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = (bitrate, high);
            Err(Error::Unsupported("VideoToolbox H.264"))
        }
    }
}

/// Sets one property on a running session. VideoToolbox decides which it
/// takes from a given encoder; refusal is reported, not an error.
#[cfg(target_os = "macos")]
fn set(
    session: &CompressionSession,
    key: videotoolbox::ffi::CFStringRef,
    value: &dyn apple_cf::cf::AsCFType,
) -> bool {
    // SAFETY: `key` is one of VideoToolbox's exported property keys, and
    // `value` is a live Core Foundation object of the type that key takes
    // (a number, an array of numbers or a string), borrowed for the call;
    // VideoToolbox retains whatever it keeps.
    unsafe { session.set_property(key, value.as_ptr().cast_const()) }.is_ok()
}

/// The target bitrate and frame rate, with peaks held to 1.5 times the
/// average over any second, so a keyframe cannot overrun the link. Returns
/// whether the encoder took the bitrate.
#[cfg(target_os = "macos")]
fn set_rate(session: &CompressionSession, bitrate: i32, frame_rate: u8) -> bool {
    use apple_cf::cf::{CFArray, CFNumber};
    use videotoolbox::ffi;
    // SAFETY (all three reads): VideoToolbox's exported constant keys,
    // initialised by the framework before any session exists.
    let (average, limits, expected) = unsafe {
        (
            ffi::kVTCompressionPropertyKey_AverageBitRate,
            ffi::kVTCompressionPropertyKey_DataRateLimits,
            ffi::kVTCompressionPropertyKey_ExpectedFrameRate,
        )
    };
    let took = set(session, average, &CFNumber::from_i64(i64::from(bitrate)));
    let peak_bytes = CFNumber::from_i64(i64::from(bitrate) * 3 / 16);
    let second = CFNumber::from_f64(1.0);
    set(
        session,
        limits,
        &CFArray::from_values(&[&peak_bytes, &second]),
    );
    set(
        session,
        expected,
        &CFNumber::from_f64(f64::from(frame_rate)),
    );
    took
}

/// The lowest QP the session may use; [`NO_QP_FLOOR`] clears it. Returns
/// whether the encoder took it.
#[cfg(target_os = "macos")]
fn set_qp_floor(session: &CompressionSession, qp: i64) -> bool {
    use apple_cf::cf::CFNumber;
    // SAFETY: VideoToolbox's exported constant key, as in `set_rate`.
    let key = unsafe { videotoolbox::ffi::kVTCompressionPropertyKey_MinAllowedFrameQP };
    set(session, key, &CFNumber::from_i64(qp))
}

/// The value a session reports for `MinAllowedFrameQP` before one is set.
#[cfg(target_os = "macos")]
const NO_QP_FLOOR: i64 = -1;

/// No frame held back for later output, and colour marked as BT.709, the
/// matrix VideoToolbox converts the BGRA pictures with, so decoders do not
/// guess.
#[cfg(target_os = "macos")]
fn low_latency(session: &CompressionSession) {
    use apple_cf::cf::{CFNumber, CFString};
    use videotoolbox::ffi;
    // SAFETY: VideoToolbox's exported constant keys, as in `set_rate`.
    let (delay, primaries, transfer, matrix) = unsafe {
        (
            ffi::kVTCompressionPropertyKey_MaxFrameDelayCount,
            ffi::kVTCompressionPropertyKey_ColorPrimaries,
            ffi::kVTCompressionPropertyKey_TransferFunction,
            ffi::kVTCompressionPropertyKey_YCbCrMatrix,
        )
    };
    set(session, delay, &CFNumber::from_i64(0));
    // The value of kCVImageBuffer…_ITU_R_709_2 for all three.
    let bt709 = CFString::new("ITU_R_709_2");
    set(session, primaries, &bt709);
    set(session, transfer, &bt709);
    set(session, matrix, &bt709);
}

impl VideoEncoder for MacEncoder {
    fn retune(&mut self, bitrate_bps: u32, frame_rate: u8) -> Result<bool, Error> {
        #[cfg(target_os = "macos")]
        {
            let Some(bitrate) = i32::try_from(bitrate_bps).ok().filter(|bits| *bits > 0) else {
                return Ok(false);
            };
            Ok(frame_rate > 0 && set_rate(&self.session, bitrate, frame_rate))
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = (bitrate_bps, frame_rate);
            Ok(false)
        }
    }

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
            let floored = params.force_keyframe
                && params.recovery
                && set_qp_floor(&self.session, i64::from(self.recovery_qp.floor()));
            let encoded = self
                .session
                .encode_with_properties(&self.surface, time, properties);
            if floored {
                set_qp_floor(&self.session, NO_QP_FLOOR);
            }
            let encoded =
                encoded.map_err(|_| Error::EncodeRejected("VideoToolbox rejected the frame"))?;
            if encoded.data.is_empty() {
                return Err(Error::EncodeRejected("the encoder produced no frame"));
            }
            if floored {
                self.recovery_qp.observe(encoded.data.len(), params);
            } else if params.force_keyframe {
                self.recovery_qp.observe_full(encoded.data.len(), params);
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
            if self.codec == vnilink::VideoCodec::H265 {
                self.frame_index = self.frame_index.saturating_add(1);
                return vnilink_media::hevc_annex_b(
                    &encoded.data,
                    nal_length_size,
                    &sets.parameter_sets,
                );
            }
            let keyframe = contains_idr(&encoded.data, usize::from(nal_length_size));
            // VideoToolbox writes no VUI, so decoders may hold pictures back
            // for reordering that never happens; say so in the SPS.
            let parameter_sets: Vec<Vec<u8>> = sets
                .parameter_sets
                .iter()
                .map(|set| {
                    set.first()
                        .filter(|header| *header & 0x1F == 7)
                        .and_then(|_| vnilink_media::h264_low_delay_sps(set))
                        .unwrap_or_else(|| set.clone())
                })
                .collect();
            let annex_b = to_annex_b(&AvccAccessUnit {
                nal_length_size,
                avcc: &encoded.data,
                parameter_sets: &parameter_sets,
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

/// One negotiated encoder per Desktop owner; software is never shared across peers.
pub enum MacVideoEncoder {
    Native(MacEncoder),
    Software(vnilink_media::SoftwareEncoder),
}
impl MacVideoEncoder {
    pub fn open(width: u16, height: u16, fps: u8, bitrate: u32, codec: u8) -> Result<Self, Error> {
        let kind = vnilink::VideoCodec::from_wire_id(codec)
            .ok_or(Error::Unsupported("negotiated codec"))?;
        if matches!(kind, vnilink::VideoCodec::H264 | vnilink::VideoCodec::H265) {
            match MacEncoder::open_codec(
                width,
                height,
                fps,
                bitrate,
                kind,
                codec == vnilink::CODEC_H264_HIGH,
            ) {
                Ok(encoder) => Ok(Self::Native(encoder)),
                Err(_) if kind == vnilink::VideoCodec::H265 => Ok(Self::Software(
                    vnilink_media::SoftwareEncoder::open(kind, width, height, fps, bitrate)?,
                )),
                Err(error) => Err(error),
            }
        } else {
            Ok(Self::Software(vnilink_media::SoftwareEncoder::open(
                kind, width, height, fps, bitrate,
            )?))
        }
    }
}
impl VideoEncoder for MacVideoEncoder {
    fn encode(&mut self, frame: &RawFrame, params: &EncodeParams) -> Result<EncodedSample, Error> {
        match self {
            Self::Native(encoder) => encoder.encode(frame, params),
            Self::Software(encoder) => encoder.encode(frame, params),
        }
    }

    fn retune(&mut self, bitrate_bps: u32, frame_rate: u8) -> Result<bool, Error> {
        match self {
            Self::Native(encoder) => encoder.retune(bitrate_bps, frame_rate),
            Self::Software(encoder) => encoder.retune(bitrate_bps, frame_rate),
        }
    }
}
/// Real synthetic roundtrip without screen/camera capture or OS permission changes.
#[cfg(target_os = "macos")]
pub fn probe_hevc() -> vnilink::CodecSupport {
    let result = (|| -> Result<bool, Error> {
        use vnilink_media::VideoDecoder;
        let raw = RawFrame::bgra(64, 64, 0, [32u8, 64, 128, 255].repeat(4096))?;
        let mut encoder =
            MacEncoder::open_codec(64, 64, 30, 500_000, vnilink::VideoCodec::H265, false)?;
        let sample = encoder.encode(
            &raw,
            &EncodeParams {
                force_keyframe: true,
                recovery: false,
                bitrate_bps: 500_000,
                frame_rate: 30,
            },
        )?;
        let frame = vnilink::EncodedFrame::new(
            vnilink::VideoCodec::H265,
            sample.keyframe(),
            0,
            sample.bytes().to_vec(),
        )
        .map_err(|_| Error::InvalidFrame("HEVC probe packet"))?;
        let decoded = crate::MacDecoder::new().decode(&frame)?;
        if decoded.width() != 64 || decoded.height() != 64 {
            return Err(Error::DecodeRejected("HEVC probe geometry"));
        }
        Ok(encoder.hardware())
    })();
    vnilink::CodecSupport {
        codec: vnilink::VideoCodec::H265,
        encode: result.is_ok(),
        decode: result.is_ok(),
        hardware: result.unwrap_or(false),
    }
}
