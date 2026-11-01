// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! Media Foundation H.264 capability probe. Opening a tiny encoder proves
//! the platform transform exists; a failure registers cleared bits, never
//! assumed ones. Decode is reported alongside encode: both ride the same
//! Media Foundation component, and the decoder path fails its owner
//! explicitly if a stream ever defeats it.
// trace: records every ignored failure

use crate::encode::WindowsEncoder;

pub fn probe_video_codecs() -> vnilink::VideoCodecCaps {
    #[cfg(not(windows))]
    {
        // Keeps the import used where the probe does not run.
        let _probe = &WindowsEncoder::open;
        vnilink::VideoCodecCaps::default()
    }
    #[cfg(windows)]
    {
        let encoder = WindowsEncoder::open(64, 64, 30, 500_000, vnilink::YuvMatrix::Bt709).ok();
        let encode = encoder.is_some();
        // High only where an encoder opens it: a transform that refuses the
        // profile leaves the stream baseline.
        let high =
            WindowsEncoder::open_profile(64, 64, 30, 500_000, true, vnilink::YuvMatrix::Bt709)
                .is_ok();
        vnilink::VideoCodecCaps {
            encode_baseline: encode,
            encode_high: high,
            decode_baseline: encode,
            decode_high: encode,
            max_width: 4096,
            max_height: 4096,
            hardware: encoder.as_ref().is_some_and(WindowsEncoder::hardware),
            device: "mediafoundation".into(),
            chroma444: false,
            additional: hardware_codecs(),
        }
    }
}

/// HEVC and AV1 where a hardware encoder's output decodes, in the right
/// colour, with the shared software decoder: a transform whose bitstream does
/// not is never offered over software.
#[cfg(windows)]
fn hardware_codecs() -> Vec<vnilink::CodecSupport> {
    use vnilink::VideoCodec;
    use vnilink_media::{decodes, EncodeParams, VideoEncoder, PROBE_SIDE};
    [VideoCodec::H265, VideoCodec::Av1]
        .into_iter()
        .filter(|codec| {
            let Ok(mut encoder) = WindowsEncoder::open_hardware_codec(
                *codec,
                PROBE_SIDE,
                PROBE_SIDE,
                30,
                1_000_000,
                vnilink::YuvMatrix::Bt709,
                None,
            ) else {
                return false;
            };
            decodes(*codec, |picture, key| {
                encoder.encode(
                    picture,
                    &EncodeParams {
                        force_keyframe: key,
                        recovery: false,
                        bitrate_bps: 1_000_000,
                        frame_rate: 30,
                    },
                )
            })
        })
        .map(|codec| vnilink::CodecSupport {
            codec,
            encode: true,
            decode: false,
            hardware: true,
        })
        .collect()
}
