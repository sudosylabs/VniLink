//! Which cameras a viewer may open: the host's grant lists them, each opens
//! for one viewer at a time, a camera that fails to open says so where the
//! host's availability is read, and recording follows the host's consent.

use expect_test::expect;
use std::sync::Arc;
use std::time::Duration;

use vnilink::camera::{CameraBackend, CameraConfig, CameraDevice, CameraSource, HostCameraFactory};
use vnilink::{
    AvailabilityReason, BackendStartup, Capability, Error, RemoteCameraService, ServiceKind,
    SessionInfo,
};
use vnilink_testkit::camera::{config, CameraHost, StandInCameras};
use vnilink_testkit::{traced, wait_for, Engine, Pair, TestResult, DEFAULT_WAIT};

use super::{first_picture, keyframe, unshared};

/// The host's camera is offered before it is shared, but its cameras are
/// listed only once the host shares it.
#[test]
fn the_hosts_cameras_are_listed_only_once_it_shares_them() -> TestResult {
    let _trace = traced();
    let host = unshared();
    let controller = &host.pair.controller.core;
    let connection = &host.connection;
    assert!(
        controller
            .remote_service_support(connection.id, connection.generation)?
            .camera,
        "a host with a camera backend offers a camera"
    );
    assert!(
        matches!(
            controller.remote_camera_devices(connection.id, connection.generation),
            Err(Error::Unauthorized)
        ),
        "a camera the host does not share lists no cameras"
    );
    host.pair
        .host
        .core
        .set_service_sharing(ServiceKind::Camera, true)?;
    assert_eq!(
        controller.remote_camera_devices(connection.id, connection.generation)?,
        host.cameras.devices()?,
        "a shared camera lists the host's cameras"
    );
    Ok(())
}

/// The host's two cameras, each opened by one viewer at a time.
struct TwoCameras;

impl HostCameraFactory for TwoCameras {
    fn devices(&self) -> Result<Vec<CameraDevice>, Error> {
        Ok(["one", "two"]
            .map(|id| CameraDevice {
                id: id.into(),
                name: id.into(),
            })
            .into())
    }

    fn open(
        &self,
        requested: CameraConfig,
        startup: BackendStartup,
    ) -> Result<CameraBackend, Error> {
        startup.check()?;
        CameraBackend::new(requested, CameraSource::new())
    }
}

fn camera(id: &str) -> CameraConfig {
    CameraConfig {
        device_id: id.into(),
        ..config()
    }
}

/// Opens the camera `id` on `connection` under an owner of its own: a
/// refused open's owner is closed and forgotten, so the next attach never
/// races its cleanup.
fn open(
    controller: &Engine,
    connection: &SessionInfo,
    id: &str,
) -> Result<RemoteCameraService, Error> {
    let owner = controller.core.attach_controller_service(
        connection.id,
        connection.generation,
        ServiceKind::Camera,
    )?;
    controller
        .core
        .open_camera_service(owner.id, owner.generation, camera(id))
        .inspect_err(|_| {
            controller
                .core
                .close_controller_service(owner.id, owner.generation)
                .expect("the refused owner closes");
            controller
                .core
                .forget_controller_service(owner.id)
                .expect("the refused owner is forgotten");
        })
}

/// A camera opens for one viewer at a time: another viewer's open of it is
/// refused as busy while a different camera opens for it, and once closed
/// the camera opens for the other viewer.
#[test]
fn a_camera_opens_for_one_viewer_at_a_time_and_frees_on_close() -> TestResult {
    let trace = traced();
    let pair = Pair::for_services(&[ServiceKind::Camera]);
    pair.host
        .core
        .set_camera_factory(Some(Arc::new(TwoCameras)))?;
    let (first, second) = (&pair.controller, pair.add_viewer("second"));
    let (to_first, to_second) = (
        pair.connect_services(),
        second.connect_services(&pair.host)?,
    );
    let one = open(first, &to_first, "one")?;
    assert!(
        matches!(open(&second, &to_second, "one"), Err(Error::Busy)),
        "a camera another viewer has open is busy"
    );
    let two = open(&second, &to_second, "two")?;
    one.close();
    two.close();
    let owner = two.info();
    second
        .core
        .close_controller_service(owner.id, owner.generation)?;
    second.core.forget_controller_service(owner.id)?;
    // Each attempt opens under an owner of its own, since a refused open
    // consumes its owner.
    let taken = wait_for(
        "the closed camera to open for the other viewer",
        Duration::from_secs(8),
        || match open(&second, &to_second, "one") {
            Ok(camera) => Some(camera),
            Err(Error::Busy) => None,
            Err(other) => {
                panic!("a freed camera is refused for a reason other than busy: {other:?}")
            }
        },
    );
    taken.close();
    // The host's side alone: a viewer's record of a camera it closed is
    // not always written once its owner is forgotten.
    trace.wait_for_records("camera stream ended", 4);
    // Each viewer records the pictures it closed; the refused open has none.
    trace.wait_for_records("camera picture ended", 3);
    expect![[r#"
        controller:
          camera: camera picture ended closed_here=true service=1
        host:
          camera: camera opened fps=30 height=480 service=1 width=640
          camera: camera not opened error=busy service=2
          camera: camera stream ended ended=busy service=2
          camera: camera opened fps=30 height=480 service=3 width=640
          camera: camera stream ended ended=viewer closed service=1
          camera: camera stream ended ended=viewer closed service=3
          camera: camera opened fps=30 height=480 service=4 width=640
          camera: camera stream ended ended=viewer closed service=4
        second:
          camera: camera picture ended closed_here=true service=2
          camera: camera picture ended closed_here=true service=4
    "#]]
    .assert_eq(&trace.tell(&["camera"]).concurrent().to_string());
    Ok(())
}

/// The host's camera that cannot open.
struct Failing;

impl HostCameraFactory for Failing {
    fn devices(&self) -> Result<Vec<CameraDevice>, Error> {
        StandInCameras::default().devices()
    }

    fn open(&self, _: CameraConfig, _: BackendStartup) -> Result<CameraBackend, Error> {
        Err(Error::Unsupported("camera sensor"))
    }
}

/// A camera that fails to open reads as failed in the host's availability,
/// and one that opens later clears it.
#[test]
fn a_camera_that_failed_to_open_reads_as_failed_until_one_opens() -> TestResult {
    let trace = traced();
    let pair = Pair::for_services(&[ServiceKind::Camera]);
    let (host, controller) = (&pair.host.core, &pair.controller.core);
    host.set_camera_factory(Some(Arc::new(Failing)))?;
    let connection = pair.connect_services();
    let owner = pair.attach(&connection, ServiceKind::Camera);
    assert!(
        controller
            .open_camera_service(owner.id, owner.generation, config())
            .is_err(),
        "a camera that cannot open is refused"
    );
    let failed = host.capability_availability(None, Capability::Camera)?;
    assert!(
        !failed.available,
        "a camera that failed to open is not available"
    );
    assert_eq!(
        failed.reasons,
        [AvailabilityReason::BackendFailed {
            detail: "camera sensor is not available yet".into(),
        }],
        "the availability says the camera failed, and why"
    );
    host.set_camera_factory(Some(Arc::new(StandInCameras::default())))?;
    // The refused open leaves its owner behind; it is released before the
    // retry attaches, so the retry never races its cleanup.
    controller.close_controller_service(owner.id, owner.generation)?;
    controller.forget_controller_service(owner.id)?;
    let retry = pair.attach(&connection, ServiceKind::Camera);
    let camera = controller.open_camera_service(retry.id, retry.generation, config())?;
    camera.close();
    let recovered = host.capability_availability(None, Capability::Camera)?;
    assert!(
        recovered.available,
        "a camera that opened is available again: {recovered:?}"
    );
    trace.wait_for_records("camera picture ended", 1);
    expect![[r#"
        controller:
          camera: camera picture ended closed_here=true service=2
        host:
          camera: camera not opened error=unsupported reason=camera sensor service=1
          camera: camera stream ended ended=unsupported service=1
          camera: camera opened fps=30 height=480 service=2 width=640
          camera: camera stream ended ended=viewer closed service=2
    "#]]
    .assert_eq(&trace.story(&["camera"]));
    Ok(())
}

/// Recording a camera is the host's to allow and starts off; once allowed,
/// a recording begins on a keyframe it asks the host for, the viewer keeps
/// its pictures, and withdrawing consent ends the recording but not the
/// camera.
#[test]
fn recording_a_camera_follows_the_hosts_consent() -> TestResult {
    let trace = traced();
    let host = CameraHost::start();
    let hosting = &host.pair.host.core;
    let camera = host.open(config())?;
    let source = first_picture(&host, &camera);
    assert!(!camera.recording_allowed(), "recording starts off");
    assert!(
        matches!(camera.recording_video_tap(), Err(Error::Unauthorized)),
        "a camera the host does not allow recording is not recorded"
    );
    hosting.set_recording_sharing(true)?;
    wait_for("the host to allow recording", DEFAULT_WAIT, || {
        camera.recording_allowed().then_some(())
    });
    let _ = source.take_keyframe_request();
    let tap = camera.recording_video_tap()?;
    wait_for("the recording to ask for a keyframe", DEFAULT_WAIT, || {
        source.take_keyframe_request().then_some(())
    });
    source.publish(keyframe(2, 64)?)?;
    wait_for("the recording to begin on a keyframe", DEFAULT_WAIT, || {
        tap.try_recv()
            .is_some_and(|frame| frame.keyframe)
            .then_some(())
    });
    assert!(
        camera.recv_timeout(Duration::from_secs(5)).is_some(),
        "the viewer keeps its pictures"
    );
    hosting.set_recording_sharing(false)?;
    wait_for("the recording to end", DEFAULT_WAIT, || {
        tap.is_closed().then_some(())
    });
    assert!(
        !camera.recording_allowed(),
        "withdrawn consent disallows recording"
    );
    assert!(
        !camera.is_cancelled(),
        "withdrawing recording leaves the camera open"
    );
    expect![[r#"
        host:
          camera: camera opened fps=30 height=480 service=1 width=640
    "#]]
    .assert_eq(&trace.story(&["camera", "recording"]));
    Ok(())
}
