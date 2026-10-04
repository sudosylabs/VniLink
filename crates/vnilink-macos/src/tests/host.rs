use vnilink_media::Quality;

#[cfg(not(target_os = "macos"))]
use vnilink_media::Error;
#[cfg(target_os = "macos")]
use vnilink_media::{parse_annex_b, FrameSink, PumpOutput, RawFrame};

use crate::encode::MacEncoder;
use crate::host::HostSession;

#[cfg(not(target_os = "macos"))]
#[test]
fn the_host_encoder_is_unavailable() {
    let quality = Quality::new(500, 30, 64, 64).unwrap();
    assert!(matches!(
        HostSession::open(64, 64, quality, MacEncoder::open),
        Err(Error::Unsupported("VideoToolbox H.264"))
    ));
}

#[cfg(target_os = "macos")]
#[test]
fn the_platform_encoder_plugs_into_the_generic_session() {
    use vnilink::CoreEvent;

    let quality = Quality::new(500, 30, 32, 32).unwrap();
    let mut session = HostSession::open(64, 64, quality, MacEncoder::open).unwrap();
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
}

#[cfg(target_os = "macos")]
#[derive(Default)]
struct MemorySink {
    ids: Vec<u64>,
    frames: Vec<vnilink::EncodedFrame>,
}

#[cfg(target_os = "macos")]
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

#[cfg(target_os = "macos")]
fn solid(width: u16, height: u16, timestamp_us: u64) -> RawFrame {
    let mut pixels = Vec::with_capacity(usize::from(width) * usize::from(height) * 4);
    for _ in 0..usize::from(width) * usize::from(height) {
        pixels.extend_from_slice(&[0, 0, 255, 255]);
    }
    RawFrame::bgra(width, height, timestamp_us, pixels).unwrap()
}
