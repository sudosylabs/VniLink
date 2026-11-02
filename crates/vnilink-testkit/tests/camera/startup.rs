// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! A camera whose backend is still starting when its grant, its owner, its
//! connection or its host goes away: the backend learns it is no longer
//! wanted, rather than waiting out its startup, without being released from
//! outside, and a backend that opens anyway is closed unused.

use expect_test::{expect, Expect};
use std::sync::mpsc::{channel, sync_channel, Receiver, SyncSender};
use std::sync::{Arc, Mutex};

use vnilink::camera::{CameraBackend, CameraConfig, CameraDevice, CameraSource, HostCameraFactory};
use vnilink::{BackendStartup, Error, ServiceKind};
use vnilink_testkit::camera::{self, config, HeldStartup, StandInCameras};
use vnilink_testkit::desktop::keyframe;
use vnilink_testkit::terminal::Held;
use vnilink_testkit::{lock, traced, wait_for, TestResult, Traced, DEFAULT_WAIT};

/// What ends a camera's wanting while its backend starts.
enum Cause {
    HostStopsSharing,
    HostStopsServingThePeer,
    HostShutsDown,
    ViewerCloses,
}

/// Opens the camera on a host whose backend never finishes starting, and
/// once it is starting, `cause` ends the camera's wanting. The backend's
/// startup learns it was cancelled, not that its own deadline passed, and
/// the open fails; a cause that leaves the connection leaves it active,
/// the host serving nothing. The host's camera records are `expect`.
#[track_caller]
fn check(trace: &Traced, cause: Cause, expect: Expect) -> TestResult {
    let held = HeldStartup::new();
    let (pair, connection) = camera::connect(true, Arc::clone(&held) as _)?;
    let owner = pair.attach(&connection, ServiceKind::Camera);
    std::thread::scope(|scope| -> TestResult {
        let opening = scope.spawn(|| {
            pair.controller
                .core
                .open_camera_service(owner.id, owner.generation, config())
        });
        held.wait_holding();
        match cause {
            Cause::HostStopsSharing => pair
                .host
                .core
                .set_service_sharing(ServiceKind::Camera, false)?,
            Cause::HostStopsServingThePeer => {
                let peer = pair.host.core.hosted_sessions()?[0].id;
                pair.host.core.stop_hosted_peer(peer)?;
            }
            Cause::HostShutsDown => pair.host.core.shutdown(),
            Cause::ViewerCloses => pair
                .controller
                .core
                .close_controller_service(owner.id, owner.generation)?,
        }
        assert_eq!(
            held.wait_finished(),
            Held::Cancelled,
            "a starting backend whose camera is no longer wanted is cancelled"
        );
        assert!(
            opening.join().map_err(|_| "the open panicked")?.is_err(),
            "a camera whose startup stopped does not open"
        );
        Ok(())
    })?;
    if matches!(cause, Cause::HostStopsSharing | Cause::ViewerCloses) {
        pair.host.wait_released(None);
        assert!(
            camera::connection_active(&pair.controller, &connection),
            "a camera that did not start keeps the connection"
        );
    }
    vnilink_testkit::assert_story(expect, || trace.story(&["camera"]));
    Ok(())
}

#[test]
fn a_starting_camera_stops_when_the_host_stops_sharing_it() -> TestResult {
    let trace = traced();
    check(
        &trace,
        Cause::HostStopsSharing,
        expect![[r#"
        host:
          camera: camera stream ended ended=service closed service=1
    "#]],
    )
}

#[test]
fn a_starting_camera_stops_when_the_host_stops_serving_its_peer() -> TestResult {
    let trace = traced();
    check(
        &trace,
        Cause::HostStopsServingThePeer,
        expect![[r#"
        host:
          camera: camera stream ended ended=connection ended service=1
    "#]],
    )
}

#[test]
fn a_starting_camera_stops_when_the_host_shuts_down() -> TestResult {
    let trace = traced();
    check(
        &trace,
        Cause::HostShutsDown,
        expect![[r#"
        host:
          camera: camera stream ended ended=shut down service=1
    "#]],
    )
}

#[test]
fn a_starting_camera_stops_when_its_viewer_closes_it() -> TestResult {
    let trace = traced();
    check(
        &trace,
        Cause::ViewerCloses,
        expect![[r#"
        host:
          camera: camera not opened error=no session service=1
          camera: camera stream ended ended=viewer closed service=1
    "#]],
    )
}

/// A camera that opens on the host only once the camera is no longer
/// wanted, as an operating system call that finishes late.
struct OpensLate {
    entered: SyncSender<()>,
    release: Mutex<Receiver<()>>,
    source: Arc<CameraSource>,
}

impl HostCameraFactory for OpensLate {
    fn devices(&self) -> Result<Vec<CameraDevice>, Error> {
        StandInCameras::default().devices()
    }

    fn open(&self, config: CameraConfig, _: BackendStartup) -> Result<CameraBackend, Error> {
        self.entered.send(()).expect("the test waits for the open");
        lock(&self.release)
            .recv_timeout(DEFAULT_WAIT)
            .expect("the test releases the open");
        CameraBackend::new(config, Arc::clone(&self.source))
    }
}

/// A camera that opens after the host stopped sharing it is closed without
/// being admitted: the viewer's open fails, the late capture stops and
/// takes no pictures, and the connection stays.
#[test]
fn a_camera_that_opens_after_its_grant_is_withdrawn_is_closed_unused() -> TestResult {
    let trace = traced();
    let (entered, entering) = sync_channel(1);
    let (release, released) = channel();
    let source = CameraSource::new();
    let (pair, connection) = camera::connect(
        true,
        Arc::new(OpensLate {
            entered,
            release: Mutex::new(released),
            source: Arc::clone(&source),
        }),
    )?;
    let owner = pair.attach(&connection, ServiceKind::Camera);
    std::thread::scope(|scope| -> TestResult {
        let opening = scope.spawn(|| {
            pair.controller
                .core
                .open_camera_service(owner.id, owner.generation, config())
        });
        entering.recv_timeout(DEFAULT_WAIT)?;
        pair.host
            .core
            .set_service_sharing(ServiceKind::Camera, false)?;
        pair.host.wait_released(None);
        release.send(())?;
        assert!(
            opening.join().map_err(|_| "the open panicked")?.is_err(),
            "a camera that opened after its grant was withdrawn is not admitted"
        );
        Ok(())
    })?;
    wait_for("the late capture to stop", DEFAULT_WAIT, || {
        source.is_cancelled().then_some(())
    });
    assert!(
        source.publish(keyframe(1, vec![7; 1])).is_err(),
        "a late capture takes no pictures"
    );
    assert!(
        camera::connection_active(&pair.controller, &connection),
        "a camera that opened late keeps the connection"
    );
    vnilink_testkit::assert_story(
        expect![[r#"
        host:
          camera: camera stream ended ended=service closed service=1
    "#]],
        || trace.story(&["camera"]),
    );
    Ok(())
}
