//! Video codec capabilities with probe-validated registration.
//!
//! A capability bit means a backend initialized and processed a sample, not
//! that a library was linked. Native code probes its encoder and decoder,
//! then registers the resulting [`VideoCodecCaps`]; admission and
//! negotiation read the registered caps. Documented size limits come from
//! the backend's API contract and clamp offers; the probe validates
//! availability, not every advertised pixel count.

use crate::error::Error;
use crate::protocol::{Quality, CODEC_H264, CODEC_H264_HIGH};

/// H.264 encode/decode abilities of this installation. All `false` and zero
/// sizes means no usable video path; admission then reports unsupported
/// instead of a session that can never carry video.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct VideoCodecCaps {
    pub encode_baseline: bool,
    pub encode_high: bool,
    pub decode_baseline: bool,
    pub decode_high: bool,
    /// Documented encoder maximums. Zero means unknown/unsupported.
    pub max_width: u16,
    pub max_height: u16,
    /// True when a hardware backend initialized; false covers software
    /// paths and absent backends alike.
    pub hardware: bool,
    /// Short backend label, e.g. `videotoolbox`, `vaapi`, `mediafoundation`.
    pub device: String,
    /// Full-chroma (4:4:4) path. No shipped backend offers one yet; the bit
    /// exists so negotiation can fall back explicitly when it does.
    pub chroma444: bool,
}

impl VideoCodecCaps {
    /// Full H.264 ability for tests and for native callers whose probe
    /// validated both directions. Sizes are the caller's documented limit.
    pub fn h264_full(max_width: u16, max_height: u16, device: &str) -> Self {
        Self {
            encode_baseline: true,
            encode_high: true,
            decode_baseline: true,
            decode_high: true,
            max_width,
            max_height,
            hardware: true,
            device: capped_label(device),
            chroma444: false,
        }
    }

    /// Encode-only ability, e.g. a headless host that never views.
    pub fn encode_only(max_width: u16, max_height: u16, device: &str) -> Self {
        Self {
            encode_baseline: true,
            encode_high: true,
            decode_baseline: false,
            decode_high: false,
            max_width,
            max_height,
            hardware: true,
            device: capped_label(device),
            chroma444: false,
        }
    }

    /// Decode-only ability, e.g. a viewer-only controller.
    pub fn decode_only(device: &str) -> Self {
        Self {
            decode_baseline: true,
            decode_high: true,
            device: capped_label(device),
            ..Self::default()
        }
    }

    /// Baseline-only ability, e.g. a constrained hardware entrypoint.
    pub fn h264_baseline(max_width: u16, max_height: u16, device: &str) -> Self {
        Self {
            encode_baseline: true,
            encode_high: false,
            decode_baseline: true,
            decode_high: true,
            max_width,
            max_height,
            hardware: true,
            device: capped_label(device),
            chroma444: false,
        }
    }

    pub fn can_encode(&self) -> bool {
        self.encode_baseline || self.encode_high
    }

    pub fn can_decode(&self) -> bool {
        self.decode_baseline || self.decode_high
    }

    pub fn validate(&self) -> Result<(), Error> {
        if self.device.len() > 64 {
            return Err(Error::InvalidSetting("codec device label is too long"));
        }
        if self.can_encode() && (self.max_width == 0 || self.max_height == 0) {
            return Err(Error::InvalidSetting(
                "an encoding capability needs positive size limits",
            ));
        }
        Ok(())
    }
}

/// Agreed codec with the encoder size both sides must respect.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AgreedCodec {
    pub codec: u8,
    pub max_width: u16,
    pub max_height: u16,
}

/// Intersects the offered codec with host and controller abilities. High
/// profile needs it on both ends; anything else falls back to Baseline.
/// Sizes clamp to the host encoder limits. `None` means no common codec.
pub fn negotiate(
    offered: u8,
    host: &VideoCodecCaps,
    controller_decodes_high: bool,
) -> Option<AgreedCodec> {
    let codec = match offered {
        CODEC_H264 if host.encode_baseline => Some(CODEC_H264),
        CODEC_H264_HIGH if host.encode_high && controller_decodes_high => Some(CODEC_H264_HIGH),
        CODEC_H264_HIGH if host.encode_baseline => Some(CODEC_H264),
        _ => None,
    }?;
    Some(AgreedCodec {
        codec,
        max_width: host.max_width.max(1),
        max_height: host.max_height.max(1),
    })
}

/// Clamps an offer to the monitor and the encoder limits. Dimensions never
/// exceed either; a zero monitor side leaves that side to the codec caps.
pub fn clamp_quality(offered: &Quality, monitor: (u16, u16), caps: &VideoCodecCaps) -> Quality {
    let limit = |monitor_side: u16, cap_side: u16| {
        let monitor = if monitor_side > 0 {
            monitor_side
        } else {
            u16::MAX
        };
        let cap = if cap_side > 0 { cap_side } else { u16::MAX };
        monitor.min(cap)
    };
    let width = offered.max_width.min(limit(monitor.0, caps.max_width));
    let height = offered.max_height.min(limit(monitor.1, caps.max_height));
    Quality::new(
        offered.max_bitrate_kbps,
        offered.max_frame_rate,
        width.max(1),
        height.max(1),
    )
    .unwrap_or(*offered)
}

fn capped_label(device: &str) -> String {
    const MAX: usize = 64;
    if device.len() <= MAX {
        return device.into();
    }
    let mut end = MAX;
    while !device.is_char_boundary(end) {
        end -= 1;
    }
    device[..end].into()
}
