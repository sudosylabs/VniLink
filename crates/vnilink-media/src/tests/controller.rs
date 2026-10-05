use std::sync::atomic::AtomicBool;

use crate::controller::{ControllerOutput, ControllerVideo};
use crate::error::Error;
use crate::frame::RawFrame;
use crate::pump::VideoDecoder;
use vnilink::EncodedFrame;

struct CopyDecoder {
    fail_unsupported: bool,
}

impl VideoDecoder for CopyDecoder {
    fn decode(&mut self, frame: &EncodedFrame) -> Result<RawFrame, Error> {
        if self.fail_unsupported {
            return Err(Error::Unsupported("no decoder on this system"));
        }
        if frame.payload.first() != Some(&0) {
            return Err(Error::DecodeRejected("the decoder has not seen a keyframe"));
        }
        RawFrame::bgra(2, 2, frame.timestamp_us, vec![7; 16])
    }
}

fn good(timestamp_us: u64) -> EncodedFrame {
    EncodedFrame::h264(true, timestamp_us, vec![0, 1]).unwrap()
}

fn bad(timestamp_us: u64) -> EncodedFrame {
    EncodedFrame::h264(false, timestamp_us, vec![1, 2]).unwrap()
}

#[test]
fn rejected_frames_skip_and_unsupported_ends() {
    let mut controller = ControllerVideo::new(CopyDecoder {
        fail_unsupported: false,
    });
    assert!(matches!(
        controller.push(&bad(1)).unwrap(),
        ControllerOutput::Skipped
    ));
    assert_eq!(controller.skipped(), 1);
    assert_eq!(controller.decoded(), 0);
    let decoded = controller.push(&good(2)).unwrap();
    assert!(matches!(decoded, ControllerOutput::Decoded(_)));
    assert_eq!(controller.decoded(), 1);

    let mut broken = ControllerVideo::new(CopyDecoder {
        fail_unsupported: true,
    });
    assert!(matches!(broken.push(&good(3)), Err(Error::Unsupported(_))));
}

#[test]
fn serve_controller_paints_in_order_and_stops_on_request() {
    use std::sync::atomic::Ordering;

    use crate::controller::serve_controller;

    // Good, bad, good: the bad middle frame skips, the two good frames
    // paint in order. The paint callback stops the loop once both arrived.
    let mut frames = vec![good(1), bad(2), good(3)].into_iter();
    let stop = AtomicBool::new(false);
    let mut painted = Vec::new();
    let mut video = ControllerVideo::new(CopyDecoder {
        fail_unsupported: false,
    });
    serve_controller(&mut |_| frames.next(), &mut video, &stop, |picture| {
        painted.push(picture.timestamp_us());
        if painted.len() >= 2 {
            stop.store(true, Ordering::Relaxed);
        }
    })
    .unwrap();
    assert_eq!(painted, vec![1, 3]);
    assert_eq!(video.decoded(), 2);
    assert_eq!(video.skipped(), 1);

    // A set stop paints nothing and returns at once.
    let mut frames = vec![good(9)].into_iter();
    let stop = AtomicBool::new(true);
    let mut video = ControllerVideo::new(CopyDecoder {
        fail_unsupported: false,
    });
    let mut painted = 0;
    serve_controller(&mut |_| frames.next(), &mut video, &stop, |_| painted += 1).unwrap();
    assert_eq!(painted, 0);
}
