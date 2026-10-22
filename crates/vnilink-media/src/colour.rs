//! The one colour signal VniLink's H.264 streams carry, and how a decoder
//! reads another.
//!
//! Every H.264 stream is BT.709 in limited range and says so: the host
//! converts with BT.709 (`bgra_to_nv12`, or VideoToolbox from BGRA), and the
//! SPS of every keyframe carries that video signal in its VUI
//! ([`crate::h264_stream_sps`]). A decoder that converts to BGRA itself
//! follows the SPS ([`crate::h264_video_signal`]), so a stream from
//! elsewhere is shown as it says, and WebCodecs does the same.
//!
//! BT.709 is what VideoToolbox converts BGRA with and what Chromium assumes
//! for an untagged high-definition stream. Limited range is what H.264
//! decoders, Media Foundation's included, handle most consistently.

/// A stream's colour, as H.264 VUI and ITU-T H.273 code it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VideoSignal {
    pub primaries: u8,
    pub transfer: u8,
    pub matrix: u8,
    pub full_range: bool,
}

/// BT.709 primaries, transfer and matrix, in limited range: what every
/// VniLink H.264 stream carries.
pub const STREAM_SIGNAL: VideoSignal = VideoSignal {
    primaries: 1,
    transfer: 1,
    matrix: 1,
    full_range: false,
};

/// The matrix that turns Y'CbCr back into R'G'B'.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum YuvMatrix {
    Bt601,
    Bt709,
}

impl VideoSignal {
    /// BT.601 for the matrix codes that mean it (5, 6: BT.470BG, SMPTE
    /// 170M); BT.709 for 1 and for anything unspecified or not handled,
    /// as Chromium does for high-definition video.
    pub fn yuv_matrix(&self) -> YuvMatrix {
        match self.matrix {
            5 | 6 => YuvMatrix::Bt601,
            _ => YuvMatrix::Bt709,
        }
    }
}

/// Fixed-point (x256) Y'CbCr → R'G'B' for one matrix and range:
/// `luma` scales Y' (after the 16 offset in limited range), then Cr→R,
/// Cb→G, Cr→G and Cb→B.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ToRgb {
    pub luma: i32,
    pub luma_offset: i32,
    pub cr_red: i32,
    pub cb_green: i32,
    pub cr_green: i32,
    pub cb_blue: i32,
}

impl ToRgb {
    pub(crate) fn of(signal: VideoSignal) -> Self {
        let (luma, luma_offset) = if signal.full_range {
            (256, 0)
        } else {
            (298, 16)
        };
        let [cr_red, cb_green, cr_green, cb_blue] = match (signal.yuv_matrix(), signal.full_range) {
            (YuvMatrix::Bt709, false) => [459, -55, -136, 541],
            (YuvMatrix::Bt709, true) => [403, -48, -120, 475],
            (YuvMatrix::Bt601, false) => [409, -100, -208, 516],
            (YuvMatrix::Bt601, true) => [359, -88, -183, 454],
        };
        Self {
            luma,
            luma_offset,
            cr_red,
            cb_green,
            cr_green,
            cb_blue,
        }
    }
}
