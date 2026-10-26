//! The Camera service alone: a host's camera viewed from the controller.

use expect_test::expect;
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
    expect![[r#"
        host:
          camera: camera opened fps=30 height=480 service=1 width=640
    "#]]
    .assert_eq(&trace.story(&["camera"]));
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

    expect![[r#"
        controller:
          camera: camera picture ended closed_here=false reason=Stopped service=1
          camera: camera picture ended closed_here=false reason=Withdrawn service=2
        host:
          camera: camera opened fps=30 height=480 service=1 width=640
          camera: camera capture ended failed=true sent=1 service=1
          camera: camera stream ended service=1 withdrawn=false
          camera: camera opened fps=30 height=480 service=2 width=640
          camera: camera stream ended ended=no session service=2 withdrawn=true
    "#]]
    .assert_eq(&trace.story(&["camera"]));
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
    wait_for("the refusal to be recorded", DEFAULT_WAIT, || {
        (!trace.named("camera not opened").is_empty()).then_some(())
    });
    expect![[r#"
        host:
          camera: camera not opened error=unsupported reason=camera device service=1
          camera: camera stream ended ended=unsupported service=1 withdrawn=false
    "#]]
    .assert_eq(&trace.story(&["camera"]));
}
