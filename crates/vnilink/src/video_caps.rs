//! Video codec capabilities with probe-validated registration.
//!
//! A capability bit means a backend initialized and processed a sample, not
//! that a library was linked. Native code probes its encoder and decoder,
//! then registers the resulting [`VideoCodecCaps`]; admission and
//! negotiation read the registered caps. Documented size limits come from
//! the backend's API contract and clamp offers; the probe validates
//! availability, not every advertised pixel count.

use crate::error::Error;
use crate::protocol::{Quality, CODEC_H264_HIGH};

/// H.264 encode/decode abilities of this installation. All `false` and zero
/// sizes means no usable video path; admission then reports unsupported
/// instead of a session that can never carry video.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VideoCodecCaps {
    #[serde(default)]
    pub additional: Vec<crate::video::CodecSupport>,
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

/// A codec the window's own decoder can show, and whether in hardware.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WindowDecoder {
    pub codec: crate::VideoCodec,
    pub hardware: bool,
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
            additional: Vec::new(),
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
            additional: Vec::new(),
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
            additional: Vec::new(),
        }
    }

    /// The decode side this installation can show in its window: only the
    /// codecs the window's decoder takes, and of those only the ones it
    /// decodes in hardware when it has any. `None` leaves the caps as the
    /// native probe registered them.
    pub fn for_window(&self, decoders: Option<&[WindowDecoder]>) -> Self {
        let Some(decoders) = decoders else {
            return self.clone();
        };
        let hardware = decoders
            .iter()
            .any(|decoder| decoder.hardware && self.supports_decode(decoder.codec));
        let shown = |codec: crate::VideoCodec| {
            decoders
                .iter()
                .any(|decoder| decoder.codec == codec && (decoder.hardware || !hardware))
        };
        let mut caps = self.clone();
        if !shown(crate::VideoCodec::H264) {
            caps.decode_baseline = false;
            caps.decode_high = false;
        }
        for support in &mut caps.additional {
            support.decode &= shown(support.codec);
        }
        caps
    }

    /// Offers only the selected codec; unavailable explicit choices never fall back.
    pub fn preferred_offer(&self, preference: Option<crate::VideoCodec>) -> Result<u8, Error> {
        match preference {
            None if self.can_decode() => Ok(self.decoder_offer()),
            Some(crate::VideoCodec::H264) if self.supports_decode(crate::VideoCodec::H264) => {
                Ok(0x40 | u8::from(self.decode_baseline) | if self.decode_high { 2 } else { 0 })
            }
            Some(codec) if self.supports_decode(codec) => Ok(0x40 | codec.offer_bit()),
            _ => Err(Error::Unsupported("selected video decoder")),
        }
    }

    pub fn decoder_offer(&self) -> u8 {
        0x40 | u8::from(self.decode_baseline)
            | if self.decode_high { 2 } else { 0 }
            | self
                .additional
                .iter()
                .filter(|c| c.decode)
                .fold(0, |bits, c| bits | c.codec.offer_bit())
    }
    pub fn supports_encode(&self, codec: crate::video::VideoCodec) -> bool {
        if codec == crate::video::VideoCodec::H264 {
            self.encode_baseline || self.encode_high
        } else {
            self.additional.iter().any(|c| c.codec == codec && c.encode)
        }
    }
    pub fn supports_decode(&self, codec: crate::video::VideoCodec) -> bool {
        if codec == crate::video::VideoCodec::H264 {
            self.decode_baseline || self.decode_high
        } else {
            self.additional.iter().any(|c| c.codec == codec && c.decode)
        }
    }
    pub fn can_encode(&self) -> bool {
        self.encode_baseline || self.encode_high || self.additional.iter().any(|c| c.encode)
    }

    pub fn can_decode(&self) -> bool {
        self.decode_baseline || self.decode_high || self.additional.iter().any(|c| c.decode)
    }

    pub fn validate(&self) -> Result<(), Error> {
        if self.additional.len() > 4
            || self.additional.iter().enumerate().any(|(i, c)| {
                c.codec == crate::video::VideoCodec::H264
                    || self.additional[..i].iter().any(|p| p.codec == c.codec)
            })
        {
            return Err(Error::InvalidSetting(
                "duplicate or invalid additional codec",
            ));
        }
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

/// Intersects the controller's decoder mask (`0x40` plus one bit per codec,
/// with bit 2 for H.264 High) with what the host encodes. High profile needs
/// it on both ends. Sizes clamp to the host encoder limits. `None` means the
/// offer is not a mask or there is no common codec.
pub fn negotiate(offered: u8, host: &VideoCodecCaps) -> Option<AgreedCodec> {
    if offered & 0x80 != 0 || offered & 0x40 == 0 {
        return None;
    }
    let codec = {
        let mut candidates = crate::video::VideoCodec::ALL.to_vec();
        // Prefer validated native hardware, the most efficient codec first.
        // In software, prefer the cheapest to encode in real time: the
        // platform's H.264, then VP8 and VP9, with software HEVC and AV1
        // last. Choice is per Desktop owner.
        candidates.sort_by_key(|codec| {
            use crate::video::VideoCodec as C;
            let hardware = if *codec == C::H264 {
                host.hardware
            } else {
                host.additional
                    .iter()
                    .any(|c| c.codec == *codec && c.hardware)
            };
            let order = if hardware {
                match codec {
                    C::H265 => 0,
                    C::H264 => 1,
                    C::Vp9 => 2,
                    C::Vp8 => 3,
                    C::Av1 => 4,
                }
            } else {
                match codec {
                    C::H264 => 0,
                    C::Vp8 => 1,
                    C::Vp9 => 2,
                    C::H265 => 3,
                    C::Av1 => 4,
                }
            };
            (!hardware, order)
        });
        let selected = candidates.into_iter().find(|codec| {
            if *codec == crate::VideoCodec::H264 {
                (host.encode_baseline && offered & 1 != 0) || (host.encode_high && offered & 2 != 0)
            } else {
                host.supports_encode(*codec) && offered & codec.offer_bit() != 0
            }
        })?;
        if selected == crate::video::VideoCodec::H264 && offered & 2 != 0 && host.encode_high {
            CODEC_H264_HIGH
        } else {
            selected.wire_id()
        }
    };
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
