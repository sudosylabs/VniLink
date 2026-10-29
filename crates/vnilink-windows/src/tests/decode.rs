use vnilink::EncodedFrame;
use vnilink_media::{Error, VideoDecoder};

#[cfg(windows)]
use vnilink_media::{EncodeParams, RawFrame, VideoEncoder};

use crate::decode::WindowsDecoder;
#[cfg(windows)]
use crate::encode::WindowsEncoder;

#[test]
fn frames_before_the_first_keyframe_are_refused() {
    let mut decoder = WindowsDecoder::new();
    let predicted = EncodedFrame::h264(false, 0, vec![0, 0, 0, 1, 0x41, 0x00]).unwrap();
    assert!(matches!(
        decoder.decode(&predicted),
        Err(Error::DecodeRejected("the decoder has not seen a keyframe"))
    ));
    assert!(matches!(
        decoder.decode(&EncodedFrame::h264(true, 0, vec![0, 0, 0, 1, 0x65, 0x00]).unwrap()),
        Err(Error::DecodeRejected(
            "a keyframe needs SPS, PPS, and an IDR"
        ))
    ));
}

#[cfg(not(windows))]
#[test]
fn the_decoder_is_unavailable() {
    // A real SPS with stub PPS and IDR passes validation, so the test reaches
    // the Media Foundation stub instead of the bitstream checks.
    let mut keyframe = vec![
        0, 0, 0, 1, 0x67, 0x42, 0xc0, 0x0a, 0x96, 0x54, 0x21, 0x34, 0x20, 0x00, 0x00, 0x03, 0x00,
        0x20, 0x00, 0x00, 0x07, 0x91, 0xe1, 0x10, 0x8d, 0x40,
    ];
    keyframe.extend_from_slice(&[0, 0, 0, 1, 0x68, 0xce]);
    keyframe.extend_from_slice(&[0, 0, 0, 1, 0x65, 0x10]);
    let frame = EncodedFrame::h264(true, 0, keyframe).unwrap();
    let mut decoder = WindowsDecoder::new();
    assert!(matches!(
        decoder.decode(&frame),
        Err(Error::Unsupported("Media Foundation H.264"))
    ));
}

#[cfg(windows)]
#[test]
fn media_foundation_decodes_what_it_encoded() {
    let mut encoder = WindowsEncoder::open(64, 64, 30, 500_000, vnilink::YuvMatrix::Bt709).unwrap();
    let sample = encoder
        .encode(
            &solid(64, 64, 0, 0, 0, 255),
            &EncodeParams {
                force_keyframe: true,
                recovery: false,
                bitrate_bps: 500_000,
                frame_rate: 30,
            },
        )
        .unwrap();
    assert!(sample.keyframe());
    let encoded = EncodedFrame::h264(true, 0, sample.bytes().to_vec()).unwrap();
    let mut decoder = WindowsDecoder::new();
    let picture = decoder.decode(&encoded).unwrap();
    assert_eq!((picture.width(), picture.height()), (64, 64));
    let pixel = &picture.row(32)[32 * 4..32 * 4 + 4];
    assert!(
        pixel[2] > pixel[0] && pixel[2] > pixel[1] && pixel[2] > 128,
        "decoded picture is not the encoded color"
    );
}

/// Each picture comes out of the decoder for its own input, none later,
/// whichever transform on this machine encoded it. Every picture is a page
/// of text with a grey square whose grey is that picture's own; consecutive
/// greys are far apart, so a picture that came out late is told by its grey. A lone keyframe into
/// a fresh decoder comes out too.
///
/// Measured before low-latency mode and waiting for the output (Windows 11
/// 26H2, 2026-10-23): Intel's stream came out one picture late for half to
/// three quarters of its pictures, and 11 to 38 of 40 lone keyframes from
/// Intel's and Microsoft's transforms gave no picture.
#[cfg(windows)]
#[test]
fn every_picture_decodes_on_its_own_input() {
    use vnilink_media::measure::text_page;
    const PICTURES: u64 = 100;
    const KEYFRAMES: usize = 20;
    let (width, height) = (1280u16, 720u16);
    let grey = |index: u64| 16 + (index * 37 % 200) as u8;
    let params = |index: u64| EncodeParams {
        force_keyframe: index % 30 == 0,
        recovery: false,
        bitrate_bps: 8_000_000,
        frame_rate: 30,
    };
    for candidate in unsafe { crate::transform::h264_encoders() } {
        let name = &candidate.name;
        let Ok(mut encoder) = WindowsEncoder::open_candidate(
            &candidate,
            width,
            height,
            30,
            8_000_000,
            vnilink::YuvMatrix::Bt709,
        ) else {
            continue;
        };
        let mut decoder = WindowsDecoder::new();
        for index in 0..PICTURES {
            let level = grey(index);
            let timestamp_us = index * 33_333;
            let mut pixels = text_page(width, height, index).pixels().to_vec();
            for y in usize::from(height) / 2 - 32..usize::from(height) / 2 + 32 {
                let row = y * usize::from(width) * 4;
                for x in usize::from(width) / 2 - 32..usize::from(width) / 2 + 32 {
                    pixels[row + x * 4..row + x * 4 + 3].copy_from_slice(&[level; 3]);
                }
            }
            let picture = RawFrame::bgra(width, height, timestamp_us, pixels).unwrap();
            let sample = encoder
                .encode(&picture, &params(index))
                .unwrap_or_else(|err| panic!("{name}: picture {index} was not encoded: {err}"));
            let encoded =
                EncodedFrame::h264(sample.keyframe(), timestamp_us, sample.bytes().to_vec())
                    .unwrap();
            let decoded = decoder
                .decode(&encoded)
                .unwrap_or_else(|err| panic!("{name}: picture {index} did not come out: {err}"));
            // Nearer this picture's grey than any of the pictures before. At
            // 2 Mb/s Microsoft's transform, starved by its keyframe, moved
            // the square only part of the way to the next grey; 8 Mb/s
            // leaves every transform room to draw it.
            let centre = decoded.row(height / 2)[usize::from(width) / 2 * 4];
            let own = centre.abs_diff(level);
            for earlier in index.saturating_sub(3)..index {
                assert!(
                    own < centre.abs_diff(grey(earlier)),
                    "{name}: picture {index} decoded as grey {centre}, nearer picture {earlier}'s {} than its own {level}",
                    grey(earlier)
                );
            }
        }
        drop(encoder);
        for attempt in 0..KEYFRAMES {
            // Some hardware transforms refuse a picture this small.
            let Ok(mut encoder) = WindowsEncoder::open_candidate(
                &candidate,
                64,
                64,
                30,
                500_000,
                vnilink::YuvMatrix::Bt709,
            ) else {
                break;
            };
            let sample = encoder
                .encode(&solid(64, 64, 0, 0, 0, 255), &params(0))
                .unwrap();
            let encoded = EncodedFrame::h264(true, 0, sample.bytes().to_vec()).unwrap();
            let mut decoder = WindowsDecoder::new();
            decoder
                .decode(&encoded)
                .unwrap_or_else(|err| panic!("{name}: lone keyframe {attempt}: {err}"));
        }
    }
}

#[cfg(windows)]
fn solid(width: u16, height: u16, timestamp_us: u64, b: u8, g: u8, r: u8) -> RawFrame {
    let mut pixels = Vec::with_capacity(usize::from(width) * usize::from(height) * 4);
    for _ in 0..usize::from(width) * usize::from(height) {
        pixels.extend_from_slice(&[b, g, r, 255]);
    }
    RawFrame::bgra(width, height, timestamp_us, pixels).unwrap()
}

/// How long each picture takes to come out of the decoder, per encoder
/// transform on this machine, against [`crate::decode::DECODE_WAIT`].
/// Windows only; run with
/// `cargo test -p vnilink-windows --release --lib decode_wait -- --ignored --nocapture`.
#[cfg(windows)]
#[test]
#[ignore = "measurement"]
fn decode_wait() {
    use vnilink_media::measure::text_page;
    const PICTURES: u64 = 120;
    let (width, height) = (1920u16, 1080u16);
    for candidate in unsafe { crate::transform::h264_encoders() } {
        let Ok(mut encoder) = WindowsEncoder::open_candidate(
            &candidate,
            width,
            height,
            30,
            8_000_000,
            vnilink::YuvMatrix::Bt709,
        ) else {
            println!("{} ({:?}): did not open", candidate.name, candidate.vendor);
            continue;
        };
        let mut decoder = WindowsDecoder::new();
        let mut times = Vec::new();
        for index in 0..PICTURES {
            let timestamp_us = index * 33_333;
            let picture = text_page(width, height, index).with_timestamp(timestamp_us);
            let sample = encoder
                .encode(
                    &picture,
                    &EncodeParams {
                        force_keyframe: index % 30 == 0,
                        recovery: false,
                        bitrate_bps: 8_000_000,
                        frame_rate: 30,
                    },
                )
                .unwrap();
            let encoded =
                EncodedFrame::h264(sample.keyframe(), timestamp_us, sample.bytes().to_vec())
                    .unwrap();
            let started = std::time::Instant::now();
            decoder.decode(&encoded).unwrap();
            times.push(started.elapsed());
        }
        times.sort();
        let at = |share: f64| times[((times.len() - 1) as f64 * share) as usize];
        println!(
            "{} ({:?}): 1080p decode p50 {:?}, p99 {:?}, max {:?}; bound {:?}",
            candidate.name,
            candidate.vendor,
            at(0.5),
            at(0.99),
            times[times.len() - 1],
            crate::decode::DECODE_WAIT,
        );
    }
}
