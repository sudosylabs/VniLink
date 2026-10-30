// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! Native capture callbacks and independently owned encoded camera streams.
use crate::{error::map_error, FfiEncodedFrame, FfiError, FfiServiceInfo, VnilinkCore};
use std::sync::Arc;

/// The camera setting value that offers every camera.
pub const ALL_CAMERAS: &str = "vnilink:all-cameras";
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FfiCameraDevice {
    pub id: String,
    pub name: String,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FfiCameraConfig {
    pub device_id: String,
    pub width: u16,
    pub height: u16,
    pub fps: u16,
}
impl From<FfiCameraConfig> for vnilink::camera::CameraConfig {
    fn from(c: FfiCameraConfig) -> Self {
        Self {
            device_id: c.device_id,
            width: c.width,
            height: c.height,
            fps: c.fps,
        }
    }
}
impl From<vnilink::camera::CameraConfig> for FfiCameraConfig {
    fn from(c: vnilink::camera::CameraConfig) -> Self {
        Self {
            device_id: c.device_id,
            width: c.width,
            height: c.height,
            fps: c.fps,
        }
    }
}
/// Called on background workers. Capture/OS consent belongs to the native
/// provider; do not call Core inside these callbacks. Open returns a fresh source.

pub trait CameraProvider: Send + Sync {
    fn devices(&self) -> Result<Vec<FfiCameraDevice>, FfiError>;
    fn start_capture(
        &self,
        requested: FfiCameraConfig,
        startup: Arc<BackendStartup>,
    ) -> Result<Arc<CameraSource>, FfiError>;
}
/// Poll while acquiring the native camera. Cancellation/deadline applies only
/// to startup; use CameraSource.is_cancelled after Open succeeds.
pub struct BackendStartup {
    pub(crate) inner: vnilink::BackendStartup,
}

impl BackendStartup {
    pub fn is_cancelled(&self) -> bool {
        self.inner.is_cancelled()
    }
    pub fn check(&self) -> Result<(), FfiError> {
        self.inner.check().map_err(map_error)
    }
}
pub struct CameraSource {
    config: vnilink::camera::CameraConfig,
    inner: Arc<vnilink::camera::CameraSource>,
}

impl CameraSource {
    pub fn new(config: FfiCameraConfig) -> Result<Arc<Self>, FfiError> {
        let config: vnilink::camera::CameraConfig = config.into();
        config.validate().map_err(map_error)?;
        Ok(Arc::new(Self {
            config,
            inner: vnilink::camera::CameraSource::new(),
        }))
    }
    pub fn publish(&self, frame: FfiEncodedFrame) -> Result<(), FfiError> {
        let frame = vnilink::EncodedFrame::h264(frame.keyframe, frame.timestamp_us, frame.payload)
            .map_err(map_error)?;
        self.inner.publish(frame).map_err(map_error)
    }
    pub fn take_keyframe_request(&self) -> bool {
        self.inner.take_keyframe_request()
    }
    pub fn is_cancelled(&self) -> bool {
        self.inner.is_cancelled()
    }
    pub fn fail(&self) {
        self.inner.fail();
    }
    pub fn close(&self) {
        self.inner.close();
    }
}
struct NativeCamera(Box<dyn CameraProvider>);
impl vnilink::camera::HostCameraFactory for NativeCamera {
    fn devices(&self) -> Result<Vec<vnilink::camera::CameraDevice>, vnilink::Error> {
        self.0
            .devices()
            .map(|devices| {
                devices
                    .into_iter()
                    .map(|d| vnilink::camera::CameraDevice {
                        id: d.id,
                        name: d.name,
                    })
                    .collect()
            })
            .map_err(|_| vnilink::Error::Unsupported("native camera enumeration"))
    }
    fn open(
        &self,
        requested: vnilink::camera::CameraConfig,
        startup: vnilink::BackendStartup,
    ) -> Result<vnilink::camera::CameraBackend, vnilink::Error> {
        startup.check()?;
        let source = self
            .0
            .start_capture(
                requested.into(),
                Arc::new(BackendStartup {
                    inner: startup.clone(),
                }),
            )
            .map_err(|_| vnilink::Error::Unsupported("native camera capture"))?;
        if let Err(error) = startup.check() {
            source.inner.close();
            return Err(error);
        }
        if source.inner.is_cancelled() {
            return Err(vnilink::Error::NoSession);
        }
        vnilink::camera::CameraBackend::new(source.config.clone(), source.inner.clone())
    }
}
pub struct CameraService {
    inner: vnilink::RemoteCameraService,
}

impl CameraService {
    pub fn info(&self) -> FfiServiceInfo {
        self.inner.info().clone().into()
    }
    pub fn config(&self) -> FfiCameraConfig {
        self.inner.config().into()
    }
    pub fn is_cancelled(&self) -> bool {
        self.inner.is_cancelled()
    }
    pub fn poll(&self) -> Option<FfiEncodedFrame> {
        self.inner.try_recv().map(Into::into)
    }
    pub fn lagged(&self) -> u64 {
        self.inner.lagged()
    }
    pub(crate) fn remote(&self) -> &vnilink::RemoteCameraService {
        &self.inner
    }
    /// Why the stream ended, once, after the frames sent before the end.
    pub fn take_end(&self) -> Option<FfiCameraEnd> {
        self.inner.take_end().map(Into::into)
    }
    pub fn close(&self) {
        self.inner.close();
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FfiCameraEnd {
    Stopped,
    Withdrawn,
    Lost,
}
impl From<vnilink::camera::CameraEnd> for FfiCameraEnd {
    fn from(end: vnilink::camera::CameraEnd) -> Self {
        match end {
            vnilink::camera::CameraEnd::Stopped => Self::Stopped,
            vnilink::camera::CameraEnd::Withdrawn => Self::Withdrawn,
            vnilink::camera::CameraEnd::Lost => Self::Lost,
        }
    }
}

impl VnilinkCore {
    pub fn install_camera_provider(
        &self,
        provider: Box<dyn CameraProvider>,
    ) -> Result<(), FfiError> {
        self.inner
            .set_camera_factory(Some(Arc::new(NativeCamera(provider))))
            .map_err(map_error)
    }
    /// Offers no camera. Where this computer has a camera backend, paired
    /// computers read Camera as turned off rather than missing.
    pub fn disable_camera(&self) -> Result<(), FfiError> {
        #[cfg(any(target_os = "macos", target_os = "linux", windows))]
        let result = self.inner.turn_off_camera();
        #[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
        let result = self.inner.set_camera_factory(None);
        result.map_err(map_error)
    }
    pub fn remote_camera_devices(
        &self,
        connection_id: u64,
        generation: u64,
    ) -> Result<Vec<FfiCameraDevice>, FfiError> {
        self.inner
            .remote_camera_devices(connection_id, generation)
            .map(|ds| {
                ds.into_iter()
                    .map(|d| FfiCameraDevice {
                        id: d.id,
                        name: d.name,
                    })
                    .collect()
            })
            .map_err(map_error)
    }
    pub fn open_camera_service(
        &self,
        id: u64,
        generation: u64,
        config: FfiCameraConfig,
    ) -> Result<Arc<CameraService>, FfiError> {
        self.inner
            .open_camera_service(id, generation, config.into())
            .map(|inner| Arc::new(CameraService { inner }))
            .map_err(map_error)
    }
}

impl VnilinkCore {
    /// Local device metadata. Does not request consent or start capture.
    pub fn native_camera_devices(&self) -> Result<Vec<FfiCameraDevice>, FfiError> {
        #[cfg(target_os = "macos")]
        let result = vnilink_macos::camera_devices();
        #[cfg(target_os = "linux")]
        let result = vnilink_linux::camera_devices();
        #[cfg(windows)]
        let result = vnilink_windows::camera_devices();
        #[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
        let result: Result<Vec<vnilink::camera::CameraDevice>, vnilink::Error> =
            Err(vnilink::Error::Unsupported("native camera backend"));
        result
            .map(|devices| {
                devices
                    .into_iter()
                    .map(|d| FfiCameraDevice {
                        id: d.id,
                        name: d.name,
                    })
                    .collect()
            })
            .map_err(map_error)
    }
    /// Opt-in selection made locally, never camera selection by an unapproved
    /// remote ID. `vnilink:all-cameras` offers every camera, including one
    /// connected later. macOS OS consent must already have been granted.
    pub fn enable_native_camera(&self, approved_device_ids: Vec<String>) -> Result<(), FfiError> {
        let all = approved_device_ids.iter().any(|id| id == ALL_CAMERAS);
        if all && approved_device_ids.len() != 1 {
            return Err(map_error(vnilink::Error::InvalidSetting(
                "all cameras cannot be combined with explicit cameras",
            )));
        }
        let approved_device_ids = if all {
            vnilink::camera::CameraApproval::All
        } else {
            vnilink::camera::CameraApproval::Only(approved_device_ids)
        };
        #[cfg(target_os = "macos")]
        let factory =
            vnilink_macos::MacCameraFactory::new(approved_device_ids).map_err(map_error)?;
        #[cfg(target_os = "linux")]
        let factory =
            vnilink_linux::LinuxCameraFactory::new(approved_device_ids).map_err(map_error)?;
        #[cfg(windows)]
        let factory =
            vnilink_windows::WindowsCameraFactory::new(approved_device_ids).map_err(map_error)?;
        #[cfg(any(target_os = "macos", target_os = "linux", windows))]
        {
            self.inner
                .set_camera_factory(Some(Arc::new(factory)))
                .map_err(map_error)
        }
        #[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
        {
            let _ = approved_device_ids;
            Err(map_error(vnilink::Error::Unsupported(
                "native camera backend",
            )))
        }
    }
}
