//! Stable codec identities, shared by negotiation, packets and native adapters.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum VideoCodec {
    H264,
    H265,
    Vp8,
    Vp9,
    Av1,
}
impl VideoCodec {
    pub const ALL: [Self; 5] = [Self::H265, Self::H264, Self::Av1, Self::Vp9, Self::Vp8];
    pub fn wire_id(self) -> u8 {
        match self {
            Self::H264 => 1,
            Self::H265 => 3,
            Self::Vp8 => 4,
            Self::Vp9 => 5,
            Self::Av1 => 6,
        }
    }
    pub fn from_wire_id(id: u8) -> Option<Self> {
        match id {
            1 | 2 => Some(Self::H264),
            3 => Some(Self::H265),
            4 => Some(Self::Vp8),
            5 => Some(Self::Vp9),
            6 => Some(Self::Av1),
            _ => None,
        }
    }
    pub fn offer_bit(self) -> u8 {
        match self {
            Self::H264 => 1,
            Self::H265 => 4,
            Self::Vp8 => 8,
            Self::Vp9 => 16,
            Self::Av1 => 32,
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::H264 => "h264",
            Self::H265 => "h265",
            Self::Vp8 => "vp8",
            Self::Vp9 => "vp9",
            Self::Av1 => "av1",
        }
    }
}

/// The matrix a stream converts RGB to Y'CbCr with, and declares. A host
/// encodes each session in the matrix its viewer's decoder shows correctly:
/// BT.709 for every decoder that follows the stream, BT.601 for Chromium's
/// VA-API path, which converts as BT.601 whatever the stream says.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum YuvMatrix {
    #[default]
    Bt709,
    Bt601,
}
impl YuvMatrix {
    /// The matrix code H.264 VUI and ITU-T H.273 carry: 1 for BT.709, 6
    /// (SMPTE 170M) for BT.601.
    pub fn code(self) -> u8 {
        match self {
            Self::Bt709 => 1,
            Self::Bt601 => 6,
        }
    }

    /// The matrix a Hello's `bt601` mask gives `codec`.
    pub fn of_codec(bt601: u8, codec: VideoCodec) -> Self {
        if bt601 & codec.offer_bit() != 0 {
            Self::Bt601
        } else {
            Self::Bt709
        }
    }
}

/// Probe-validated abilities. Hardware describes this codec's encoder, not the
/// entire installation; software fallback remains independently observable.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CodecSupport {
    pub codec: VideoCodec,
    pub encode: bool,
    pub decode: bool,
    pub hardware: bool,
}
