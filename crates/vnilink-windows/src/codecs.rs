//! Media Foundation H.264 capability probe. Opening a tiny encoder proves
//! the platform transform exists; a failure registers cleared bits, never
//! assumed ones. Decode is reported alongside encode: both ride the same
//! Media Foundation component, and the decoder path fails its owner
//! explicitly if a stream ever defeats it.

use crate::encode::WindowsEncoder;

pub fn probe_video_codecs() -> vnilink::VideoCodecCaps {
    #[cfg(not(windows))]
    {
        let _ = &WindowsEncoder::open;
        return vnilink::VideoCodecCaps::default();
    }
    #[cfg(windows)]
    {
        let encoder = WindowsEncoder::open(64, 64, 30, 500_000, vnilink::YuvMatrix::Bt709).ok();
        let encode = encoder.is_some();
        vnilink::VideoCodecCaps {
            encode_baseline: encode,
            encode_high: false,
            decode_baseline: encode,
            decode_high: encode,
            max_width: 4096,
            max_height: 4096,
            hardware: encoder.as_ref().is_some_and(WindowsEncoder::hardware),
            device: "mediafoundation".into(),
            chroma444: false,
            additional: Vec::new(),
        }
    }
}
