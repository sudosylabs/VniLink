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
//!
//! The software codecs follow the same rule: they convert to BT.709, and
//! H.265, AV1 and VP9 declare it. VP8 cannot declare a colour; Chromium
//! shows an undeclared VP8 picture as BT.709 too (Electron 44, Chromium 152).
//! Their decoder shows each picture in the colour its stream declares
//! ([`declared_signal`]).

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

/// The colour a decoder shows a picture in: the matrix and range its stream
/// declares, or [`STREAM_SIGNAL`] when the matrix is unspecified (H.273
/// code 2), not handled, or cannot be declared, as in VP8.
pub(crate) fn declared_signal(matrix: Option<u8>, full_range: bool) -> VideoSignal {
    match matrix {
        Some(matrix @ (1 | 5 | 6)) => VideoSignal {
            matrix,
            full_range,
            ..STREAM_SIGNAL
        },
        _ => STREAM_SIGNAL,
    }
}

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

/// Fixed-point (x256) R'G'B' → limited-range Y'CbCr for one matrix. Each
/// chroma row sums to 0 so grey stays at 128; with eight-bit RGB the
/// results stay within 16..=235 and 16..=240 without clamping.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ToYuv {
    luma: [i32; 3],
    cb: [i32; 3],
    cr: [i32; 3],
}

impl ToYuv {
    pub(crate) fn of(matrix: YuvMatrix) -> Self {
        match matrix {
            YuvMatrix::Bt709 => Self {
                luma: [47, 157, 16],
                cb: [-26, -86, 112],
                cr: [112, -102, -10],
            },
            YuvMatrix::Bt601 => Self {
                luma: [66, 129, 25],
                cb: [-38, -74, 112],
                cr: [112, -94, -18],
            },
        }
    }

    pub(crate) fn convert(&self, red: u8, green: u8, blue: u8) -> (u8, u8, u8) {
        let rgb = [i32::from(red), i32::from(green), i32::from(blue)];
        let dot = |row: [i32; 3]| (row[0] * rgb[0] + row[1] * rgb[1] + row[2] * rgb[2] + 128) >> 8;
        (
            (dot(self.luma) + 16) as u8,
            (dot(self.cb) + 128) as u8,
            (dot(self.cr) + 128) as u8,
        )
    }
}
