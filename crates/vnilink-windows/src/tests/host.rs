use vnilink_media::Quality;

#[cfg(not(windows))]
use vnilink_media::Error;
#[cfg(windows)]
use vnilink_media::{parse_annex_b, FrameSink, PumpOutput, RawFrame};

use crate::encode::WindowsEncoder;
use crate::host::HostSession;

#[cfg(not(windows))]
#[test]
fn the_host_encoder_is_unavailable() {
    let quality = Quality::new(500, 30, 64, 64).unwrap();
    assert!(matches!(
        HostSession::open(64, 64, quality, WindowsEncoder::open),
        Err(Error::Unsupported("Media Foundation H.264"))
    ));
}

#[cfg(windows)]
#[test]
fn the_platform_encoder_plugs_into_the_generic_session() {
    use vnilink::CoreEvent;

    let quality = Quality::new(500, 30, 64, 64).unwrap();
    let mut session = HostSession::open(128, 128, quality, WindowsEncoder::open).unwrap();
    assert_eq!(session.capture_size(), (128, 128));
    assert_eq!(session.encoded_size(), (64, 64));
    let mut sink = MemorySink::default();
    let first = session.publish(&solid(128, 128, 0), &mut sink, 7).unwrap();
    assert!(matches!(first, PumpOutput::Encoded(_)));
    assert_eq!(sink.ids, vec![7]);
    let parsed = parse_annex_b(&sink.frames[0].payload).unwrap();
    assert!(sink.frames[0].keyframe && parsed.has_sps() && parsed.has_pps() && parsed.has_idr());

    let skipped = session
        .publish(&solid(128, 128, 1_000), &mut sink, 7)
        .unwrap();
    assert_eq!(skipped, PumpOutput::Skipped);
    assert_eq!(sink.ids.len(), 1);

    session
        .observe(&CoreEvent::KeyframeRequested { id: 7 })
        .unwrap();
    let forced = session
        .publish(&solid(128, 128, 2_000), &mut sink, 7)
        .unwrap();
    assert!(matches!(forced, PumpOutput::Encoded(_)));
    assert!(sink.frames[1].keyframe);
}

#[cfg(windows)]
#[derive(Default)]
struct MemorySink {
    ids: Vec<u64>,
    frames: Vec<vnilink::EncodedFrame>,
}

#[cfg(windows)]
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

#[cfg(windows)]
fn solid(width: u16, height: u16, timestamp_us: u64) -> RawFrame {
    let mut pixels = Vec::with_capacity(usize::from(width) * usize::from(height) * 4);
    for _ in 0..usize::from(width) * usize::from(height) {
        pixels.extend_from_slice(&[0, 0, 255, 255]);
    }
    RawFrame::bgra(width, height, timestamp_us, pixels).unwrap()
}
