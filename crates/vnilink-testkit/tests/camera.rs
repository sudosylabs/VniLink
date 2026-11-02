// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! The Camera service alone: a host's camera viewed from the controller.

#[path = "camera/ending.rs"]
mod ending;
#[path = "camera/opening.rs"]
mod opening;
#[path = "camera/startup.rs"]
mod startup;

use expect_test::expect;
use std::sync::Arc;
use std::time::Duration;

use vnilink::camera::{CameraConfig, CameraEnd, HostCameraFactory};
use vnilink::{Error, ServiceKind};
use vnilink_testkit::camera::{config, CameraHost};
use vnilink_testkit::desktop::{delta, keyframe};
use vnilink_testkit::session::service_ended;
use vnilink_testkit::{traced, wait_for, TestResult, DEFAULT_WAIT};

/// Why `camera` ended, once it says; it says it once, and its owner on the
/// controller is finished by then.
fn ended(
    controller: &vnilink::Core,
    camera: &vnilink::RemoteCameraService,
) -> Result<CameraEnd, Error> {
    let end = wait_for("the camera to say why it ended", DEFAULT_WAIT, || {
        camera.take_end()
    });
    assert_eq!(camera.take_end(), None, "the reason is reported once");
    let owner = controller.controller_service(camera.info().id)?;
    assert!(
        service_ended(&owner.phase),
        "an ended camera still holds its owner: {owner:?}"
    );
    Ok(end)
}

/// A camera opens once per owner at the size asked, pictures start at the
/// first keyframe, a large one arrives whole, and the opening is recorded
/// with the size the camera opened at.
#[test]
fn pictures_start_at_a_keyframe_and_arrive_whole() -> TestResult {
    let trace = traced();
    let host = CameraHost::start()?;
    let controller = &host.pair.controller.core;
    let owner = host.pair.attach(&host.connection, ServiceKind::Camera);
    let camera = controller.open_camera_service(owner.id, owner.generation, config())?;
    assert!(
        matches!(
            controller.open_camera_service(owner.id, owner.generation, config()),
            Err(Error::Busy)
        ),
        "an owner opens its camera once"
    );
    assert_eq!(
        camera.config(),
        config(),
        "the camera opens at the size asked"
    );
    let source = host.cameras.latest();
    assert!(
        source.take_keyframe_request(),
        "a new viewer asks for a keyframe"
    );
    source.publish(delta(1, vec![1; 5]))?;
    assert!(
        camera.recv_timeout(Duration::from_millis(100)).is_none(),
        "a delta before the keyframe was shown"
    );
    let large = keyframe(2, vec![7; 512 * 1024]);
    source.publish(large.clone())?;
    assert_eq!(
        camera.recv_timeout(Duration::from_secs(5)),
        Some(large),
        "a large keyframe arrives whole"
    );
    vnilink_testkit::assert_story(
        expect![[r#"
        host:
          camera: camera opened fps=30 height=480 service=1 width=640
    "#]],
        || trace.story(&["camera"]),
    );
    Ok(())
}

/// The viewer learns why its picture stopped, once, and its owner is
/// finished: the host's camera failed or stopped delivering, the host
/// stopped sharing it or turned it off, or the host went away. Each end is
/// recorded with its reason.
#[test]
fn the_viewer_learns_why_the_picture_stopped() -> TestResult {
    let trace = traced();
    let host = CameraHost::start()?;
    let (hosting, controller) = (&host.pair.host.core, &host.pair.controller.core);
    let cameras: Arc<dyn HostCameraFactory> = host.cameras.clone();
    let open = || -> Result<_, Error> {
        hosting.set_service_sharing(ServiceKind::Camera, true)?;
        hosting.set_camera_factory(Some(Arc::clone(&cameras)))?;
        let camera = host.open(config())?;
        let source = host.first_picture(&camera)?;
        assert_eq!(camera.take_end(), None, "a camera that shows has not ended");
        Ok((camera, source))
    };

    let (camera, source) = open()?;
    source.fail();
    assert_eq!(
        ended(controller, &camera)?,
        CameraEnd::Stopped,
        "a failed capture stops the picture"
    );
    let (camera, source) = open()?;
    source.close();
    assert_eq!(
        ended(controller, &camera)?,
        CameraEnd::Stopped,
        "a capture that stops delivering stops the picture"
    );

    let (camera, _) = open()?;
    hosting.set_service_sharing(ServiceKind::Camera, false)?;
    assert_eq!(
        ended(controller, &camera)?,
        CameraEnd::Withdrawn,
        "a camera the host stops sharing is withdrawn"
    );
    let (camera, _) = open()?;
    hosting.turn_off_camera()?;
    assert_eq!(
        ended(controller, &camera)?,
        CameraEnd::Withdrawn,
        "a camera the host turns off is withdrawn"
    );

    // The host's side before it goes away; its last end follows below.
    trace.wait_for_records("camera stream ended", 4);
    vnilink_testkit::assert_story(
        expect![[r#"
        host:
          camera: camera opened fps=30 height=480 service=1 width=640
          camera: camera capture ended failed=true sent=1 service=1
          camera: camera stream ended ended=capture failed service=1
          camera: camera opened fps=30 height=480 service=2 width=640
          camera: camera capture ended failed=false sent=1 service=2
          camera: camera stream ended ended=capture ended service=2
          camera: camera opened fps=30 height=480 service=3 width=640
          camera: camera stream ended ended=service closed service=3
          camera: camera opened fps=30 height=480 service=4 width=640
          camera: camera stream ended ended=service closed service=4
    "#]],
        || trace.tell(&["camera"]).of("host").to_string(),
    );

    let (camera, _) = open()?;
    hosting.shutdown();
    assert_eq!(
        ended(controller, &camera)?,
        CameraEnd::Lost,
        "a host that goes away loses the picture"
    );
    trace.wait_for_records("camera picture ended", 5);
    vnilink_testkit::assert_story(
        expect![[r#"
        controller:
          camera: camera picture ended closed_here=false reason=Stopped service=1
          camera: camera picture ended closed_here=false reason=Stopped service=2
          camera: camera picture ended closed_here=false reason=Withdrawn service=3
          camera: camera picture ended closed_here=false reason=Withdrawn service=4
          camera: camera picture ended closed_here=false reason=Lost service=5
    "#]],
        || trace.tell(&["camera"]).of("controller").to_string(),
    );
    // The host records the end of the camera it was serving as it shut down.
    trace.wait_for_records("camera stream ended", 5);
    vnilink_testkit::assert_story(
        expect![[r#"
        host:
          camera: camera opened fps=30 height=480 service=1 width=640
          camera: camera capture ended failed=true sent=1 service=1
          camera: camera stream ended ended=capture failed service=1
          camera: camera opened fps=30 height=480 service=2 width=640
          camera: camera capture ended failed=false sent=1 service=2
          camera: camera stream ended ended=capture ended service=2
          camera: camera opened fps=30 height=480 service=3 width=640
          camera: camera stream ended ended=service closed service=3
          camera: camera opened fps=30 height=480 service=4 width=640
          camera: camera stream ended ended=service closed service=4
          camera: camera opened fps=30 height=480 service=5 width=640
          camera: camera stream ended ended=shut down service=5
    "#]],
        || trace.tell(&["camera"]).of("host").to_string(),
    );
    Ok(())
}

/// A camera asked for at a size it cannot have, or one the host does not
/// have, is refused, nothing starts, and the connection stays.
#[test]
fn an_unknown_camera_or_an_invalid_size_is_refused_and_nothing_starts() -> TestResult {
    let trace = traced();
    let host = CameraHost::start()?;
    let controller = &host.pair.controller.core;
    let owner = host.pair.attach(&host.connection, ServiceKind::Camera);
    assert!(
        matches!(
            controller.open_camera_service(
                owner.id,
                owner.generation,
                CameraConfig { fps: 0, ..config() }
            ),
            Err(Error::InvalidSetting(_))
        ),
        "a camera at no frame rate is refused as an invalid setting"
    );
    assert!(
        matches!(
            controller.open_camera_service(
                owner.id,
                owner.generation,
                CameraConfig {
                    device_id: "no-such-camera".into(),
                    ..config()
                }
            ),
            Err(Error::Unsupported(_))
        ),
        "a camera the host does not have is refused as unsupported"
    );
    assert!(
        host.cameras.opened() == 0,
        "a refused camera starts no capture"
    );
    assert!(
        host.connection_active(),
        "a refused camera keeps the connection"
    );
    trace.wait_for_records("camera not opened", 1);
    vnilink_testkit::assert_story(
        expect![[r#"
        host:
          camera: camera not opened error=unsupported reason=camera device service=1
          camera: camera stream ended ended=unsupported service=1
    "#]],
        || trace.story(&["camera"]),
    );
    Ok(())
}
