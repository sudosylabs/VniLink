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
use std::time::Duration;

use vnilink::camera::CameraConfig;
use vnilink::{Error, ServiceKind};
use vnilink_testkit::camera::{config, CameraHost};
use vnilink_testkit::desktop::{delta, keyframe};
use vnilink_testkit::{traced, TestResult};

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
