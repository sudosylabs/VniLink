// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! Host-approved encoded camera sources. Platform capture remains outside Core.
use crate::{
    video::{EncodedFrame, VideoOutbox, VideoPush},
    Error,
};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct CameraDevice {
    pub id: String,
    pub name: String,
}
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct CameraConfig {
    pub device_id: String,
    pub width: u16,
    pub height: u16,
    pub fps: u16,
}
impl CameraConfig {
    pub fn validate(&self) -> Result<(), Error> {
        if self.device_id.is_empty()
            || self.device_id.len() > 256
            || self.width == 0
            || self.height == 0
            || self.width > 4096
            || self.height > 4096
            || !self.width.is_multiple_of(2)
            || !self.height.is_multiple_of(2)
            || self.fps == 0
            || self.fps > 60
        {
            return Err(Error::InvalidSetting(
                "camera configuration is out of range",
            ));
        }
        Ok(())
    }
}
/// Which cameras the host's own user offers to paired computers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CameraApproval {
    /// Every camera, including one connected later.
    All,
    /// Only these camera ids.
    Only(Vec<String>),
}
impl CameraApproval {
    pub fn validate(&self) -> Result<(), Error> {
        match self {
            Self::All => Ok(()),
            Self::Only(ids)
                if ids.len() <= 32 && ids.iter().all(|id| !id.is_empty() && id.len() <= 256) =>
            {
                Ok(())
            }
            Self::Only(_) => Err(Error::InvalidSetting("invalid approved camera IDs")),
        }
    }
    pub fn allows(&self, id: &str) -> bool {
        match self {
            Self::All => true,
            Self::Only(ids) => ids.iter().any(|approved| approved == id),
        }
    }
    /// Whether `id` was chosen by name, as opposed to allowed by All.
    pub fn names(&self, id: &str) -> bool {
        matches!(self, Self::Only(ids) if ids.iter().any(|approved| approved == id))
    }
}
/// Why a camera stream ended, as its viewer learns it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CameraEnd {
    /// The host's camera stopped delivering pictures.
    Stopped,
    /// The host stopped sharing the camera: its access or its camera setting changed.
    Withdrawn,
    /// The connection to the host ended.
    Lost,
}
/// Host opt-in is separate from each peer's Camera grant. Return only devices
/// approved locally. Open must honour OS consent and return the actual format.
/// Encode Annex-B H.264 Constrained Baseline, with SPS/PPS on every keyframe,
/// without B-frames. The producer must serialize publication and stop promptly
/// when its source is cancelled, including when the device stops delivering.
/// During Open, check BackendStartup between resource acquisitions and while
/// waiting for the first encoded frame; cancellation/deadline must close partial
/// capture. After Open succeeds, use CameraSource cancellation instead.
pub trait HostCameraFactory: Send + Sync {
    fn devices(&self) -> Result<Vec<CameraDevice>, Error>;
    fn open(
        &self,
        requested: CameraConfig,
        startup: crate::BackendStartup,
    ) -> Result<CameraBackend, Error>;
}
/// Native producer polls cancellation and keyframe requests. One source per
/// capture; do not reuse a source across Opens or publish Desktop frames here.
pub struct CameraSource {
    claimed: AtomicBool,
    pub(crate) frames: VideoOutbox,
    stopped: AtomicBool,
    failed: AtomicBool,
    keyframe: AtomicBool,
    needs_keyframe: AtomicBool,
}
impl CameraSource {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            claimed: AtomicBool::new(false),
            frames: VideoOutbox::new(),
            stopped: AtomicBool::new(false),
            failed: AtomicBool::new(false),
            keyframe: AtomicBool::new(true),
            needs_keyframe: AtomicBool::new(true),
        })
    }
    pub fn publish(&self, frame: EncodedFrame) -> Result<(), Error> {
        if self.is_cancelled() {
            return Err(Error::NoSession);
        }
        frame.check()?;
        if frame.role == crate::FrameRole::Still {
            return Err(Error::Unsupported("still pictures from a camera"));
        }
        if !frame.keyframe && self.needs_keyframe.load(Ordering::Acquire) {
            return Ok(());
        }
        if frame.keyframe {
            self.needs_keyframe.store(false, Ordering::Release);
        }
        if matches!(self.frames.push(frame), VideoPush::KeyframeNeeded) {
            self.request_keyframe();
        }
        Ok(())
    }
    pub fn take_keyframe_request(&self) -> bool {
        self.keyframe.swap(false, Ordering::AcqRel)
    }
    pub(crate) fn request_keyframe(&self) {
        self.needs_keyframe.store(true, Ordering::Release);
        self.keyframe.store(true, Ordering::Release);
    }
    pub fn is_cancelled(&self) -> bool {
        self.stopped.load(Ordering::Acquire)
    }
    pub fn fail(&self) {
        self.failed.store(true, Ordering::Release);
        self.close();
    }
    pub(crate) fn has_failed(&self) -> bool {
        self.failed.load(Ordering::Acquire)
    }
    pub fn close(&self) {
        self.stopped.store(true, Ordering::Release);
    }
}
pub struct CameraBackend {
    pub config: CameraConfig,
    pub source: Arc<CameraSource>,
    _claim: (),
}
impl CameraBackend {
    pub fn new(config: CameraConfig, source: Arc<CameraSource>) -> Result<Self, Error> {
        config.validate()?;
        if source.is_cancelled() {
            return Err(Error::NoSession);
        }
        if source
            .claimed
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Err(Error::Busy);
        }
        Ok(Self {
            config,
            source,
            _claim: (),
        })
    }
}
impl Drop for CameraBackend {
    fn drop(&mut self) {
        self.source.close();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn camera_approval_names_or_allows_every_camera() {
        let only = CameraApproval::Only(vec!["a".into()]);
        assert!(only.allows("a") && only.names("a"));
        assert!(!only.allows("b") && !only.names("b"));
        assert!(CameraApproval::All.allows("later") && !CameraApproval::All.names("later"));
        assert!(CameraApproval::All.validate().is_ok());
        assert!(CameraApproval::Only(vec![String::new()])
            .validate()
            .is_err());
        assert!(CameraApproval::Only(vec!["x".into(); 33])
            .validate()
            .is_err());
    }
}
