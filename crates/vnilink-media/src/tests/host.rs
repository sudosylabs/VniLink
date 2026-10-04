use std::sync::{Arc, Mutex};

use crate::annexb::{to_annex_b, AvccAccessUnit};
use crate::error::{Error, PublishError};
use crate::frame::RawFrame;
use crate::host::HostSession;
use crate::pump::{EncodeParams, EncodedSample, FrameSink, PumpOutput, VideoEncoder};
use crate::{parse_annex_b, Quality};
use vnilink::CoreEvent;

struct Fake {
    width: u16,
    height: u16,
}

impl Fake {
    fn open(
        opens: &Arc<Mutex<usize>>,
        width: u16,
        height: u16,
        frame_rate: u8,
        bitrate_bps: u32,
    ) -> Result<Self, Error> {
        *opens.lock().unwrap() += 1;
        if width == 0 || height == 0 || width % 2 != 0 || height % 2 != 0 {
            return Err(Error::InvalidFrame(
                "frame width and height must be non-zero",
            ));
        }
        if frame_rate == 0 || bitrate_bps == 0 {
            return Err(Error::InvalidSetting("frame rate must be non-zero"));
        }
        Ok(Self { width, height })
    }
}

impl VideoEncoder for Fake {
    fn encode(&mut self, frame: &RawFrame, params: &EncodeParams) -> Result<EncodedSample, Error> {
        if frame.width() != self.width || frame.height() != self.height {
            return Err(Error::InvalidFrame(
                "the frame size does not match the encoder",
            ));
        }
        sample(params.force_keyframe)
    }
}

fn sample(keyframe: bool) -> Result<EncodedSample, Error> {
    let picture: &[u8] = if keyframe {
        &[0x65, 0x10]
    } else {
        &[0x41, 0x10]
    };
    let mut avcc = (picture.len() as u32).to_be_bytes().to_vec();
    avcc.extend_from_slice(picture);
    let sets = [vec![0x67, 0x42], vec![0x68, 0xCE]];
    let annex = to_annex_b(&AvccAccessUnit {
        nal_length_size: 4,
        avcc: &avcc,
        parameter_sets: &sets,
        keyframe,
    })?;
    EncodedSample::annex_b(keyframe, annex)
}

fn solid(width: u16, height: u16, timestamp_us: u64) -> RawFrame {
    RawFrame::bgra(
        width,
        height,
        timestamp_us,
        vec![0; usize::from(width) * usize::from(height) * 4],
    )
    .unwrap()
}

fn session(capture: (u16, u16), quality: Quality, opens: &Arc<Mutex<usize>>) -> HostSession<Fake> {
    let opener = Arc::clone(opens);
    HostSession::open(capture.0, capture.1, quality, move |w, h, fps, bps| {
        Fake::open(&opener, w, h, fps, bps)
    })
    .unwrap()
}

#[test]
fn a_paced_picture_is_published_and_a_fast_one_is_skipped() {
    let opens = Arc::new(Mutex::new(0));
    let quality = Quality::new(500, 30, 32, 32).unwrap();
    let mut session = session((64, 64), quality, &opens);
    assert_eq!(session.capture_size(), (64, 64));
    assert_eq!(session.encoded_size(), (32, 32));
    assert_eq!(*opens.lock().unwrap(), 1);
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
        Err(PublishError::Media(Error::InvalidFrame(
            "the frame size does not match the encoder"
        )))
    ));
}

#[test]
fn a_size_change_reopens_the_encoder_through_the_session() {
    let opens = Arc::new(Mutex::new(0));
    let mut session = session((64, 64), Quality::new(500, 30, 32, 32).unwrap(), &opens);
    session
        .observe(&CoreEvent::QualityUpdated {
            id: 7,
            quality: Quality::new(500, 30, 16, 16).unwrap(),
        })
        .unwrap();
    assert_eq!(*opens.lock().unwrap(), 2);
    assert_eq!(session.encoded_size(), (16, 16));
}

#[derive(Default)]
struct MemorySink {
    ids: Vec<u64>,
    frames: Vec<vnilink::EncodedFrame>,
}

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
