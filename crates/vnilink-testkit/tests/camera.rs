//! The Camera service alone: a host's camera viewed from the controller.

use std::time::Duration;

use vnilink::camera::{CameraConfig, CameraEnd};
use vnilink::{EncodedFrame, ServiceKind};
use vnilink_testkit::camera::{config, CameraHost};
use vnilink_testkit::{traced, wait_for, DEFAULT_WAIT};

fn ended(camera: &vnilink::RemoteCameraService) -> CameraEnd {
    wait_for("the camera to say why it ended", DEFAULT_WAIT, || {
        camera.take_end()
    })
}

/// Pictures start at the first keyframe, a large one arrives whole, and the
/// opening is recorded with the size the camera opened at.
#[test]
fn pictures_start_at_a_keyframe_and_arrive_whole() {
    let trace = traced();
    let host = CameraHost::start();
    let camera = host.open(config()).unwrap();
    let source = host.cameras.latest();
    assert!(
        source.take_keyframe_request(),
        "a new viewer asks for a keyframe"
    );
    source
        .publish(EncodedFrame::h264(false, 1, vec![1; 5]).unwrap())
        .unwrap();
    assert!(
        camera.recv_timeout(Duration::from_millis(100)).is_none(),
        "a delta before the keyframe was shown"
    );
    let keyframe = EncodedFrame::h264(true, 2, vec![7; 512 * 1024]).unwrap();
    source.publish(keyframe.clone()).unwrap();
    assert_eq!(camera.recv_timeout(DEFAULT_WAIT), Some(keyframe));
    let opened = trace.named("camera opened");
    assert_eq!(opened.len(), 1, "{opened:?}");
    assert_eq!(opened[0]["width"], 640);
}

/// The viewer learns why its picture stopped: the camera stopped, or the
/// host stopped sharing it; each end is recorded with its reason.
#[test]
fn the_viewer_learns_why_the_picture_stopped() {
    let trace = traced();
    let host = CameraHost::start();
    let camera = host.open(config()).unwrap();
    let source = host.cameras.latest();
    source
        .publish(EncodedFrame::h264(true, 1, vec![7; 64]).unwrap())
        .unwrap();
    assert!(camera.recv_timeout(DEFAULT_WAIT).is_some());
    source.fail();
    assert_eq!(ended(&camera), CameraEnd::Stopped);

    let camera = host.open(config()).unwrap();
    host.cameras
        .latest()
        .publish(EncodedFrame::h264(true, 1, vec![7; 64]).unwrap())
        .unwrap();
    assert!(camera.recv_timeout(DEFAULT_WAIT).is_some());
    host.pair
        .host
        .core
        .set_service_sharing(ServiceKind::Camera, false)
        .unwrap();
    assert_eq!(ended(&camera), CameraEnd::Withdrawn);

    let ends: Vec<String> = trace
        .named("camera picture ended")
        .iter()
        .map(|event| event["reason"].as_str().unwrap_or_default().to_owned())
        .collect();
    assert_eq!(ends, ["Stopped", "Withdrawn"]);
    assert!(trace
        .named("camera capture ended")
        .iter()
        .any(|event| event["failed"] == true));
}

/// A camera the host does not have is refused, and nothing starts.
#[test]
fn an_unknown_camera_is_refused_and_nothing_starts() {
    let trace = traced();
    let host = CameraHost::start();
    let refused = host.open(CameraConfig {
        device_id: "no-such-camera".into(),
        ..config()
    });
    assert!(refused.is_err());
    assert!(host.cameras.sources.lock().unwrap().is_empty());
    let recorded = wait_for("the refusal to be recorded", DEFAULT_WAIT, || {
        let recorded = trace.named("camera not opened");
        (!recorded.is_empty()).then_some(recorded)
    });
    assert_eq!(recorded[0]["reason"], "camera device", "{recorded:?}");
}
