// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

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
/// The encoder a host has open for one viewer: a short fixed name for
/// diagnostics, whether it is hardware, and what it reads a picture from;
/// `None` where it cannot tell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EncoderImplementation {
    pub name: &'static str,
    pub hardware: Option<bool>,
    pub input: Option<EncoderInput>,
}
impl EncoderImplementation {
    pub const UNKNOWN: Self = Self {
        name: "unknown",
        hardware: None,
        input: None,
    };
}

/// Where an encoder takes a captured picture from: the cost a host pays
/// before encoding. It can change while the encoder is open, when a GPU
/// step fails and the next picture takes the next way.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EncoderInput {
    /// The picture stays on the GPU that captured it, converted there and
    /// encoded from that texture: nothing is copied to memory.
    GpuTexture,
    /// Converted (and scaled) on the GPU that captured it, then read into
    /// memory for the encoder.
    GpuReadBack,
    /// Read into memory, then uploaded to the encoder's GPU and converted
    /// there.
    GpuUpload,
    /// Read into memory, then scaled and converted on the CPU.
    Cpu,
}

impl EncoderInput {
    /// Short fixed label for diagnostics.
    pub fn label(self) -> &'static str {
        match self {
            Self::GpuTexture => "gpu-texture",
            Self::GpuReadBack => "gpu-readback",
            Self::GpuUpload => "gpu-upload",
            Self::Cpu => "cpu",
        }
    }
}

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
    /// This installation's window shows AV1 4:4:4 still pictures
    /// ([`crate::FrameRole::Still`]). Only the window's own probe sets it
    /// (see [`Self::for_window`]); a viewer asks for stills with it.
    pub chroma444: bool,
}

/// A codec the window's own decoder can show, whether in hardware, and
/// whether it also takes full chroma (4:4:4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WindowDecoder {
    pub codec: crate::VideoCodec,
    pub hardware: bool,
    pub chroma444: bool,
    /// The decoder shows BT.601 pictures correctly and BT.709 ones not, as
    /// Chromium's VA-API path converts as BT.601 whatever a stream says. A
    /// host encodes this codec for the window in BT.601.
    pub bt601: bool,
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
    /// decodes in hardware when it has any, H.264 apart. H.264 stays offered
    /// however the window decodes it: every host encodes it, most in
    /// hardware, and a host offered only codecs it encodes in software
    /// encodes in software. A Linux window, whose hardware H.264 shows the
    /// wrong colours but whose VP9 and AV1 do not, left a Mac host encoding
    /// VP9 or AV1 on its CPU. It shows still pictures when its AV1 decoder
    /// takes 4:4:4. `None` leaves the caps as the native probe registered
    /// them.
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
        if !decoders
            .iter()
            .any(|decoder| decoder.codec == crate::VideoCodec::H264)
        {
            caps.decode_baseline = false;
            caps.decode_high = false;
        }
        for support in &mut caps.additional {
            support.decode &= shown(support.codec);
        }
        caps.chroma444 = decoders
            .iter()
            .any(|decoder| decoder.codec == crate::VideoCodec::Av1 && decoder.chroma444);
        caps
    }

    /// The `bt601` mask of a Hello for these window decoders: the offer bit
    /// of every codec the window shows correctly only as BT.601. Zero
    /// without window decoders.
    pub fn bt601_mask(decoders: Option<&[WindowDecoder]>) -> u8 {
        decoders
            .unwrap_or_default()
            .iter()
            .fold(0, |mask, decoder| {
                mask | if decoder.bt601 {
                    decoder.codec.offer_bit()
                } else {
                    0
                }
            })
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::CODEC_H264;

    #[test]
    fn negotiation_takes_the_best_h264_profile_from_a_decoder_mask_and_refuses_plain_ids() {
        const MASK: u8 = 0x40;
        let full = VideoCodecCaps::h264_full(4096, 4096, "test");
        let agreed = negotiate(MASK | 1, &full).unwrap();
        assert_eq!(agreed.codec, CODEC_H264);
        let agreed = negotiate(MASK | 1 | 2, &full).unwrap();
        assert_eq!(agreed.codec, CODEC_H264_HIGH);
        assert_eq!((agreed.max_width, agreed.max_height), (4096, 4096));
        let baseline = VideoCodecCaps::h264_baseline(1920, 1080, "test");
        let agreed = negotiate(MASK | 1 | 2, &baseline).unwrap();
        assert_eq!(agreed.codec, CODEC_H264);
        assert_eq!((agreed.max_width, agreed.max_height), (1920, 1080));
        // A codec id without the mask bit is not an offer.
        for plain in [CODEC_H264, CODEC_H264_HIGH, 9] {
            assert_eq!(negotiate(plain, &full), None);
        }
        // Cleared probe bits (no-GPU Linux) agree on nothing for either
        // profile, so admission refuses explicitly.
        assert_eq!(negotiate(MASK | 1 | 2, &VideoCodecCaps::default()), None);
        assert_eq!(
            negotiate(MASK | 1 | 2, &VideoCodecCaps::decode_only("test")),
            None
        );
    }

    #[test]
    fn clamp_respects_monitor_codec_limits_and_zero_sides() {
        let offer = Quality::new(2_000, 30, 1920, 1080).unwrap();
        let caps = VideoCodecCaps::h264_full(4096, 4096, "test");
        assert_eq!(clamp_quality(&offer, (800, 600), &caps).max_width, 800);
        assert_eq!(clamp_quality(&offer, (0, 0), &caps), offer);
        let small = VideoCodecCaps::h264_full(640, 480, "test");
        let clamped = clamp_quality(&offer, (4000, 3000), &small);
        assert_eq!((clamped.max_width, clamped.max_height), (640, 480));
        let unbounded = VideoCodecCaps::decode_only("test");
        assert_eq!(clamp_quality(&offer, (800, 600), &unbounded).max_width, 800);
    }

    #[test]
    fn caps_validation_rejects_missing_limits_and_long_labels() {
        assert!(VideoCodecCaps::h264_full(4096, 4096, "test")
            .validate()
            .is_ok());
        assert!(VideoCodecCaps::default().validate().is_ok());
        let mut encode_no_limits = VideoCodecCaps::encode_only(0, 0, "test");
        assert!(encode_no_limits.validate().is_err());
        encode_no_limits.max_width = 640;
        encode_no_limits.max_height = 480;
        assert!(encode_no_limits.validate().is_ok());
        let long = VideoCodecCaps {
            device: "d".repeat(65),
            ..VideoCodecCaps::h264_full(4096, 4096, "test")
        };
        assert!(long.validate().is_err());
        assert!(!VideoCodecCaps::default().can_encode());
        assert!(!VideoCodecCaps::default().can_decode());
        assert!(VideoCodecCaps::decode_only("test").can_decode());
        assert!(!VideoCodecCaps::decode_only("test").can_encode());
    }

    #[test]
    fn auto_negotiates_only_common_probed_codecs_and_prefers_hardware() {
        use crate::{CodecSupport, VideoCodec as C};
        let mut host = VideoCodecCaps::h264_full(4096, 4096, "test");
        host.additional = vec![
            CodecSupport {
                codec: C::H265,
                encode: true,
                decode: true,
                hardware: true,
            },
            CodecSupport {
                codec: C::Vp9,
                encode: true,
                decode: true,
                hardware: false,
            },
            CodecSupport {
                codec: C::Av1,
                encode: true,
                decode: true,
                hardware: false,
            },
        ];
        let mut viewer = host.clone();
        assert_eq!(
            negotiate(viewer.decoder_offer(), &host).unwrap().codec,
            C::H265.wire_id()
        );
        viewer
            .additional
            .iter_mut()
            .find(|c| c.codec == C::H265)
            .unwrap()
            .decode = false;
        assert_eq!(
            negotiate(viewer.decoder_offer(), &host).unwrap().codec,
            CODEC_H264_HIGH
        );
        host.encode_baseline = false;
        host.encode_high = false;
        host.additional[0].encode = false;
        assert_eq!(
            negotiate(viewer.decoder_offer(), &host).unwrap().codec,
            C::Vp9.wire_id()
        );
        viewer.additional.iter_mut().for_each(|c| c.decode = false);
        assert!(negotiate(viewer.decoder_offer(), &host).is_none());
        assert!(negotiate(255, &host).is_none());
        host.additional.push(host.additional[0].clone());
        assert!(host.validate().is_err());
    }

    /// A host with no hardware encoder sends what is cheapest to encode in
    /// real time: its platform H.264 before software HEVC or AV1.
    #[test]
    fn a_software_host_prefers_the_cheapest_codec_to_encode() {
        use crate::{CodecSupport, VideoCodec as C};
        let mut host = VideoCodecCaps::h264_full(4096, 4096, "test");
        host.hardware = false;
        host.additional = [C::H265, C::Vp9, C::Vp8, C::Av1]
            .into_iter()
            .map(|codec| CodecSupport {
                codec,
                encode: true,
                decode: true,
                hardware: false,
            })
            .collect();
        let viewer = host.clone();
        assert_eq!(
            negotiate(viewer.decoder_offer(), &host).unwrap().codec,
            CODEC_H264_HIGH
        );
        host.encode_baseline = false;
        host.encode_high = false;
        assert_eq!(
            negotiate(viewer.decoder_offer(), &host).unwrap().codec,
            C::Vp8.wire_id(),
            "software HEVC was preferred to VP8"
        );
    }

    #[test]
    fn explicit_codec_offer_never_negotiates_another_codec() {
        use crate::{CodecSupport, VideoCodec};
        let mut viewer = VideoCodecCaps::decode_only("test");
        viewer.additional = VideoCodec::ALL
            .into_iter()
            .filter(|c| *c != VideoCodec::H264)
            .map(|codec| CodecSupport {
                codec,
                encode: false,
                decode: true,
                hardware: false,
            })
            .collect();
        let mut host = VideoCodecCaps::h264_full(4096, 4096, "test");
        host.additional = viewer
            .additional
            .iter()
            .map(|c| CodecSupport {
                encode: true,
                ..c.clone()
            })
            .collect();
        for codec in VideoCodec::ALL {
            let offer = viewer.preferred_offer(Some(codec)).unwrap();
            assert_eq!(
                VideoCodec::from_wire_id(negotiate(offer, &host).unwrap().codec),
                Some(codec)
            );
            let mut missing = host.clone();
            if codec == VideoCodec::H264 {
                missing.encode_baseline = false;
                missing.encode_high = false;
            } else {
                missing.additional.retain(|c| c.codec != codec);
            }
            assert!(negotiate(offer, &missing).is_none());
        }
        viewer.decode_baseline = false;
        assert_eq!(
            negotiate(
                viewer.preferred_offer(Some(VideoCodec::H264)).unwrap(),
                &host
            )
            .unwrap()
            .codec,
            CODEC_H264_HIGH
        );
        host.encode_high = false;
        assert!(negotiate(
            viewer.preferred_offer(Some(VideoCodec::H264)).unwrap(),
            &host
        )
        .is_none());
        assert!(VideoCodecCaps::default()
            .preferred_offer(Some(VideoCodec::Vp9))
            .is_err());
    }

    #[test]
    fn offers_keep_to_what_the_window_decodes_in_hardware_when_it_can() {
        use crate::video::CodecSupport;
        use crate::VideoCodec;
        let mut caps = VideoCodecCaps::h264_full(4096, 4096, "test");
        for codec in [VideoCodec::H265, VideoCodec::Vp9] {
            caps.additional.push(CodecSupport {
                codec,
                encode: true,
                decode: true,
                hardware: false,
            });
        }
        let decoder = |codec, hardware| WindowDecoder {
            codec,
            hardware,
            chroma444: false,
            bt601: false,
        };
        assert_eq!(caps.for_window(None), caps, "a window that said nothing");
        // HEVC decodes only in software here, H.264 in hardware: offer H.264.
        let window = [
            decoder(VideoCodec::H264, true),
            decoder(VideoCodec::H265, false),
        ];
        let shown = caps.for_window(Some(&window));
        assert!(shown.supports_decode(VideoCodec::H264));
        assert!(!shown.supports_decode(VideoCodec::H265));
        assert!(
            !shown.supports_decode(VideoCodec::Vp9),
            "the window can't decode VP9"
        );
        assert!(
            shown.supports_encode(VideoCodec::H265),
            "encoding is not the window's"
        );
        // Software only: every codec the window takes.
        let software = [
            decoder(VideoCodec::H264, false),
            decoder(VideoCodec::H265, false),
        ];
        let shown = caps.for_window(Some(&software));
        assert!(shown.supports_decode(VideoCodec::H264) && shown.supports_decode(VideoCodec::H265));
        // Hardware for a codec this installation cannot decode does not count.
        let mismatch = [
            decoder(VideoCodec::Av1, true),
            decoder(VideoCodec::Vp9, false),
        ];
        let shown = caps.for_window(Some(&mismatch));
        assert!(shown.supports_decode(VideoCodec::Vp9));
        assert!(!shown.can_decode() || !shown.supports_decode(VideoCodec::H264));
    }

    /// A Linux window decodes VP9 in hardware but H.264 only in software, its
    /// hardware H.264 showing the wrong colours. H.264 is still offered, so a
    /// host that encodes only H.264 in hardware, a Mac, does not encode VP9 on
    /// its CPU.
    #[test]
    fn h264_stays_offered_when_only_another_codec_decodes_in_hardware() {
        use crate::video::CodecSupport;
        use crate::VideoCodec;
        let mut viewer = VideoCodecCaps::h264_full(4096, 4096, "viewer");
        viewer.additional.push(CodecSupport {
            codec: VideoCodec::Vp9,
            encode: false,
            decode: true,
            hardware: true,
        });
        let window = [
            WindowDecoder {
                codec: VideoCodec::H264,
                hardware: false,
                chroma444: false,
                bt601: false,
            },
            WindowDecoder {
                codec: VideoCodec::Vp9,
                hardware: true,
                chroma444: false,
                bt601: false,
            },
        ];
        let shown = viewer.for_window(Some(&window));
        assert!(shown.supports_decode(VideoCodec::H264));
        assert!(shown.supports_decode(VideoCodec::Vp9));
        // A host encoding H.264 in hardware and VP9 only in software.
        let mut host = VideoCodecCaps::h264_full(4096, 4096, "host");
        host.hardware = true;
        host.additional.push(CodecSupport {
            codec: VideoCodec::Vp9,
            encode: true,
            decode: true,
            hardware: false,
        });
        let agreed = crate::video_caps::negotiate(shown.decoder_offer(), &host).unwrap();
        assert_eq!(agreed.codec, crate::CODEC_H264_HIGH);
    }

    #[test]
    fn a_window_shows_stills_only_when_its_av1_decoder_takes_444() {
        use crate::VideoCodec;
        let caps = VideoCodecCaps::h264_full(4096, 4096, "test");
        let decoder = |codec, chroma444| WindowDecoder {
            codec,
            hardware: false,
            chroma444,
            bt601: false,
        };
        assert!(
            !caps.for_window(None).chroma444,
            "a window that said nothing"
        );
        let av1 = [
            decoder(VideoCodec::H264, false),
            decoder(VideoCodec::Av1, true),
        ];
        assert!(caps.for_window(Some(&av1)).chroma444);
        let main_only = [decoder(VideoCodec::Av1, false)];
        assert!(!caps.for_window(Some(&main_only)).chroma444);
        let other = [decoder(VideoCodec::H265, true)];
        assert!(
            !caps.for_window(Some(&other)).chroma444,
            "stills are AV1 pictures"
        );
    }

    #[test]
    fn a_hello_masks_the_codecs_the_window_shows_as_bt601() {
        use crate::{VideoCodec, YuvMatrix};
        let decoder = |codec, bt601| WindowDecoder {
            codec,
            hardware: true,
            chroma444: false,
            bt601,
        };
        let decoders = [
            decoder(VideoCodec::H264, true),
            decoder(VideoCodec::H265, true),
            decoder(VideoCodec::Vp9, false),
        ];
        let mask = VideoCodecCaps::bt601_mask(Some(&decoders));
        assert_eq!(
            mask,
            VideoCodec::H264.offer_bit() | VideoCodec::H265.offer_bit()
        );
        assert_eq!(VideoCodecCaps::bt601_mask(None), 0);
        assert_eq!(
            YuvMatrix::of_codec(mask, VideoCodec::H265),
            YuvMatrix::Bt601
        );
        assert_eq!(YuvMatrix::of_codec(mask, VideoCodec::Vp9), YuvMatrix::Bt709);
        assert_eq!((YuvMatrix::Bt709.code(), YuvMatrix::Bt601.code()), (1, 6));
    }
}
