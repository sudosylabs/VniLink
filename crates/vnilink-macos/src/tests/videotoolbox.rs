use vnilink::EncodedFrame;
use vnilink_media::{
    parse_annex_b, EncodeParams, HostVideo, PumpOutput, Quality, VideoDecoder, VideoEncoder,
};

use crate::decode::MacDecoder;
use crate::encode::MacEncoder;

fn solid(
    width: u16,
    height: u16,
    timestamp_us: u64,
    b: u8,
    g: u8,
    r: u8,
) -> vnilink_media::RawFrame {
    let mut pixels = Vec::with_capacity(usize::from(width) * usize::from(height) * 4);
    for _ in 0..usize::from(width) * usize::from(height) {
        pixels.extend_from_slice(&[b, g, r, 255]);
    }
    vnilink_media::RawFrame::bgra(width, height, timestamp_us, pixels).unwrap()
}

#[test]
fn videotoolbox_encodes_and_decodes_a_synthetic_picture() {
    let mut encoder = MacEncoder::open(64, 64, 30, 500_000).unwrap();
    let params = EncodeParams {
        force_keyframe: true,
        bitrate_bps: 500_000,
        frame_rate: 30,
    };
    let sample = encoder
        .encode(&solid(64, 64, 0, 0, 0, 255), &params)
        .unwrap();
    assert!(sample.keyframe());
    let parsed = parse_annex_b(sample.bytes()).unwrap();
    assert!(parsed.has_sps() && parsed.has_pps() && parsed.has_idr());
    let encoded = EncodedFrame::h264(true, 0, sample.bytes().to_vec()).unwrap();
    let mut decoder = MacDecoder::new();
    let picture = decoder.decode(&encoded).unwrap();
    assert_eq!((picture.width(), picture.height()), (64, 64));
    let pixel = &picture.row(32)[32 * 4..32 * 4 + 4];
    assert!(
        pixel[2] > pixel[0] && pixel[2] > pixel[1] && pixel[2] > 128,
        "decoded picture is not the encoded color"
    );

    let delta = encoder
        .encode(
            &solid(64, 64, 33_333, 0, 0, 255),
            &EncodeParams {
                force_keyframe: false,
                bitrate_bps: 500_000,
                frame_rate: 30,
            },
        )
        .unwrap();
    let encoded_delta =
        EncodedFrame::h264(delta.keyframe(), 33_333, delta.bytes().to_vec()).unwrap();
    let next = decoder.decode(&encoded_delta).unwrap();
    assert_eq!((next.width(), next.height()), (64, 64));

    let junk = EncodedFrame::h264(false, 0, vec![0, 0, 0, 1, 0x41, 0x00]).unwrap();
    assert!(MacDecoder::new().decode(&junk).is_err());
}

#[test]
fn the_pump_accepts_a_videotoolbox_keyframe() {
    let quality = Quality::new(500, 30, 64, 64).unwrap();
    let encoder = MacEncoder::open(64, 64, 30, 500_000).unwrap();
    let mut video = HostVideo::new(quality, encoder);
    let output = video.push(&solid(64, 64, 0, 0, 255, 0)).unwrap();
    match output {
        PumpOutput::Encoded(frame) => assert!(frame.keyframe),
        PumpOutput::Skipped => panic!("the first picture must be encoded"),
    }
}
