//! A camera whose backend is still starting when its grant, its owner, its
//! connection or its host goes away: the backend learns at once without
//! being released from outside, and a backend that opens anyway is closed
//! unused.

use expect_test::expect;
use std::sync::mpsc::{channel, sync_channel, Receiver, Sender, SyncSender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use vnilink::camera::{CameraBackend, CameraConfig, CameraDevice, CameraSource, HostCameraFactory};
use vnilink::{BackendStartup, Error, ServiceInfo, ServiceKind};
use vnilink_testkit::camera::{config, StandInCameras};
use vnilink_testkit::desktop::keyframe;
use vnilink_testkit::{traced, wait_for, Pair, TestResult, DEFAULT_WAIT};

/// How soon a starting backend learns its camera is no longer wanted.
const NOTICED_WITHIN: Duration = Duration::from_secs(1);

/// A camera whose startup waits for an acknowledgement that never comes,
/// through [`BackendStartup::receive`], and reports how it ended.
struct NeverAcknowledged {
    entered: SyncSender<()>,
    completed: SyncSender<Result<(), Error>>,
    acknowledgement: Mutex<Receiver<()>>,
}

impl HostCameraFactory for NeverAcknowledged {
    fn devices(&self) -> Result<Vec<CameraDevice>, Error> {
        StandInCameras::default().devices()
    }

    fn open(&self, _: CameraConfig, startup: BackendStartup) -> Result<CameraBackend, Error> {
        self.entered.send(()).expect("the test waits for the open");
        let acknowledged = startup.receive(
            &self
                .acknowledgement
                .lock()
                .unwrap_or_else(|poison| poison.into_inner()),
        );
        self.completed
            .send(acknowledged)
            .expect("the test waits for the startup");
        Err(Error::NoSession)
    }
}

/// Opens the camera on a host whose backend never finishes starting, and
/// once it is starting, does `cause`. The backend learns within
/// [`NOTICED_WITHIN`] of `cause` that its camera is no longer wanted, and
/// the open fails. Gives the pair and the connection, for what `cause`
/// leaves.
fn startup_stopped_by(
    cause: impl FnOnce(&Pair, &ServiceInfo) -> Result<(), Error>,
) -> Result<(Pair, vnilink::SessionInfo), Error> {
    let pair = Pair::for_services(&[ServiceKind::Camera]);
    let (entered, entering) = sync_channel(1);
    let (completed, completing) = sync_channel(1);
    // Held, so the acknowledgement the backend waits for never comes and
    // the channel never closes.
    let (_never, acknowledgement): (Sender<()>, _) = channel();
    pair.host
        .core
        .set_camera_factory(Some(Arc::new(NeverAcknowledged {
            entered,
            completed,
            acknowledgement: Mutex::new(acknowledgement),
        })))?;
    let connection = pair.connect_services();
    let owner = pair.attach(&connection, ServiceKind::Camera);
    std::thread::scope(|scope| -> Result<(), Error> {
        let opening = scope.spawn(|| {
            pair.controller
                .core
                .open_camera_service(owner.id, owner.generation, config())
        });
        entering
            .recv_timeout(Duration::from_secs(5))
            .expect("the host's backend starts");
        let caused = Instant::now();
        cause(&pair, &owner)?;
        let acknowledged = completing
            .recv_timeout(NOTICED_WITHIN.saturating_sub(caused.elapsed()))
            .expect("the backend learns promptly that its camera is no longer wanted");
        assert!(
            matches!(acknowledged, Err(Error::NoSession)),
            "a starting backend whose camera is no longer wanted stops: {acknowledged:?}"
        );
        assert!(
            opening.join().expect("the open returns").is_err(),
            "a camera whose startup stopped does not open"
        );
        Ok(())
    })?;
    Ok((pair, connection))
}

/// The host stops serving the camera, and the connection stays.
fn host_released_the_camera(pair: &Pair, connection: &vnilink::SessionInfo) {
    pair.host.wait_released(None);
    assert!(
        pair.controller
            .core
            .controller_session(connection.id)
            .is_ok_and(|session| session.phase == vnilink::SessionPhase::Active),
        "a camera that did not start keeps the connection"
    );
}

#[test]
fn a_starting_camera_stops_when_the_host_stops_sharing_it() -> TestResult {
    let _trace = traced();
    let (pair, connection) = startup_stopped_by(|pair, _| {
        pair.host
            .core
            .set_service_sharing(ServiceKind::Camera, false)
    })?;
    host_released_the_camera(&pair, &connection);
    Ok(())
}

#[test]
fn a_starting_camera_stops_when_the_host_stops_serving_its_peer() -> TestResult {
    let _trace = traced();
    startup_stopped_by(|pair, _| {
        let peer = pair.host.core.hosted_sessions()?[0].id;
        pair.host.core.stop_hosted_peer(peer)
    })?;
    Ok(())
}

#[test]
fn a_starting_camera_stops_when_the_host_shuts_down() -> TestResult {
    let _trace = traced();
    startup_stopped_by(|pair, _| {
        pair.host.core.shutdown();
        Ok(())
    })?;
    Ok(())
}

#[test]
fn a_starting_camera_stops_when_its_viewer_closes_it() -> TestResult {
    let _trace = traced();
    let (pair, connection) = startup_stopped_by(|pair, owner| {
        pair.controller
            .core
            .close_controller_service(owner.id, owner.generation)
    })?;
    host_released_the_camera(&pair, &connection);
    Ok(())
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
        self.release
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .recv_timeout(Duration::from_secs(5))
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
    let pair = Pair::for_services(&[ServiceKind::Camera]);
    let (entered, entering) = sync_channel(1);
    let (release, released) = channel();
    let source = CameraSource::new();
    pair.host.core.set_camera_factory(Some(Arc::new(OpensLate {
        entered,
        release: Mutex::new(released),
        source: Arc::clone(&source),
    })))?;
    let connection = pair.connect_services();
    let owner = pair.attach(&connection, ServiceKind::Camera);
    std::thread::scope(|scope| -> TestResult {
        let opening = scope.spawn(|| {
            pair.controller
                .core
                .open_camera_service(owner.id, owner.generation, config())
        });
        entering.recv_timeout(Duration::from_secs(5))?;
        pair.host
            .core
            .set_service_sharing(ServiceKind::Camera, false)?;
        pair.host.wait_released(None);
        release.send(())?;
        assert!(
            opening.join().expect("the open returns").is_err(),
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
        pair.controller
            .core
            .controller_session(connection.id)
            .is_ok_and(|session| session.phase == vnilink::SessionPhase::Active),
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
