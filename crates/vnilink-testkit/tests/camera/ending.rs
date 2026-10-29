//! How a camera ends on the host's side: its viewer closes it, the host
//! withdraws it or its backend, or its capture fails; each time capture
//! stops and the connection stays.

use expect_test::expect;
use std::sync::Arc;

use vnilink::{Capability, Error, RemoteStatus, ServiceKind, ServicePhase};
use vnilink_testkit::camera::{config, CameraHost, StandInCameras};
use vnilink_testkit::desktop::keyframe;
use vnilink_testkit::{traced, wait_for, Pair, TestResult, DEFAULT_WAIT};

use super::{connection_active, first_picture, unshared};

/// A viewer that closes its camera stops the host's capture, and the
/// connection and its other services stay.
#[test]
fn closing_the_camera_stops_capture_and_leaves_the_connection() -> TestResult {
    let trace = traced();
    let host = unshared();
    host.pair
        .host
        .core
        .set_service_sharing(ServiceKind::Camera, true)?;
    let files = host.pair.attach(&host.connection, ServiceKind::Files);
    let files = host
        .pair
        .controller
        .core
        .service_lease(files.id, files.generation)?;
    let camera = host.open(config())?;
    let source = first_picture(&host, &camera);
    camera.close();
    wait_for("the host to stop capturing", DEFAULT_WAIT, || {
        source.is_cancelled().then_some(())
    });
    assert!(
        !files.is_cancelled(),
        "closing the camera leaves Files open"
    );
    assert!(
        connection_active(&host),
        "closing the camera keeps the connection"
    );
    assert!(
        matches!(
            source.publish(keyframe(3, vec![7; 1])),
            Err(Error::NoSession)
        ),
        "a closed camera takes no more pictures"
    );
    trace.wait_for_records("camera picture ended", 1);
    vnilink_testkit::assert_story(
        expect![[r#"
        controller:
          camera: camera picture ended closed_here=true service=2
        host:
          camera: camera opened fps=30 height=480 service=1 width=640
          camera: camera stream ended ended=viewer closed service=1
    "#]],
        || trace.story(&["camera"]),
    );
    Ok(())
}

/// A host that stops sharing its camera, or removes its camera backend,
/// stops the viewer's camera and its capture, and keeps the connection;
/// without a backend the host no longer offers a camera.
#[test]
fn withdrawing_the_camera_or_its_backend_stops_capture_and_keeps_the_connection() -> TestResult {
    let trace = traced();
    let host = CameraHost::start();
    let hosting = &host.pair.host.core;
    for withdraw_sharing in [true, false] {
        hosting.set_service_sharing(ServiceKind::Camera, true)?;
        let camera = host.open(config())?;
        if withdraw_sharing {
            hosting.set_service_sharing(ServiceKind::Camera, false)?;
        } else {
            hosting.set_camera_factory(None)?;
        }
        wait_for("the viewer's camera to stop", DEFAULT_WAIT, || {
            camera.is_cancelled().then_some(())
        });
        wait_for("the host to stop capturing", DEFAULT_WAIT, || {
            host.cameras.latest().is_cancelled().then_some(())
        });
        assert!(
            connection_active(&host),
            "a withdrawn camera keeps the connection"
        );
    }
    assert!(
        !host
            .pair
            .controller
            .core
            .remote_service_support(host.connection.id, host.connection.generation)?
            .camera,
        "a host without a camera backend offers no camera"
    );
    trace.wait_for_records("camera picture ended", 2);
    vnilink_testkit::assert_story(
        expect![[r#"
        controller:
          camera: camera picture ended closed_here=false reason=Withdrawn service=1
          camera: camera picture ended closed_here=false reason=Withdrawn service=2
        host:
          camera: camera opened fps=30 height=480 service=1 width=640
          camera: camera stream ended ended=service closed service=1
          camera: camera opened fps=30 height=480 service=2 width=640
          camera: camera stream ended ended=service closed service=2
    "#]],
        || trace.story(&["camera"]),
    );
    Ok(())
}

/// A host whose capture fails fails the viewer's camera service, and the
/// connection stays.
#[test]
fn a_failed_capture_fails_the_camera_and_keeps_the_connection() -> TestResult {
    let trace = traced();
    let host = CameraHost::start();
    let controller = &host.pair.controller.core;
    let camera = host.open(config())?;
    host.cameras.latest().fail();
    wait_for("the camera service to fail", DEFAULT_WAIT, || {
        matches!(
            controller.controller_service(camera.info().id).ok()?.phase,
            ServicePhase::Failed(_)
        )
        .then_some(())
    });
    assert!(
        camera.is_cancelled(),
        "a failed camera service ends its camera"
    );
    assert!(
        connection_active(&host),
        "a failed camera keeps the connection"
    );
    trace.wait_for_records("camera picture ended", 1);
    vnilink_testkit::assert_story(
        expect![[r#"
        controller:
          camera: camera picture ended closed_here=false reason=Stopped service=1
        host:
          camera: camera opened fps=30 height=480 service=1 width=640
          camera: camera capture ended failed=true sent=0 service=1
          camera: camera stream ended ended=capture failed service=1
    "#]],
        || trace.story(&["camera"]),
    );
    Ok(())
}

/// A peer sees the host's camera as unsupported without a backend,
/// available with one, and turned off once the host's user turns it off,
/// until a backend is set again.
#[test]
fn a_camera_turned_off_by_its_user_reads_as_turned_off() -> TestResult {
    let _trace = traced();
    let pair = Pair::for_services(&[ServiceKind::Camera]);
    let (host, controller) = (&pair.host.core, &pair.controller.core);
    controller.set_presence_links(true)?;
    let peer = pair.host.endpoint();
    let reads = |what: &str, status: RemoteStatus| {
        wait_for(what, DEFAULT_WAIT, || {
            let camera = controller
                .peer_availability(&peer)
                .ok()??
                .into_iter()
                .find(|(capability, _)| *capability == Capability::Camera)?
                .1;
            (camera == status).then_some(())
        })
    };
    reads(
        "a host without a camera backend to read as unsupported",
        RemoteStatus::Unsupported,
    );
    host.set_camera_factory(Some(Arc::new(StandInCameras::default())))?;
    reads(
        "a host with a camera to read as available",
        RemoteStatus::Available,
    );
    host.turn_off_camera()?;
    reads(
        "a camera its user turned off to read as turned off",
        RemoteStatus::TurnedOff,
    );
    host.set_camera_factory(Some(Arc::new(StandInCameras::default())))?;
    reads(
        "a camera set again to read as available",
        RemoteStatus::Available,
    );
    Ok(())
}
