use super::support::{pair_for_control, start_direct};
use crate::{camera::*, video::EncodedFrame, Core, Error, ServiceKind, SessionPhase};
use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
fn wait(mut condition: impl FnMut() -> bool) {
    let until = Instant::now() + Duration::from_secs(8);
    while !condition() {
        assert!(Instant::now() < until);
        std::thread::sleep(Duration::from_millis(5));
    }
}
fn config() -> CameraConfig {
    CameraConfig {
        device_id: "approved-camera".into(),
        width: 640,
        height: 480,
        fps: 30,
    }
}
#[derive(Default)]
struct Factory {
    sources: Mutex<Vec<Arc<CameraSource>>>,
}
impl HostCameraFactory for Factory {
    fn devices(&self) -> Result<Vec<CameraDevice>, Error> {
        Ok(vec![CameraDevice {
            id: config().device_id,
            name: "Approved camera".into(),
        }])
    }
    fn open(
        &self,
        requested: CameraConfig,
        startup: crate::BackendStartup,
    ) -> Result<CameraBackend, Error> {
        startup.check()?;
        let source = CameraSource::new();
        self.sources.lock().unwrap().push(source.clone());
        CameraBackend::new(requested, source)
    }
}
fn connect(controller: &Core, host: &Core) -> crate::SessionInfo {
    let c = controller
        .begin_connect_services(&host.endpoint_id())
        .unwrap();
    wait(|| controller.controller_session(c.id).unwrap().phase == SessionPhase::Active);
    controller.controller_session(c.id).unwrap()
}
#[test]
fn camera_grant_large_frame_and_independent_close() {
    let controller = start_direct();
    let host = start_direct();
    pair_for_control(&host, &controller);
    let factory = Arc::new(Factory::default());
    host.set_camera_factory(Some(factory.clone())).unwrap();
    let connection = connect(&controller, &host);
    assert!(
        controller
            .remote_service_support(connection.id, connection.generation)
            .unwrap()
            .camera
    );
    assert!(matches!(
        controller.remote_camera_devices(connection.id, connection.generation),
        Err(Error::Unauthorized)
    ));
    host.authorize_service(&controller.endpoint_id(), ServiceKind::Camera, true)
        .unwrap();
    assert_eq!(
        controller
            .remote_camera_devices(connection.id, connection.generation)
            .unwrap(),
        factory.devices().unwrap()
    );
    let files = controller
        .attach_controller_service(connection.id, connection.generation, ServiceKind::Files)
        .unwrap();
    let files_lease = controller
        .service_lease(files.id, files.generation)
        .unwrap();
    let owner = controller
        .attach_controller_service(connection.id, connection.generation, ServiceKind::Camera)
        .unwrap();
    let camera = controller
        .open_camera_service(owner.id, owner.generation, config())
        .unwrap();
    assert!(matches!(
        controller.open_camera_service(owner.id, owner.generation, config()),
        Err(Error::Busy)
    ));
    assert_eq!(camera.config(), config());
    let source = factory.sources.lock().unwrap()[0].clone();
    assert!(source.take_keyframe_request());
    source
        .publish(EncodedFrame::h264(false, 1, vec![1; 5]).unwrap())
        .unwrap();
    assert!(camera.recv_timeout(Duration::from_millis(50)).is_none());
    let expected = EncodedFrame::h264(true, 2, vec![7; 512 * 1024]).unwrap();
    source.publish(expected.clone()).unwrap();
    assert_eq!(camera.recv_timeout(Duration::from_secs(5)), Some(expected));
    camera.close();
    wait(|| source.is_cancelled());
    assert!(!files_lease.is_cancelled());
    assert_eq!(
        controller.controller_session(connection.id).unwrap().phase,
        SessionPhase::Active
    );
    assert!(matches!(
        source.publish(EncodedFrame::h264(true, 3, vec![1]).unwrap()),
        Err(Error::NoSession)
    ));
}
#[test]
fn camera_revocation_and_backend_disable_stop_capture() {
    let controller = start_direct();
    let host = start_direct();
    pair_for_control(&host, &controller);
    host.authorize_service(&controller.endpoint_id(), ServiceKind::Camera, true)
        .unwrap();
    let factory = Arc::new(Factory::default());
    host.set_camera_factory(Some(factory.clone())).unwrap();
    let connection = connect(&controller, &host);
    for revoke in [true, false] {
        host.authorize_service(&controller.endpoint_id(), ServiceKind::Camera, true)
            .unwrap();
        let owner = controller
            .attach_controller_service(connection.id, connection.generation, ServiceKind::Camera)
            .unwrap();
        let camera = controller
            .open_camera_service(owner.id, owner.generation, config())
            .unwrap();
        if revoke {
            host.authorize_service(&controller.endpoint_id(), ServiceKind::Camera, false)
                .unwrap();
        } else {
            host.set_camera_factory(None).unwrap();
        }
        wait(|| camera.is_cancelled());
        wait(|| {
            factory
                .sources
                .lock()
                .unwrap()
                .last()
                .unwrap()
                .is_cancelled()
        });
        assert_eq!(
            controller.controller_session(connection.id).unwrap().phase,
            SessionPhase::Active
        );
    }
    assert!(
        !controller
            .remote_service_support(connection.id, connection.generation)
            .unwrap()
            .camera
    );
}
#[test]
fn camera_unknown_device_and_invalid_config_do_not_start_capture() {
    let controller = start_direct();
    let host = start_direct();
    pair_for_control(&host, &controller);
    host.authorize_service(&controller.endpoint_id(), ServiceKind::Camera, true)
        .unwrap();
    let factory = Arc::new(Factory::default());
    host.set_camera_factory(Some(factory.clone())).unwrap();
    let connection = connect(&controller, &host);
    let owner = controller
        .attach_controller_service(connection.id, connection.generation, ServiceKind::Camera)
        .unwrap();
    let mut invalid = config();
    invalid.fps = 0;
    assert!(matches!(
        controller.open_camera_service(owner.id, owner.generation, invalid),
        Err(Error::InvalidSetting(_))
    ));
    let mut unknown = config();
    unknown.device_id = "not-approved".into();
    assert!(matches!(
        controller.open_camera_service(owner.id, owner.generation, unknown),
        Err(Error::Unsupported(_))
    ));
    assert!(factory.sources.lock().unwrap().is_empty());
    assert_eq!(
        controller.controller_session(connection.id).unwrap().phase,
        SessionPhase::Active
    );
}

#[test]
fn camera_backend_failure_is_reported_without_closing_parent() {
    let controller = start_direct();
    let host = start_direct();
    pair_for_control(&host, &controller);
    host.authorize_service(&controller.endpoint_id(), ServiceKind::Camera, true)
        .unwrap();
    let factory = Arc::new(Factory::default());
    host.set_camera_factory(Some(factory.clone())).unwrap();
    let connection = connect(&controller, &host);
    let owner = controller
        .attach_controller_service(connection.id, connection.generation, ServiceKind::Camera)
        .unwrap();
    let camera = controller
        .open_camera_service(owner.id, owner.generation, config())
        .unwrap();
    factory.sources.lock().unwrap()[0].fail();
    wait(|| {
        matches!(
            controller.controller_service(owner.id).unwrap().phase,
            crate::ServicePhase::Failed(_)
        )
    });
    assert!(camera.is_cancelled());
    assert_eq!(
        controller.controller_session(connection.id).unwrap().phase,
        SessionPhase::Active
    );
}

#[test]
fn camera_startup_observes_owner_revoke_parent_loss_and_shutdown_without_external_release() {
    for cause in 0..4 {
        struct BlockingFactory {
            entered: std::sync::mpsc::SyncSender<()>,
            completed: std::sync::mpsc::SyncSender<()>,
            blocked: Mutex<std::sync::mpsc::Receiver<()>>,
        }
        impl HostCameraFactory for BlockingFactory {
            fn devices(&self) -> Result<Vec<CameraDevice>, Error> {
                Ok(vec![CameraDevice {
                    id: config().device_id,
                    name: "Test camera".into(),
                }])
            }
            fn open(
                &self,
                _: CameraConfig,
                startup: crate::BackendStartup,
            ) -> Result<CameraBackend, Error> {
                self.entered.send(()).unwrap();
                let result = startup.receive(&self.blocked.lock().unwrap());
                assert!(matches!(result, Err(Error::NoSession)));
                self.completed.send(()).unwrap();
                Err(Error::NoSession)
            }
        }
        let host = start_direct();
        let controller = start_direct();
        pair_for_control(&host, &controller);
        host.authorize_service(&controller.endpoint_id(), ServiceKind::Camera, true)
            .unwrap();
        let (entered, ready) = std::sync::mpsc::sync_channel(1);
        let (completed, done) = std::sync::mpsc::sync_channel(1);
        let (_blocked, waiting) = std::sync::mpsc::channel();
        host.set_camera_factory(Some(Arc::new(BlockingFactory {
            entered,
            completed,
            blocked: Mutex::new(waiting),
        })))
        .unwrap();
        let parent = connect(&controller, &host);
        let owner = controller
            .attach_controller_service(parent.id, parent.generation, ServiceKind::Camera)
            .unwrap();
        std::thread::scope(|scope| {
            let open = scope
                .spawn(|| controller.open_camera_service(owner.id, owner.generation, config()));
            ready.recv_timeout(Duration::from_secs(5)).unwrap();
            match cause {
                0 => host
                    .authorize_service(&controller.endpoint_id(), ServiceKind::Camera, false)
                    .unwrap(),
                1 => host
                    .stop_hosted_peer(host.hosted_sessions().unwrap()[0].id)
                    .unwrap(),
                2 => host.shutdown(),
                _ => controller
                    .close_controller_service(owner.id, owner.generation)
                    .unwrap(),
            }
            done.recv_timeout(Duration::from_secs(1))
                .expect("factory must observe cancellation promptly");
            assert!(open.join().unwrap().is_err());
        });
        if cause == 0 || cause == 3 {
            wait(|| host.hosted_services().unwrap().is_empty());
            assert_eq!(
                controller.controller_session(parent.id).unwrap().phase,
                SessionPhase::Active
            );
        }
    }
}

#[test]
fn late_camera_backend_is_closed_without_admission_after_revoke() {
    struct LateFactory {
        entered: std::sync::mpsc::SyncSender<()>,
        release: Mutex<std::sync::mpsc::Receiver<()>>,
        source: Arc<CameraSource>,
    }
    impl HostCameraFactory for LateFactory {
        fn devices(&self) -> Result<Vec<CameraDevice>, Error> {
            Ok(vec![CameraDevice {
                id: config().device_id,
                name: "Test".into(),
            }])
        }
        fn open(
            &self,
            config: CameraConfig,
            _: crate::BackendStartup,
        ) -> Result<CameraBackend, Error> {
            self.entered.send(()).unwrap();
            self.release
                .lock()
                .unwrap()
                .recv_timeout(Duration::from_secs(5))
                .unwrap();
            CameraBackend::new(config, self.source.clone())
        }
    }
    let host = start_direct();
    let controller = start_direct();
    pair_for_control(&host, &controller);
    host.authorize_service(&controller.endpoint_id(), ServiceKind::Camera, true)
        .unwrap();
    let (entered, ready) = std::sync::mpsc::sync_channel(1);
    let (release, blocked) = std::sync::mpsc::channel();
    let source = CameraSource::new();
    host.set_camera_factory(Some(Arc::new(LateFactory {
        entered,
        release: Mutex::new(blocked),
        source: source.clone(),
    })))
    .unwrap();
    let parent = connect(&controller, &host);
    let owner = controller
        .attach_controller_service(parent.id, parent.generation, ServiceKind::Camera)
        .unwrap();
    std::thread::scope(|scope| {
        let open =
            scope.spawn(|| controller.open_camera_service(owner.id, owner.generation, config()));
        ready.recv_timeout(Duration::from_secs(5)).unwrap();
        host.authorize_service(&controller.endpoint_id(), ServiceKind::Camera, false)
            .unwrap();
        wait(|| host.hosted_services().unwrap().is_empty());
        release.send(()).unwrap();
        assert!(open.join().unwrap().is_err());
    });
    wait(|| source.is_cancelled());
    assert!(source
        .publish(EncodedFrame::h264(true, 1, vec![1]).unwrap())
        .is_err());
    assert_eq!(
        controller.controller_session(parent.id).unwrap().phase,
        SessionPhase::Active
    );
}

/// A factory open failure surfaces in the availability query as a backend
/// failure, and a later successful open clears it. Cancellation and
/// protocol errors never publish.
#[test]
fn camera_open_failure_publishes_and_success_clears() {
    use crate::availability::{AvailabilityReason, Capability};
    struct FailingFactory;
    impl HostCameraFactory for FailingFactory {
        fn devices(&self) -> Result<Vec<CameraDevice>, Error> {
            Ok(vec![CameraDevice {
                id: config().device_id,
                name: "Failing camera".into(),
            }])
        }
        fn open(&self, _: CameraConfig, _: crate::BackendStartup) -> Result<CameraBackend, Error> {
            Err(Error::Unsupported("camera sensor"))
        }
    }
    let controller = start_direct();
    let host = start_direct();
    pair_for_control(&host, &controller);
    host.authorize_service(&controller.endpoint_id(), ServiceKind::Camera, true)
        .unwrap();
    host.set_camera_factory(Some(Arc::new(FailingFactory)))
        .unwrap();
    let connection = connect(&controller, &host);
    let owner = controller
        .attach_controller_service(connection.id, connection.generation, ServiceKind::Camera)
        .unwrap();
    assert!(controller
        .open_camera_service(owner.id, owner.generation, config())
        .is_err());
    let failed = host
        .capability_availability(None, Capability::Camera)
        .unwrap();
    assert!(!failed.available);
    assert_eq!(
        failed.reasons,
        [AvailabilityReason::BackendFailed {
            detail: "camera sensor is not available yet".into(),
        }]
    );
    host.set_camera_factory(Some(Arc::new(Factory::default())))
        .unwrap();
    // The failed open leaves its owner record behind; release it
    // synchronously so the retry attach never races cleanup.
    controller
        .close_controller_service(owner.id, owner.generation)
        .unwrap();
    controller.forget_controller_service(owner.id).unwrap();
    let retry = controller
        .attach_controller_service(connection.id, connection.generation, ServiceKind::Camera)
        .unwrap();
    let camera = controller
        .open_camera_service(retry.id, retry.generation, config())
        .unwrap();
    camera.close();
    let recovered = host
        .capability_availability(None, Capability::Camera)
        .unwrap();
    assert!(recovered.available, "{recovered:?}");
}

/// One opener per camera device: a second concurrent open of the same
/// device is structured Busy, a different device opens fine, and closing
/// releases the device for the other peer.
#[test]
fn camera_device_is_exclusive_across_peers_and_releases_on_close() {
    struct TwoCameras;
    impl HostCameraFactory for TwoCameras {
        fn devices(&self) -> Result<Vec<CameraDevice>, Error> {
            Ok(vec![
                CameraDevice {
                    id: "cam-one".into(),
                    name: "One".into(),
                },
                CameraDevice {
                    id: "cam-two".into(),
                    name: "Two".into(),
                },
            ])
        }
        fn open(
            &self,
            requested: CameraConfig,
            startup: crate::BackendStartup,
        ) -> Result<CameraBackend, Error> {
            startup.check()?;
            CameraBackend::new(requested, CameraSource::new())
        }
    }
    fn cfg(id: &str) -> CameraConfig {
        CameraConfig {
            device_id: id.into(),
            width: 640,
            height: 480,
            fps: 30,
        }
    }
    let host = start_direct();
    host.set_camera_factory(Some(Arc::new(TwoCameras))).unwrap();
    let a = start_direct();
    let b = start_direct();
    pair_for_control(&host, &a);
    pair_for_control(&host, &b);
    host.authorize_service(&a.endpoint_id(), ServiceKind::Camera, true)
        .unwrap();
    host.authorize_service(&b.endpoint_id(), ServiceKind::Camera, true)
        .unwrap();
    let ca = connect(&a, &host);
    let cb = connect(&b, &host);
    let oa = a
        .attach_controller_service(ca.id, ca.generation, ServiceKind::Camera)
        .unwrap();
    let first = a
        .open_camera_service(oa.id, oa.generation, cfg("cam-one"))
        .unwrap();
    let ob = b
        .attach_controller_service(cb.id, cb.generation, ServiceKind::Camera)
        .unwrap();
    assert!(matches!(
        b.open_camera_service(ob.id, ob.generation, cfg("cam-one")),
        Err(Error::Busy)
    ));
    // A failed open leaves its owner record behind; release it before the
    // next attach so the retry never races cleanup.
    b.close_controller_service(ob.id, ob.generation).unwrap();
    b.forget_controller_service(ob.id).unwrap();
    let ob2 = b
        .attach_controller_service(cb.id, cb.generation, ServiceKind::Camera)
        .unwrap();
    let second = b
        .open_camera_service(ob2.id, ob2.generation, cfg("cam-two"))
        .unwrap();
    // Closing releases the device; the other peer takes it over. Each
    // attempt uses a fresh owner because a refused open consumes its own.
    first.close();
    second.close();
    b.close_controller_service(ob2.id, ob2.generation).unwrap();
    b.forget_controller_service(ob2.id).unwrap();
    let until = Instant::now() + Duration::from_secs(8);
    let taken = loop {
        let attempt = b
            .attach_controller_service(cb.id, cb.generation, ServiceKind::Camera)
            .unwrap();
        match b.open_camera_service(attempt.id, attempt.generation, cfg("cam-one")) {
            Ok(camera) => break camera,
            Err(Error::Busy) => {
                b.close_controller_service(attempt.id, attempt.generation)
                    .unwrap();
                b.forget_controller_service(attempt.id).unwrap();
                assert!(Instant::now() < until, "released device stayed busy");
                std::thread::sleep(Duration::from_millis(5));
            }
            Err(other) => panic!("unexpected open error: {other:?}"),
        }
    };
    taken.close();
}
