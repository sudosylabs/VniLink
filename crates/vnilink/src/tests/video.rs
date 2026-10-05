use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use iroh::RelayMode;

use crate::core::{Core, CoreConfig, Listen};
use crate::error::Error;
use crate::events::CoreEvent;
use crate::input::{usage, HostDesktop, InputEvent};
use crate::session::{Grants, SessionPhase, SessionRole};
use crate::video::{
    decode_video_body, encode_video_body, EncodedFrame, HostCapture, VideoPush, VideoQueue,
    MAX_VIDEO_FRAME, VIDEO_QUEUE,
};

fn start_direct() -> Core {
    let core = Core::boot(
        CoreConfig {
            listen: Listen::Loopback,
            ..CoreConfig::default()
        },
        RelayMode::Disabled,
    )
    .expect("loopback core");
    core.set_h264_support(true, true).expect("flags");
    core
}

fn pair_for_control(host: &Core, controller: &Core) {
    host.set_h264_support(true, true).unwrap();
    controller.set_h264_support(true, true).unwrap();
    host.start_hosting().unwrap();
    let token = host.create_invitation(Duration::from_secs(60)).unwrap();
    controller.accept_invitation(&token).unwrap();
    let back = controller
        .create_invitation(Duration::from_secs(60))
        .unwrap();
    host.accept_invitation(&back).unwrap();
    host.authorize(
        &controller.endpoint_id(),
        Grants {
            control: true,
            clipboard: false,
            transfer: false,
        },
    )
    .unwrap();
}

fn wait_closed(core: &Core, role: SessionRole) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let snapshot = core.snapshot().unwrap();
        let session = match role {
            SessionRole::Host => snapshot.incoming,
            SessionRole::Controller => snapshot.outgoing,
        };
        if session
            .as_ref()
            .is_some_and(|session| session.phase == SessionPhase::Closed)
        {
            return;
        }
        if Instant::now() > deadline {
            panic!("session did not close: {session:?}");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn wait_applied(events: &Mutex<Vec<InputEvent>>, expected: &[InputEvent]) {
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        let got = events.lock().unwrap_or_else(|poison| poison.into_inner());
        if got.len() >= expected.len() {
            assert_eq!(got.as_slice(), expected);
            return;
        }
        drop(got);
        if Instant::now() > deadline {
            let got = events.lock().unwrap_or_else(|poison| poison.into_inner());
            panic!("applied events did not settle: {got:?}");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

struct SizeCapture {
    width: u16,
    height: u16,
}

impl HostCapture for SizeCapture {
    fn monitor(&self) -> (u16, u16) {
        (self.width, self.height)
    }
}

struct SizeDesktop {
    width: u16,
    height: u16,
    events: Arc<Mutex<Vec<InputEvent>>>,
}

impl HostDesktop for SizeDesktop {
    fn apply(&mut self, event: InputEvent) -> Result<(), Error> {
        self.events
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .push(event);
        Ok(())
    }

    fn monitor(&self) -> (u16, u16) {
        (self.width, self.height)
    }
}

#[test]
fn video_api_rejects_a_second_subscriber_and_a_missing_session() {
    let core = start_direct();
    assert!(!core.capabilities().desktop_capture);
    assert!(matches!(
        core.publish_video(1, EncodedFrame::h264(true, 1, vec![1]).unwrap()),
        Err(Error::NoSession)
    ));
    let first = core.subscribe_video().unwrap();
    assert!(matches!(core.subscribe_video(), Err(Error::Busy)));
    drop(first);
    assert!(core.subscribe_video().is_ok());
}

#[test]
fn encoded_frames_reach_the_controller_without_blocking_input() {
    let host = start_direct();
    let controller = start_direct();
    host.set_capture(Box::new(SizeCapture {
        width: 1280,
        height: 720,
    }))
    .unwrap();
    host.set_h264_support(true, false).unwrap();
    let capture_only = host.capabilities();
    assert!(capture_only.desktop_capture);
    assert!(!capture_only.keyboard);
    assert!(!capture_only.pointer);
    assert!(!capture_only.clipboard);
    assert!(!capture_only.file_transfer);
    assert!(!capture_only.audio);
    assert_eq!(capture_only.monitors, 1);

    let applied = Arc::new(Mutex::new(Vec::new()));
    host.set_desktop(Box::new(SizeDesktop {
        width: 100,
        height: 100,
        events: Arc::clone(&applied),
    }))
    .unwrap();
    let with_desktop = host.capabilities();
    assert!(with_desktop.desktop_capture);
    assert!(with_desktop.keyboard);
    assert!(with_desktop.pointer);
    assert_eq!(with_desktop.monitors, 1);

    pair_for_control(&host, &controller);
    let video = controller.subscribe_video().unwrap();
    let host_events = host.subscribe().unwrap();
    let session = controller.connect(&host.endpoint_id()).unwrap();
    assert_eq!(session.phase, SessionPhase::Active);
    assert_eq!((session.monitor_width, session.monitor_height), (1280, 720));
    let incoming = host.snapshot().unwrap().incoming.expect("hosted session");
    assert_eq!(
        (incoming.monitor_width, incoming.monitor_height),
        (1280, 720)
    );
    assert!(matches!(
        host.set_capture(Box::new(SizeCapture {
            width: 1,
            height: 1,
        })),
        Err(Error::Busy)
    ));

    host.publish_video(session.id, EncodedFrame::h264(true, 1, vec![1]).unwrap())
        .unwrap();
    host.publish_video(session.id, EncodedFrame::h264(false, 2, vec![2]).unwrap())
        .unwrap();
    let first = video
        .recv_timeout(Duration::from_secs(5))
        .expect("keyframe");
    assert!(first.keyframe);
    assert_eq!(first.timestamp_us, 1);
    assert_eq!(first.payload, vec![1]);
    let second = video.recv_timeout(Duration::from_secs(5)).expect("delta");
    assert!(!second.keyframe);
    assert_eq!(second.timestamp_us, 2);
    assert_eq!(second.payload, vec![2]);

    for index in 0..30u64 {
        host.publish_video(
            session.id,
            EncodedFrame::h264(false, 100 + index, vec![3]).unwrap(),
        )
        .unwrap();
    }
    host.publish_video(
        session.id,
        EncodedFrame::h264(true, 999, vec![9, 9]).unwrap(),
    )
    .unwrap();

    let deadline = Instant::now() + Duration::from_secs(5);
    let mut final_frame = None;
    while Instant::now() < deadline {
        match video.recv_timeout(Duration::from_millis(200)) {
            Some(frame) if frame.timestamp_us == 999 => {
                final_frame = Some(frame);
                break;
            }
            Some(_) => {}
            None => {}
        }
    }
    let final_frame = final_frame.expect("final keyframe");
    assert!(final_frame.keyframe);
    assert_eq!(final_frame.payload, vec![9, 9]);
    assert!(video.try_recv().is_none());

    let event_deadline = Instant::now() + Duration::from_secs(2);
    let mut saw_keyframe = false;
    while Instant::now() < event_deadline {
        if let Some(CoreEvent::KeyframeRequested { id }) =
            host_events.recv_timeout(Duration::from_millis(50))
        {
            assert_eq!(id, session.id);
            saw_keyframe = true;
            break;
        }
    }
    assert!(saw_keyframe, "host did not see a keyframe request");

    controller
        .submit_input(
            session.id,
            session.generation,
            InputEvent::key_down(usage::A),
        )
        .unwrap();
    wait_applied(&applied, &[InputEvent::key_down(usage::A)]);

    let mut empty = EncodedFrame::h264(true, 1, vec![1]).unwrap();
    empty.payload.clear();
    assert!(matches!(
        host.publish_video(session.id, empty),
        Err(Error::InvalidSetting("encoded frame is empty"))
    ));
    assert!(matches!(
        host.publish_video(
            session.id + 1,
            EncodedFrame::h264(true, 1, vec![1]).unwrap()
        ),
        Err(Error::NoSession)
    ));

    controller.disconnect().unwrap();
    wait_closed(&host, SessionRole::Host);
    assert!(matches!(
        host.publish_video(session.id, EncodedFrame::h264(true, 1, vec![1]).unwrap()),
        Err(Error::NoSession)
    ));
}

#[test]
fn a_zero_capture_size_falls_back_to_the_desktop() {
    let host = start_direct();
    let controller = start_direct();
    host.set_capture(Box::new(SizeCapture {
        width: 0,
        height: 0,
    }))
    .unwrap();
    host.set_h264_support(true, false).unwrap();
    host.set_desktop(Box::new(SizeDesktop {
        width: 100,
        height: 80,
        events: Arc::new(Mutex::new(Vec::new())),
    }))
    .unwrap();
    let capabilities = host.capabilities();
    assert!(capabilities.desktop_capture);
    assert!(capabilities.keyboard);
    assert_eq!(capabilities.monitors, 1);
    pair_for_control(&host, &controller);
    let session = controller.connect(&host.endpoint_id()).unwrap();
    assert_eq!((session.monitor_width, session.monitor_height), (100, 80));
}

fn frame(keyframe: bool, timestamp_us: u64) -> EncodedFrame {
    EncodedFrame::h264(keyframe, timestamp_us, vec![1]).unwrap()
}

#[test]
fn an_overflow_episode_requests_one_keyframe_until_it_arrives() {
    let mut queue = VideoQueue::new();
    assert_eq!(queue.push(frame(true, 1)), VideoPush::Queued);
    // Fill to the bound with predicted frames.
    for index in 0..VIDEO_QUEUE - 1 {
        assert_eq!(
            queue.push(frame(false, 2 + index as u64)),
            VideoPush::Queued
        );
    }
    // The next predicted frame overflows: exactly one keyframe request for
    // the whole episode, then silence until the keyframe arrives.
    assert_eq!(queue.push(frame(false, 99)), VideoPush::KeyframeNeeded);
    for timestamp in 100..110u64 {
        assert_eq!(
            queue.push(frame(false, timestamp)),
            VideoPush::StillNeedsKeyframe
        );
    }
    assert_eq!(queue.push(frame(true, 110)), VideoPush::Queued);
    assert_eq!(queue.pop().unwrap().timestamp_us, 110);
}

#[test]
fn predicted_frames_drop_until_the_next_keyframe() {
    let mut queue = VideoQueue::new();
    assert_eq!(queue.push(frame(true, 1)), VideoPush::Queued);
    for index in 0..VIDEO_QUEUE - 1 {
        assert_eq!(
            queue.push(frame(false, 2 + index as u64)),
            VideoPush::Queued
        );
    }
    assert_eq!(queue.len(), VIDEO_QUEUE);
    assert_eq!(queue.push(frame(false, 99)), VideoPush::KeyframeNeeded);
    assert_eq!(queue.len(), 1);
    let kept = queue.pop().unwrap();
    assert!(kept.keyframe);
    assert_eq!(kept.timestamp_us, 1);
    assert!(queue.dropped() >= 1);
    assert_eq!(queue.push(frame(false, 100)), VideoPush::StillNeedsKeyframe);
    assert_eq!(queue.push(frame(true, 101)), VideoPush::Queued);
    assert_eq!(queue.len(), 1);
    let dropped = queue.dropped();
    assert_eq!(queue.push(frame(true, 102)), VideoPush::Queued);
    assert!(queue.dropped() >= dropped + 1);
    assert_eq!(queue.len(), 1);
    let only = queue.pop().unwrap();
    assert!(only.keyframe);
    assert_eq!(only.timestamp_us, 102);
    assert!(queue.pop().is_none());
}

#[test]
fn h264_frames_round_trip_and_reject_a_bad_payload() {
    assert!(matches!(
        EncodedFrame::h264(true, 1, Vec::new()),
        Err(Error::InvalidSetting("encoded frame is empty"))
    ));
    assert!(matches!(
        EncodedFrame::h264(true, 1, vec![0; MAX_VIDEO_FRAME + 1]),
        Err(Error::InvalidSetting("encoded frame is larger than 1 MiB"))
    ));
    let small = EncodedFrame::h264(false, u64::MAX, vec![0x7F]).unwrap();
    assert_eq!(
        decode_video_body(&encode_video_body(&small)).unwrap(),
        small
    );

    let mut flags = encode_video_body(&EncodedFrame::h264(true, 7, vec![4]).unwrap());
    flags[1] = 0xFF;
    assert!(decode_video_body(&flags).unwrap().keyframe);
    flags[1] = 0x02;
    assert!(!decode_video_body(&flags).unwrap().keyframe);

    assert!(matches!(
        decode_video_body(&[9, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1]),
        Err(Error::Unsupported("this video codec"))
    ));
    assert!(matches!(decode_video_body(&[1, 0]), Err(Error::Malformed)));
    assert!(matches!(
        decode_video_body(&[1, 1, 0, 0, 0, 0, 0, 0, 0, 0]),
        Err(Error::Malformed)
    ));

    let mut mutated = EncodedFrame::h264(true, 1, vec![0xAB, 0xCD]).unwrap();
    let rendered = format!("{mutated:?}");
    assert!(rendered.contains("payload_len"));
    assert!(!rendered.contains("171"));
    assert!(!rendered.contains("205"));
    mutated.payload.clear();
    assert!(matches!(
        mutated.check(),
        Err(Error::InvalidSetting("encoded frame is empty"))
    ));
    mutated.payload.resize(MAX_VIDEO_FRAME + 1, 0);
    assert!(matches!(
        mutated.check(),
        Err(Error::InvalidSetting("encoded frame is larger than 1 MiB"))
    ));
}
