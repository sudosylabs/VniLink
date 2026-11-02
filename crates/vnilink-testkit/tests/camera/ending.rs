// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! How a camera that shows ends: its viewer closes it, the host withdraws
//! it, turns it off, removes its backend or goes away, or its capture fails
//! or stops delivering. Each time capture stops, the viewer learns why once,
//! and the connection stays unless the host went away.

use expect_test::expect;
use std::sync::Arc;

use vnilink::{Capability, Error, RemoteStatus, ServiceKind, ServicePhase};
use vnilink_testkit::camera::{check_ending, CameraHost, Ending, StandInCameras};
use vnilink_testkit::desktop::keyframe;
use vnilink_testkit::{traced, wait_for, Pair, TestResult, DEFAULT_WAIT};

/// A viewer that closes its camera stops the host's capture, learns no
/// reason from the host, and the connection and its other services stay.
#[test]
fn closing_the_camera_stops_capture_and_leaves_the_connection() -> TestResult {
    let trace = traced();
    let host = CameraHost::start()?;
    let files = host.pair.attach(&host.connection, ServiceKind::Files);
    let files = host
        .pair
        .controller
        .core
        .service_lease(files.id, files.generation)?;
    let ended = check_ending(
        &trace,
        &host,
        Ending::ViewerCloses,
        expect![[r#"
        viewer learned: None
        controller:
          camera: camera picture ended closed_here=true service=2
        host:
          camera: camera opened fps=30 height=480 service=1 width=640
          camera: camera stream ended ended=viewer closed service=1
    "#]],
    )?;
    assert!(
        !files.is_cancelled(),
        "closing the camera leaves Files open"
    );
    assert!(
        matches!(
            ended.source.publish(keyframe(3, vec![7; 1])),
            Err(Error::NoSession)
        ),
        "a closed camera takes no more pictures"
    );
    Ok(())
}

/// A host that stops sharing its camera withdraws the viewer's camera and
/// stops its capture, and keeps the connection.
#[test]
fn a_camera_the_host_stops_sharing_is_withdrawn() -> TestResult {
    let trace = traced();
    let host = CameraHost::start()?;
    check_ending(
        &trace,
        &host,
        Ending::HostStopsSharing,
        expect![[r#"
        viewer learned: Some(Withdrawn)
        controller:
          camera: camera picture ended closed_here=false reason=Withdrawn service=1
        host:
          camera: camera opened fps=30 height=480 service=1 width=640
          camera: camera stream ended ended=service closed service=1
    "#]],
    )?;
    Ok(())
}

/// A host that removes its camera backend withdraws the viewer's camera,
/// stops its capture and keeps the connection, and no longer offers a
/// camera.
#[test]
fn removing_the_camera_backend_withdraws_the_camera_and_offers_none() -> TestResult {
    let trace = traced();
    let host = CameraHost::start()?;
    check_ending(
        &trace,
        &host,
        Ending::HostRemovesBackend,
        expect![[r#"
        viewer learned: Some(Withdrawn)
        controller:
          camera: camera picture ended closed_here=false reason=Withdrawn service=1
        host:
          camera: camera opened fps=30 height=480 service=1 width=640
          camera: camera stream ended ended=service closed service=1
    "#]],
    )?;
    assert!(
        !host
            .pair
            .controller
            .core
            .remote_service_support(host.connection.id, host.connection.generation)?
            .camera,
        "a host without a camera backend offers no camera"
    );
    Ok(())
}

/// A camera its host's user turns off is withdrawn, and the connection
/// stays.
#[test]
fn a_camera_the_host_turns_off_is_withdrawn() -> TestResult {
    let trace = traced();
    let host = CameraHost::start()?;
    check_ending(
        &trace,
        &host,
        Ending::HostTurnsOff,
        expect![[r#"
        viewer learned: Some(Withdrawn)
        controller:
          camera: camera picture ended closed_here=false reason=Withdrawn service=1
        host:
          camera: camera opened fps=30 height=480 service=1 width=640
          camera: camera stream ended ended=service closed service=1
    "#]],
    )?;
    Ok(())
}

/// A host whose capture fails stops the picture and fails the viewer's
/// camera service, and the connection stays.
#[test]
fn a_failed_capture_fails_the_camera_and_keeps_the_connection() -> TestResult {
    let trace = traced();
    let host = CameraHost::start()?;
    let ended = check_ending(
        &trace,
        &host,
        Ending::CaptureFails,
        expect![[r#"
        viewer learned: Some(Stopped)
        controller:
          camera: camera picture ended closed_here=false reason=Stopped service=1
        host:
          camera: camera opened fps=30 height=480 service=1 width=640
          camera: camera capture ended failed=true sent=1 service=1
          camera: camera stream ended ended=capture failed service=1
    "#]],
    )?;
    let owner = host
        .pair
        .controller
        .core
        .controller_service(ended.camera.info().id)?;
    assert!(
        matches!(owner.phase, ServicePhase::Failed(_)),
        "a failed capture fails the camera service: {owner:?}"
    );
    Ok(())
}

/// A capture that stops delivering stops the picture, and the connection
/// stays.
#[test]
fn a_capture_that_stops_delivering_stops_the_picture() -> TestResult {
    let trace = traced();
    let host = CameraHost::start()?;
    check_ending(
        &trace,
        &host,
        Ending::CaptureEnds,
        expect![[r#"
        viewer learned: Some(Stopped)
        controller:
          camera: camera picture ended closed_here=false reason=Stopped service=1
        host:
          camera: camera opened fps=30 height=480 service=1 width=640
          camera: camera capture ended failed=false sent=1 service=1
          camera: camera stream ended ended=capture ended service=1
    "#]],
    )?;
    Ok(())
}

/// A host that goes away loses the viewer's picture, and records the end
/// of the camera it was serving as it shuts down.
#[test]
fn a_host_that_goes_away_loses_the_picture() -> TestResult {
    let trace = traced();
    let host = CameraHost::start()?;
    check_ending(
        &trace,
        &host,
        Ending::HostShutsDown,
        expect![[r#"
        viewer learned: Some(Lost)
        controller:
          camera: camera picture ended closed_here=false reason=Lost service=1
        host:
          camera: camera opened fps=30 height=480 service=1 width=640
          camera: camera stream ended ended=shut down service=1
    "#]],
    )?;
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
