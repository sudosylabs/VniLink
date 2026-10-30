// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! The host's capture consumers and the codec and quality settings Desktop
//! owners start from.

use super::*;

use std::sync::Arc;

use crate::error::Error;
use crate::protocol::Quality;
use crate::video::HostCapture;

/// One consumer of the shared physical capture: an active hosted Desktop
/// service. Each target owns its encoder, quality and queue.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VideoTarget {
    /// The hosted Desktop service.
    pub id: u64,
    pub generation: u64,
    pub quality: Quality,
    pub separate_cursor: bool,
    pub codec: u8,
    /// The matrix its pictures are converted and declared in.
    pub matrix: crate::YuvMatrix,
    /// Display this consumer selected. Zero means it has not chosen one and
    /// still follows whichever capture the supervisor opened.
    pub display_id: u64,
    /// Topology revision the selection was accepted under. Zero when the
    /// consumer selected nothing or the display is no longer published.
    pub revision: u64,
}

/// Consumers and the resolved source for consumers without a selection.
#[derive(Debug, Clone)]
pub struct VideoRouting {
    pub targets: Vec<VideoTarget>,
    pub default_source: Option<u64>,
}
impl VideoRouting {
    pub fn serves_selection(&self, selected: &[u64], source: u64) -> bool {
        serves_source(
            selected.first().copied().unwrap_or(0),
            source,
            self.default_source,
        )
    }
}
impl VideoTarget {
    pub fn serves_source(&self, source: u64, default: Option<u64>) -> bool {
        serves_source(self.display_id, source, default)
    }
}
fn serves_source(selected: u64, source: u64, default: Option<u64>) -> bool {
    source == 0
        || if selected == 0 {
            default.is_none_or(|id| id == source)
        } else {
            selected == source
        }
}

impl Core {
    /// Fence physical capture startup to the consumers present when it began.
    /// One departing viewer does not cancel its siblings. A newly attached
    /// viewer cannot adopt a cancelled startup; the supervisor opens afresh.
    pub fn desktop_capture_startup(
        &self,
        display_id: u64,
        stop: Arc<std::sync::atomic::AtomicBool>,
        timeout: std::time::Duration,
    ) -> Result<crate::BackendStartup, Error> {
        let _engine = self.engine();
        let state = self.alive_state()?;
        let routing = video_routing_locked(&self.shared, &state);
        let mut owners = Vec::new();
        for target in routing
            .targets
            .into_iter()
            .filter(|target| target.serves_source(display_id, routing.default_source))
        {
            if let Some(owner) = state
                .hosted_services
                .get(&target.id)
                .filter(|owner| owner.info.generation == target.generation)
            {
                owners.push(owner.stop.subscribe());
            }
        }
        let startup = crate::BackendStartup::for_consumers(stop, owners, timeout);
        startup.check()?;
        Ok(startup)
    }
    /// Reports the monitor this host captures.
    ///
    /// The platform captures and encodes, then passes bytes to
    /// [`Core::publish_desktop_video`]. A connection may configure capture
    /// before Desktop Open. Refused while Desktop is active or completing
    /// teardown.
    pub fn set_capture(&self, capture: Box<dyn HostCapture>) -> Result<(), Error> {
        let _engine = self.engine();
        let _state = self.alive_state()?;
        if self
            .shared
            .desktop_streams
            .load(std::sync::atomic::Ordering::Acquire)
            != 0
        {
            return Err(Error::Busy);
        }
        let _gate = self
            .shared
            .desktop_gate
            .try_lock()
            .map_err(|_| Error::Busy)?;
        *lock(&self.shared.capture) = Some(capture);
        Ok(())
    }

    /// Registers the platform's probe-validated video codec abilities.
    ///
    /// The library never probes hardware itself. The native layer calls this
    /// after initializing its encoder and decoder so `capabilities()`,
    /// admission and negotiation report an explicit unsupported state
    /// instead of admitting a session that can never carry video. Every set
    /// bit must mean a backend processed a sample; a failed probe registers
    /// cleared bits, never assumed ones.
    pub fn set_video_codecs(&self, caps: crate::video_caps::VideoCodecCaps) -> Result<(), Error> {
        let _engine = self.engine();
        self.ensure_alive()?;
        caps.validate()?;
        *lock(&self.shared.video_caps) = caps;
        Ok(())
    }

    /// Currently registered video codec abilities.
    pub fn video_codecs(&self) -> Result<crate::video_caps::VideoCodecCaps, Error> {
        let _engine = self.engine();
        self.ensure_alive()?;
        Ok(lock(&self.shared.video_caps).clone())
    }

    /// Live capture consumers. The list is a snapshot: a later open, close or
    /// revoke changes the next call. Ordered by service id.
    pub fn video_targets(&self) -> Result<Vec<VideoTarget>, Error> {
        let _engine = self.engine();
        let state = self.alive_state()?;
        Ok(video_targets_locked(&self.shared, &state))
    }

    /// Frame, cursor and failure attribution share this atomic routing snapshot.
    pub fn video_routing(&self) -> Result<VideoRouting, Error> {
        let _engine = self.engine();
        let state = self.alive_state()?;
        Ok(video_routing_locked(&self.shared, &state))
    }

    /// Starting limits for newly opened Desktop owners.
    /// The application persists this preference and restores it before dialing.
    pub fn desktop_quality_defaults(&self) -> Result<Quality, Error> {
        let _engine = self.engine();
        Ok(self.alive_state()?.desktop_quality_default)
    }

    /// Codec for new outgoing Desktop owners; None negotiates automatically.
    pub fn desktop_codec_default(&self) -> Result<Option<crate::VideoCodec>, Error> {
        let _engine = self.engine();
        Ok(self.alive_state()?.desktop_codec_default)
    }

    /// The codecs the app's window decodes, hardware ones marked. Future
    /// Desktop owners offer only these, and only the hardware ones when the
    /// window has any; existing owners keep their codec.
    pub fn set_window_decoders(
        &self,
        decoders: Vec<crate::video_caps::WindowDecoder>,
    ) -> Result<(), Error> {
        let _engine = self.engine();
        if decoders.len() > crate::VideoCodec::ALL.len()
            || decoders.iter().enumerate().any(|(index, decoder)| {
                decoders[..index]
                    .iter()
                    .any(|earlier| earlier.codec == decoder.codec)
            })
        {
            return Err(Error::InvalidSetting("duplicate window decoder"));
        }
        self.alive_state()?.window_decoders = Some(decoders);
        Ok(())
    }

    /// Whether the app's window draws the host's cursor itself, from a
    /// cursor sent as its own stream. Desktop owners opened from now on ask
    /// for that stream; a viewer enables this only when it renders cursor
    /// metadata.
    pub fn set_cursor_receiver(&self, enabled: bool) -> Result<(), Error> {
        let _engine = self.engine();
        self.ensure_alive()?;
        self.shared
            .cursor_receiver
            .store(enabled, std::sync::atomic::Ordering::Release);
        Ok(())
    }

    /// Saves a preference without changing existing owners or requiring a decoder now.
    pub fn set_desktop_codec_default(&self, codec: Option<crate::VideoCodec>) -> Result<(), Error> {
        let _engine = self.engine();
        self.alive_state()?.desktop_codec_default = codec;
        Ok(())
    }

    pub fn set_desktop_quality_defaults(&self, quality: Quality) -> Result<(), Error> {
        let _engine = self.engine();
        let quality = Quality::new(
            quality.max_bitrate_kbps,
            quality.max_frame_rate,
            quality.max_width,
            quality.max_height,
        )?;
        self.alive_state()?.desktop_quality_default = quality;
        Ok(())
    }
}

fn video_targets_locked(shared: &Shared, state: &State) -> Vec<VideoTarget> {
    let topology = lock(&shared.topology).clone();
    let revision_of = |display_id: u64| {
        topology
            .displays
            .iter()
            .find(|display| display.id == display_id)
            .map(|display| display.revision)
            .unwrap_or(0)
    };
    let mut targets = Vec::new();
    for record in state.hosted_services.values() {
        if record.info.kind != crate::ServiceKind::Desktop
            || record.info.phase != crate::ServicePhase::Active
            || *record.stop.borrow()
        {
            continue;
        }
        let Some(desktop) = &record.desktop else {
            continue;
        };
        let info = desktop.metadata();
        let display_id = lock(&desktop.selected).first().copied().unwrap_or(0);
        targets.push(VideoTarget {
            id: info.service.id,
            generation: info.service.generation,
            quality: info.quality,
            separate_cursor: info.separate_cursor,
            codec: info.codec,
            matrix: info.matrix,
            display_id,
            revision: revision_of(display_id),
        });
    }
    targets.sort_by_key(|target| target.id);
    targets
}

pub(super) fn video_routing_locked(shared: &Shared, state: &State) -> VideoRouting {
    let targets = video_targets_locked(shared, state);
    let default_source = targets
        .iter()
        .filter_map(|t| (t.display_id != 0).then_some(t.display_id))
        .min()
        .or_else(|| lock(&shared.topology).displays.first().map(|d| d.id));
    VideoRouting {
        targets,
        default_source,
    }
}
