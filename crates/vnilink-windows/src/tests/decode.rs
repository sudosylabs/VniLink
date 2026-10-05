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
    let mut encoder = WindowsEncoder::open(64, 64, 30, 500_000).unwrap();
    let sample = encoder
        .encode(
            &solid(64, 64, 0, 0, 0, 255),
            &EncodeParams {
                force_keyframe: true,
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

#[cfg(windows)]
fn solid(width: u16, height: u16, timestamp_us: u64, b: u8, g: u8, r: u8) -> RawFrame {
    let mut pixels = Vec::with_capacity(usize::from(width) * usize::from(height) * 4);
    for _ in 0..usize::from(width) * usize::from(height) {
        pixels.extend_from_slice(&[b, g, r, 255]);
    }
    RawFrame::bgra(width, height, timestamp_us, pixels).unwrap()
}
