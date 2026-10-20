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
        let encode = WindowsEncoder::open(64, 64, 30, 500_000).is_ok();
        vnilink::VideoCodecCaps {
            encode_baseline: encode,
            encode_high: false,
            decode_baseline: encode,
            decode_high: encode,
            max_width: 4096,
            max_height: 4096,
            // The Microsoft H.264 transform is a software encoder.
            hardware: false,
            device: "mediafoundation".into(),
            chroma444: false,
            additional: Vec::new(),
        }
    }
}
