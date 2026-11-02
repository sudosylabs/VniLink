// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! What the camera suites share: a stand-in camera on the host that sends
//! the pictures a test gives it, a host offering a camera backend and a
//! camera opened from the controller as the application opens one, and a
//! backend whose startup holds until it is no longer wanted.

use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};

use vnilink::camera::{CameraBackend, CameraConfig, CameraDevice, CameraSource, HostCameraFactory};
use vnilink::{BackendStartup, Error, RemoteCameraService, ServiceKind, SessionInfo, SessionPhase};

use crate::desktop::keyframe;
use crate::terminal::Held;
use crate::{lock, wait_for, Engine, Pair, DEFAULT_WAIT};

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
            lock(&self.sources).last().cloned()
        })
    }

    /// How many cameras the host opened.
    pub fn opened(&self) -> usize {
        lock(&self.sources).len()
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
        lock(&self.sources).push(Arc::clone(&source));
        CameraBackend::new(requested, source)
    }
}

/// A host offering `factory` as its cameras, sharing its camera when
/// `shared`, and the controller's services connection to it.
pub fn connect(
    shared: bool,
    factory: Arc<dyn HostCameraFactory>,
) -> Result<(Pair, SessionInfo), Error> {
    let kinds: &[ServiceKind] = if shared { &[ServiceKind::Camera] } else { &[] };
    let pair = Pair::for_services(kinds);
    pair.host.core.set_camera_factory(Some(factory))?;
    let connection = pair.controller.connect_services(&pair.host)?;
    Ok((pair, connection))
}

/// Whether `controller`'s `connection` is still up.
pub fn connection_active(controller: &Engine, connection: &SessionInfo) -> bool {
    controller
        .core
        .controller_session(connection.id)
        .is_ok_and(|session| session.phase == SessionPhase::Active)
}

/// A host offering its stand-in camera, and a controller connected to it.
pub struct CameraHost {
    pub pair: Pair,
    pub cameras: Arc<StandInCameras>,
    pub connection: SessionInfo,
}

impl CameraHost {
    /// The host shares its camera.
    pub fn start() -> Result<Self, Error> {
        Self::offering(true)
    }

    /// The host's camera is installed but not shared.
    pub fn unshared() -> Result<Self, Error> {
        Self::offering(false)
    }

    fn offering(shared: bool) -> Result<Self, Error> {
        let cameras = Arc::new(StandInCameras::default());
        let (pair, connection) = connect(shared, Arc::clone(&cameras) as _)?;
        Ok(Self {
            pair,
            cameras,
            connection,
        })
    }

    /// Opens the camera with `config`.
    pub fn open(&self, config: CameraConfig) -> Result<RemoteCameraService, Error> {
        self.pair.controller.open_camera(&self.connection, config)
    }

    /// The latest camera's first picture, a keyframe, reaches `camera`; the
    /// source it came from.
    pub fn first_picture(&self, camera: &RemoteCameraService) -> Result<Arc<CameraSource>, Error> {
        let source = self.cameras.latest();
        source.publish(keyframe(1, vec![7; 64]))?;
        assert!(
            camera.recv_timeout(DEFAULT_WAIT).is_some(),
            "the camera's first picture arrives"
        );
        Ok(source)
    }

    /// Whether the controller's connection is still up.
    pub fn connection_active(&self) -> bool {
        connection_active(&self.pair.controller, &self.connection)
    }
}

/// Opening services: the Camera service, beside [`Engine::open_desktop`].
impl Engine {
    /// A Camera service attached on `connection` and opened with `config`,
    /// as the application opens one.
    pub fn open_camera(
        &self,
        connection: &SessionInfo,
        config: CameraConfig,
    ) -> Result<RemoteCameraService, Error> {
        let owner = self.core.attach_controller_service(
            connection.id,
            connection.generation,
            ServiceKind::Camera,
        )?;
        self.core
            .open_camera_service(owner.id, owner.generation, config)
    }
}

/// A camera whose startup waits, through [`BackendStartup::receive`], for
/// an acknowledgement that never comes, so only the camera no longer being
/// wanted, or the startup's own deadline, ends it; it then fails to open.
/// It says when it started, and how its wait ended.
pub struct HeldStartup {
    holding: (Mutex<Sender<()>>, Mutex<Receiver<()>>),
    finished: (Mutex<Sender<Held>>, Mutex<Receiver<Held>>),
    // Its sender is kept, so the channel never closes.
    acknowledgement: (Mutex<Sender<()>>, Mutex<Receiver<()>>),
}

impl HeldStartup {
    pub fn new() -> Arc<Self> {
        let (holding, held) = channel();
        let (finished, ended) = channel();
        let (acknowledge, acknowledgement) = channel();
        Arc::new(Self {
            holding: (Mutex::new(holding), Mutex::new(held)),
            finished: (Mutex::new(finished), Mutex::new(ended)),
            acknowledgement: (Mutex::new(acknowledge), Mutex::new(acknowledgement)),
        })
    }

    /// Waits until the startup holds.
    #[track_caller]
    pub fn wait_holding(&self) {
        wait(&lock(&self.holding.1), "the camera's startup to hold");
    }

    /// Waits until the startup stops holding, and says how.
    #[track_caller]
    pub fn wait_finished(&self) -> Held {
        wait(&lock(&self.finished.1), "the camera's startup to end")
    }
}

#[track_caller]
fn wait<T>(receiver: &Receiver<T>, what: &str) -> T {
    match receiver.recv_timeout(DEFAULT_WAIT) {
        Ok(value) => value,
        Err(_) => panic!("waited {DEFAULT_WAIT:?} for {what}, which did not happen"),
    }
}

impl HostCameraFactory for HeldStartup {
    fn devices(&self) -> Result<Vec<CameraDevice>, Error> {
        StandInCameras::default().devices()
    }

    fn open(&self, _: CameraConfig, startup: BackendStartup) -> Result<CameraBackend, Error> {
        // The receivers live as long as the backend.
        let _ = lock(&self.holding.0).send(());
        let acknowledged = startup.receive(&lock(&self.acknowledgement.1));
        let _ = lock(&self.finished.0).send(match &acknowledged {
            Ok(()) => Held::Released,
            Err(Error::NoSession) => Held::Cancelled,
            Err(error) => Held::Ended(error.to_string()),
        });
        Err(Error::NoSession)
    }
}
