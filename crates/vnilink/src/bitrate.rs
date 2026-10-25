//! The bitrate a picture can use, and what holds the stream below it.
//!
//! The viewer's bitrate setting is a limit, not a rate. The rate the stream
//! asks for follows the picture (its size, its frame rate and the codec) and
//! the link; [`crate::adaptation`] moves it between a start and the lowest of
//! the bounds here.
use crate::VideoCodec;

/// The rate a stream starts at and a relayed path is held to, in kb/s.
/// Public relays are shared and rate limited: a stream through one is kept
/// at what a lone viewer can safely use, and a direct path finds the rest.
pub(crate) const START_KBPS: u32 = 4_000;
pub(crate) const RELAY_KBPS: u32 = 4_000;

/// The lowest rate adaptation sets.
pub(crate) const FLOOR_KBPS: u32 = 500;

/// Bits a picture can usefully spend per pixel, in thousandths, for each
/// codec. This is a ceiling, not a target: an encoder spends less on a
/// screen that changes little, so only complex, fast content reaches it.
///
/// H.264 is the reference. 0.16 bits per pixel is 20 Mb/s at 1920x1080 and
/// 60 fps, the default of Sunshine and in line with what Moonlight suggests
/// for that size. HEVC needs about a third fewer bits than H.264 for the
/// same quality and AV1 about two fifths fewer, as encoder comparisons
/// commonly report; VP9 sits between H.264 and HEVC and VP8 above H.264.
/// The ratios come from those comparisons and are not measured in this
/// project. An encoder that does worse only spends more of the rate on the
/// same picture, which the link bounds in turn.
fn milli_bits_per_pixel(codec: VideoCodec) -> u64 {
    match codec {
        VideoCodec::H264 => 160,
        VideoCodec::H265 => 110,
        VideoCodec::Vp9 => 125,
        VideoCodec::Av1 => 95,
        VideoCodec::Vp8 => 190,
    }
}

/// The size the host encodes: the viewer's limit, fitted into the monitor's
/// proportions and never above it. An unknown monitor leaves the limit.
pub(crate) fn encode_size(limit: (u16, u16), monitor: (u16, u16)) -> (u32, u32) {
    let (width, height) = (u32::from(limit.0.max(1)), u32::from(limit.1.max(1)));
    let (monitor_width, monitor_height) = (u32::from(monitor.0), u32::from(monitor.1));
    if monitor_width == 0 || monitor_height == 0 {
        return (width, height);
    }
    // The largest scale of the monitor inside the limit, at most 1.
    if width >= monitor_width && height >= monitor_height {
        return (monitor_width, monitor_height);
    }
    if u64::from(width) * u64::from(monitor_height) <= u64::from(height) * u64::from(monitor_width)
    {
        (
            width,
            (u64::from(monitor_height) * u64::from(width) / u64::from(monitor_width)).max(1) as u32,
        )
    } else {
        (
            (u64::from(monitor_width) * u64::from(height) / u64::from(monitor_height)).max(1)
                as u32,
            height,
        )
    }
}

/// The most a picture of this size and frame rate can use, in kb/s, within
/// what a [`crate::Quality`] holds.
pub(crate) fn picture_kbps(codec: VideoCodec, size: (u32, u32), frame_rate: u8) -> u32 {
    let pixels_a_second = u64::from(size.0) * u64::from(size.1) * u64::from(frame_rate);
    let kbps = pixels_a_second * milli_bits_per_pixel(codec) / 1_000_000;
    kbps.clamp(u64::from(FLOOR_KBPS), 50_000) as u32
}

/// What most holds the bitrate where it is, as the status line says it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RateLimit {
    /// Nothing: the picture needs no more, or is still, or is on its way up.
    #[default]
    None,
    /// The viewer's own setting.
    Setting,
    /// The relayed path.
    Relay,
    /// The link, measured or just proven too small.
    Network,
    /// The host's encoder is near its budget.
    Encoder,
    /// This computer's decoder is near its budget.
    Receiver,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn a_picture_needs_what_its_pixels_and_frame_rate_and_codec_say() {
        assert_eq!(picture_kbps(VideoCodec::H264, (1920, 1080), 60), 19_906);
        assert_eq!(picture_kbps(VideoCodec::H264, (1920, 1080), 30), 9_953);
        // Fewer bits for the same quality from the newer codecs.
        assert!(
            picture_kbps(VideoCodec::H265, (1920, 1080), 60)
                < picture_kbps(VideoCodec::H264, (1920, 1080), 60) * 7 / 10
        );
        assert!(
            picture_kbps(VideoCodec::Av1, (1920, 1080), 60)
                < picture_kbps(VideoCodec::H265, (1920, 1080), 60)
        );
        // A large picture stops at what the protocol carries; a tiny one at
        // the floor.
        assert_eq!(picture_kbps(VideoCodec::H264, (3840, 2160), 60), 50_000);
        assert_eq!(picture_kbps(VideoCodec::Av1, (320, 180), 1), FLOOR_KBPS);
    }
    #[test]
    fn the_encode_size_keeps_the_monitors_proportions_inside_the_limit() {
        assert_eq!(encode_size((1920, 1080), (3840, 2160)), (1920, 1080));
        assert_eq!(encode_size((1920, 1080), (3440, 1440)), (1920, 803));
        assert_eq!(encode_size((1920, 1080), (1280, 800)), (1280, 800));
        assert_eq!(encode_size((1920, 1080), (1080, 1920)), (607, 1080));
        assert_eq!(encode_size((1920, 1080), (0, 0)), (1920, 1080));
    }
}
