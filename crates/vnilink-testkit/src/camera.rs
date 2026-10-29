//! What the camera suites share: a stand-in camera on the host that sends
//! the pictures a test gives it, and a camera opened from the controller
//! as the application opens one.

use std::sync::{Arc, Mutex};

use vnilink::camera::{CameraBackend, CameraConfig, CameraDevice, CameraSource, HostCameraFactory};
use vnilink::{BackendStartup, Error, RemoteCameraService, ServiceKind};

use crate::{wait_for, Pair, DEFAULT_WAIT};

pub const CAMERA: &str = "testkit-camera";

pub fn config() -> CameraConfig {
    CameraConfig {
        device_id: CAMERA.into(),
        width: 640,
        height: 480,
        fps: 30,
    }
}

/// The host's cameras: one, whose pictures are what a test publishes into
/// the latest [`CameraSource`] it opened.
#[derive(Default)]
pub struct StandInCameras {
    pub sources: Mutex<Vec<Arc<CameraSource>>>,
}

impl StandInCameras {
    /// The source the last camera opened sends from.
    pub fn latest(&self) -> Arc<CameraSource> {
        wait_for("the host to open its camera", DEFAULT_WAIT, || {
            self.sources
                .lock()
                .unwrap_or_else(|poison| poison.into_inner())
                .last()
                .cloned()
        })
    }
}

impl HostCameraFactory for StandInCameras {
    fn devices(&self) -> Result<Vec<CameraDevice>, Error> {
        Ok(vec![CameraDevice {
            id: CAMERA.into(),
            name: "Test kit camera".into(),
        }])
    }

    fn open(
        &self,
        requested: CameraConfig,
        startup: BackendStartup,
    ) -> Result<CameraBackend, Error> {
        startup.check()?;
        let source = CameraSource::new();
        self.sources
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .push(Arc::clone(&source));
        CameraBackend::new(requested, source)
    }
}

/// A host sharing its stand-in camera, and a controller connected to it.
pub struct CameraHost {
    pub pair: Pair,
    pub cameras: Arc<StandInCameras>,
    pub connection: vnilink::SessionInfo,
}

impl CameraHost {
    pub fn start() -> Self {
        let pair = Pair::for_services(&[ServiceKind::Camera]);
        let cameras = Arc::new(StandInCameras::default());
        pair.host
            .core
            .set_camera_factory(Some(Arc::clone(&cameras) as Arc<dyn HostCameraFactory>))
            .expect("a camera backend");
        let connection = pair.connect_services();
        Self {
            pair,
            cameras,
            connection,
        }
    }

    /// Opens the camera with `config`.
    pub fn open(&self, config: CameraConfig) -> Result<RemoteCameraService, Error> {
        let owner = self.pair.attach(&self.connection, ServiceKind::Camera);
        self.pair
            .controller
            .core
            .open_camera_service(owner.id, owner.generation, config)
    }
}
