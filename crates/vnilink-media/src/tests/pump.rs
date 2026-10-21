use std::sync::{Arc, Mutex};

use vnilink::Quality;
use vnilink::{CoreEvent, EncodedFrame};

use crate::annexb::{to_annex_b, AvccAccessUnit};
use crate::error::{Error, PublishError};
use crate::frame::RawFrame;
use crate::pump::{
    EncodeParams, EncodedSample, FrameSink, HostVideo, PumpOutput, VideoDecoder, VideoEncoder,
};

struct Log {
    honor_force: bool,
    calls: Vec<EncodeParams>,
    sizes: Vec<(u16, u16)>,
}

struct Scripted {
    log: Arc<Mutex<Log>>,
}

impl Scripted {
    fn honoring() -> (Self, Arc<Mutex<Log>>) {
        let log = Arc::new(Mutex::new(Log {
            honor_force: true,
            calls: Vec::new(),
            sizes: Vec::new(),
        }));
        (
            Self {
                log: Arc::clone(&log),
            },
            log,
        )
    }
}

impl VideoEncoder for Scripted {
    fn encode(&mut self, frame: &RawFrame, params: &EncodeParams) -> Result<EncodedSample, Error> {
        let mut log = self.log.lock().unwrap();
        log.calls.push(*params);
        log.sizes.push((frame.width(), frame.height()));
        let keyframe = log.honor_force && params.force_keyframe;
        drop(log);
        sample(keyframe)
    }
}

fn sample(keyframe: bool) -> Result<EncodedSample, Error> {
    let sps = vec![0x67, 0x42];
    let pps = vec![0x68, 0xCE];
    let picture: &[u8] = if keyframe {
        &[0x65, 0x10]
    } else {
        &[0x41, 0x10]
    };
    let mut avcc = (picture.len() as u32).to_be_bytes().to_vec();
    avcc.extend_from_slice(picture);
    let sets = [sps, pps];
    let annex = to_annex_b(&AvccAccessUnit {
        nal_length_size: 4,
        avcc: &avcc,
        parameter_sets: &sets,
        keyframe,
    })?;
    EncodedSample::annex_b(keyframe, annex)
}

fn picture(width: u16, height: u16, timestamp_us: u64) -> RawFrame {
    RawFrame::bgra(
        width,
        height,
        timestamp_us,
        vec![0; usize::from(width) * usize::from(height) * 4],
    )
    .unwrap()
}

fn quality(fps: u8, width: u16, height: u16) -> Quality {
    Quality::new(2_000, fps, width, height).unwrap()
}

fn encoded(output: PumpOutput) -> EncodedFrame {
    match output {
        PumpOutput::Encoded(frame) => frame,
        PumpOutput::Skipped => panic!("expected an encoded frame"),
    }
}

#[test]
fn the_first_frame_is_a_keyframe_and_carries_the_bitrate() {
    let (encoder, log) = Scripted::honoring();
    let mut host = HostVideo::new(quality(30, 64, 64), encoder);
    let frame = encoded(host.push(&picture(64, 64, 0)).unwrap());
    assert!(frame.keyframe);
    assert_eq!(frame.timestamp_us, 0);
    assert_eq!(&frame.payload[..4], &[0, 0, 0, 1]);
    let log = log.lock().unwrap();
    assert_eq!(log.calls[0].bitrate_bps, 2_000_000);
    assert_eq!(log.calls[0].frame_rate, 30);
    assert!(log.calls[0].force_keyframe);
    assert_eq!(log.sizes[0], (64, 64));
}

#[test]
fn frames_inside_the_interval_are_skipped_until_a_keyframe_is_requested() {
    let (encoder, log) = Scripted::honoring();
    let mut host = HostVideo::new(quality(30, 32, 32), encoder);
    host.push(&picture(32, 32, 0)).unwrap();
    assert!(matches!(
        host.push(&picture(32, 32, 33_332)).unwrap(),
        PumpOutput::Skipped
    ));
    let frame = encoded(host.push(&picture(32, 32, 33_333)).unwrap());
    assert!(!frame.keyframe);
    {
        let log = log.lock().unwrap();
        assert_eq!(log.calls.len(), 2);
        assert!(!log.calls[1].force_keyframe);
    }

    host.observe(&CoreEvent::InputFailed {
        id: 1,
        message: "input was rejected",
    });
    assert!(matches!(
        host.push(&picture(32, 32, 33_400)).unwrap(),
        PumpOutput::Skipped
    ));
    host.observe(&CoreEvent::KeyframeRequested { id: 9 });
    let forced = encoded(host.push(&picture(32, 32, 33_500)).unwrap());
    assert!(forced.keyframe);
    assert!(log.lock().unwrap().calls.last().unwrap().force_keyframe);
}

/// A keyframe the core asks for answers a loss and may be capped; the
/// first picture, a size change, and a full request are not.
#[test]
fn only_a_requested_keyframe_is_a_recovery_keyframe() {
    let (encoder, log) = Scripted::honoring();
    let mut host = HostVideo::new(quality(30, 32, 32), encoder);
    host.push(&picture(32, 32, 0)).unwrap();
    host.observe(&CoreEvent::KeyframeRequested { id: 1 });
    host.push(&picture(32, 32, 1_000)).unwrap();
    host.push(&picture(32, 32, 40_000)).unwrap();
    host.observe(&CoreEvent::KeyframeRequested { id: 1 });
    host.request_keyframe();
    host.push(&picture(32, 32, 41_000)).unwrap();
    host.request_keyframe();
    host.request_recovery_keyframe();
    host.push(&picture(32, 32, 42_000)).unwrap();
    host.observe(&CoreEvent::KeyframeRequested { id: 1 });
    host.observe(&CoreEvent::QualityUpdated {
        id: 1,
        quality: quality(30, 16, 16),
    });
    host.push(&picture(32, 32, 43_000)).unwrap();
    let calls: Vec<_> = log
        .lock()
        .unwrap()
        .calls
        .iter()
        .map(|call| (call.force_keyframe, call.recovery))
        .collect();
    assert_eq!(
        calls,
        [
            (true, false),
            (true, true),
            (false, false),
            (true, false),
            (true, false),
            (true, false),
        ]
    );
}

/// A recovery keyframe is replaced by a full-quality one once no loss has
/// asked for another; a loss in between puts the replacement off.
#[test]
fn a_recovery_keyframe_is_refreshed_at_full_quality_once_the_link_is_quiet() {
    let refresh = crate::pump::REFRESH_AFTER_US;
    let (encoder, log) = Scripted::honoring();
    let mut host = HostVideo::new(quality(30, 32, 32), encoder);
    host.push(&picture(32, 32, 0)).unwrap();
    host.observe(&CoreEvent::KeyframeRequested { id: 1 });
    host.push(&picture(32, 32, 100_000)).unwrap();
    // Another loss before the refresh: a new recovery keyframe, and the
    // refresh is put off from it.
    host.observe(&CoreEvent::KeyframeRequested { id: 1 });
    host.push(&picture(32, 32, 1_000_000)).unwrap();
    host.push(&picture(32, 32, 100_000 + refresh)).unwrap();
    host.push(&picture(32, 32, 1_000_000 + refresh)).unwrap();
    // Refreshed once only.
    host.push(&picture(32, 32, 1_000_000 + 3 * refresh))
        .unwrap();
    let calls: Vec<_> = log
        .lock()
        .unwrap()
        .calls
        .iter()
        .map(|call| (call.force_keyframe, call.recovery))
        .collect();
    assert_eq!(
        calls,
        [
            (true, false),
            (true, true),
            (true, true),
            (false, false),
            (true, false),
            (false, false),
        ]
    );
}

#[test]
fn a_clock_that_moves_backwards_is_encoded() {
    let (encoder, _) = Scripted::honoring();
    let mut host = HostVideo::new(quality(30, 16, 16), encoder);
    host.push(&picture(16, 16, 50_000)).unwrap();
    assert!(matches!(
        host.push(&picture(16, 16, 49_000)).unwrap(),
        PumpOutput::Encoded(_)
    ));
}

#[test]
fn a_delta_is_rejected_when_a_keyframe_was_required() {
    let (encoder, log) = Scripted::honoring();
    log.lock().unwrap().honor_force = false;
    let mut host = HostVideo::new(quality(30, 16, 16), encoder);
    assert!(matches!(
        host.push(&picture(16, 16, 0)),
        Err(Error::EncodeRejected(
            "the encoder did not produce a keyframe"
        ))
    ));
    log.lock().unwrap().honor_force = true;
    let frame = encoded(host.push(&picture(16, 16, 1)).unwrap());
    assert!(frame.keyframe);
}

#[test]
fn oversized_input_is_scaled_and_a_size_change_forces_a_keyframe() {
    let (encoder, log) = Scripted::honoring();
    let mut host = HostVideo::new(quality(60, 160, 120), encoder);
    host.push(&picture(320, 180, 0)).unwrap();
    assert_eq!(log.lock().unwrap().sizes[0], (160, 90));
    let updated = quality(10, 80, 60);
    host.observe(&CoreEvent::QualityUpdated {
        id: 3,
        quality: updated,
    });
    assert_eq!(host.quality(), updated);
    host.push(&picture(320, 180, 1_000)).unwrap();
    let log = log.lock().unwrap();
    assert_eq!(log.sizes[1], (80, 44));
    assert!(log.calls[1].force_keyframe);
    assert_eq!(log.calls[1].frame_rate, 10);
}

#[test]
fn a_one_pixel_limit_cannot_be_encoded() {
    let quality = Quality::new(1, 1, 1, 1).unwrap();
    let (encoder, _) = Scripted::honoring();
    let mut host = HostVideo::new(quality, encoder);
    assert!(matches!(
        host.push(&picture(8, 8, 0)),
        Err(Error::InvalidSetting(
            "encoded size must be at least 2 by 2"
        ))
    ));
}

#[test]
fn publish_to_sends_encoded_frames_and_skips_the_rest() {
    let (encoder, _) = Scripted::honoring();
    let mut host = HostVideo::new(quality(30, 16, 16), encoder);
    let sent = Mutex::new(Vec::new());
    {
        let mut sink = MemSink { sent: &sent };
        let output = host.publish_to(&mut sink, 4, &picture(16, 16, 0)).unwrap();
        assert!(matches!(output, PumpOutput::Encoded(_)));
        host.publish_to(&mut sink, 4, &picture(16, 16, 1)).unwrap();
    }
    let sent = sent.lock().unwrap();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].0, 4);
    assert!(sent[0].1.keyframe);
}

#[test]
fn an_idr_without_parameter_sets_is_not_a_sample() {
    let annex = to_annex_b(&AvccAccessUnit {
        nal_length_size: 4,
        avcc: &[0, 0, 0, 2, 0x65, 0x01],
        parameter_sets: &[],
        keyframe: false,
    })
    .unwrap();
    assert!(matches!(
        EncodedSample::annex_b(true, annex),
        Err(Error::EncodeRejected(
            "a keyframe needs SPS, PPS, and an IDR"
        ))
    ));
}

struct MemSink<'a> {
    sent: &'a Mutex<Vec<(u64, EncodedFrame)>>,
}

impl FrameSink for MemSink<'_> {
    fn publish_frame(
        &mut self,
        session_id: u64,
        frame: EncodedFrame,
    ) -> Result<(), vnilink::Error> {
        self.sent.lock().unwrap().push((session_id, frame));
        Ok(())
    }
}

struct CopyDecoder;

impl VideoDecoder for CopyDecoder {
    fn decode(&mut self, frame: &EncodedFrame) -> Result<RawFrame, Error> {
        if frame.payload.first() != Some(&0) {
            return Err(Error::DecodeRejected("the decoder rejected the frame"));
        }
        RawFrame::bgra(2, 2, frame.timestamp_us, vec![7; 16])
    }
}

#[test]
fn a_decoder_returns_pixels_without_reading_them_back_from_the_log() {
    let mut decoder = CopyDecoder;
    let bad = EncodedFrame::h264(false, 1, vec![1, 2]).unwrap();
    assert!(matches!(
        decoder.decode(&bad),
        Err(Error::DecodeRejected("the decoder rejected the frame"))
    ));
    let annex = sample(true).unwrap();
    let good = EncodedFrame::h264(true, 9, annex.bytes().to_vec()).unwrap();
    let picture = decoder.decode(&good).unwrap();
    assert_eq!(picture.timestamp_us(), 9);
    assert_eq!(picture.width(), 2);
}

#[test]
fn publish_error_displays_the_media_reason() {
    let err = PublishError::from(Error::Unsupported("VA-API H.264"));
    assert_eq!(
        err.to_string(),
        "VA-API H.264 is not available on this system"
    );
}
