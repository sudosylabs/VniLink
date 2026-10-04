use vnilink::CoreEvent;
use vnilink_media::{parse_annex_b, Error, FrameSink, PumpOutput, Quality, RawFrame};

use crate::host::HostSession;

#[test]
fn a_quality_below_two_pixels_is_rejected() {
    let quality = Quality::new(500, 30, 1, 1).unwrap();
    assert!(matches!(
        HostSession::open(64, 64, quality),
        Err(Error::InvalidSetting(
            "encoded size must be at least 2 by 2"
        ))
    ));
}

#[cfg(not(target_os = "linux"))]
#[test]
fn the_host_encoder_is_unavailable() {
    let quality = Quality::new(500, 30, 64, 64).unwrap();
    assert!(matches!(
        HostSession::open(64, 64, quality),
        Err(Error::Unsupported("VA-API H.264"))
    ));
}

#[cfg(target_os = "linux")]
#[test]
fn a_paced_picture_is_published_and_a_fast_one_is_skipped() {
    let quality = Quality::new(500, 30, 32, 32).unwrap();
    let mut session = match HostSession::open(64, 64, quality) {
        Ok(session) => session,
        Err(Error::Unsupported("VA-API H.264")) => return,
        Err(err) => panic!("VA-API failed to open: {err}"),
    };
    assert_eq!(session.capture_size(), (64, 64));
    assert_eq!(session.encoded_size(), (32, 32));
    let mut sink = MemorySink::default();
    let first = session.publish(&solid(64, 64, 0), &mut sink, 7).unwrap();
    assert!(matches!(first, PumpOutput::Encoded(_)));
    assert_eq!(sink.ids, vec![7]);
    let parsed = parse_annex_b(&sink.frames[0].payload).unwrap();
    assert!(sink.frames[0].keyframe && parsed.has_sps() && parsed.has_pps() && parsed.has_idr());

    let skipped = session
        .publish(&solid(64, 64, 1_000), &mut sink, 7)
        .unwrap();
    assert_eq!(skipped, PumpOutput::Skipped);
    assert_eq!(sink.ids.len(), 1);

    session
        .observe(&CoreEvent::KeyframeRequested { id: 7 })
        .unwrap();
    let forced = session
        .publish(&solid(64, 64, 2_000), &mut sink, 7)
        .unwrap();
    assert!(matches!(forced, PumpOutput::Encoded(_)));
    assert!(sink.frames[1].keyframe);
    assert!(matches!(
        session.publish(&solid(32, 32, 50_000), &mut sink, 7),
        Err(vnilink_media::PublishError::Media(Error::InvalidFrame(
            "the frame size does not match the encoder"
        )))
    ));
}

#[cfg(target_os = "linux")]
#[derive(Default)]
struct MemorySink {
    ids: Vec<u64>,
    frames: Vec<vnilink::EncodedFrame>,
}

#[cfg(target_os = "linux")]
impl FrameSink for MemorySink {
    fn publish_frame(
        &mut self,
        session_id: u64,
        frame: vnilink::EncodedFrame,
    ) -> Result<(), vnilink::Error> {
        self.ids.push(session_id);
        self.frames.push(frame);
        Ok(())
    }
}

#[cfg(target_os = "linux")]
fn solid(width: u16, height: u16, timestamp_us: u64) -> RawFrame {
    let mut pixels = Vec::with_capacity(usize::from(width) * usize::from(height) * 4);
    for _ in 0..usize::from(width) * usize::from(height) {
        pixels.extend_from_slice(&[0, 0, 255, 255]);
    }
    RawFrame::bgra(width, height, timestamp_us, pixels).unwrap()
}
