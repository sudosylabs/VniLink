// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

use crate::error::Error;
use crate::events::{CoreEvent, EventHub};
use crate::protocol::Quality;

/// Which side of a session this installation is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionRole {
    Controller,
    Host,
}

/// Session lifecycle. `Idle` is the absence of a session.
/// The controller enters `Reconnecting` after a dropped link, before the next
/// dial. The host closes the old generation first, then accepts the new one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionPhase {
    Connecting,
    Authenticated,
    Authorized,
    Negotiating,
    Active,
    Reconnecting,
    Stopping,
    Closed,
    Failed(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionInfo {
    pub id: u64,
    pub generation: u64,
    pub role: SessionRole,
    pub phase: SessionPhase,
    pub peer_id: String,
}

/// Separate grants. Pairing does not set any of them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Grants {
    /// Permission to receive Desktop pixels, independent of input injection.
    pub view: bool,
    pub control: bool,
    pub clipboard: bool,
    pub transfer: bool,
}

/// One named grant. [`crate::trust::Trust::granted`] answers all three
/// through this so the mapping lives in one method.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Grant {
    View,
    Control,
    Clipboard,
    Transfer,
}

/// Neutral transport gate. Pairing and an explicit host enablement admit a
/// connection, but every feature still needs its own service permission.
pub(crate) fn admit_services(
    hosting: bool,
    open: bool,
    paired: bool,
    version: u16,
) -> Result<(), Error> {
    if !hosting {
        return Err(Error::NotHosting);
    }
    if version != crate::protocol::PROTOCOL_VERSION {
        return Err(Error::IncompatibleVersion);
    }
    if !paired {
        return Err(Error::NotPaired);
    }
    if open {
        return Err(Error::Busy);
    }
    Ok(())
}

/// Default limits the controller dials with. The bitrate is the most a
/// stream may use, and the default sets none beyond what the protocol
/// carries: the rate follows the picture and the link below it.
pub(crate) fn controller_offer() -> Quality {
    Quality::new(crate::protocol::MAX_BITRATE_KBPS, 30, 1920, 1080).expect("default offer is valid")
}

/// What a stream starts at under the controller's `limits`: the bitrate
/// starts low, within the limit, and adaptation raises it while the picture
/// and the link allow.
pub(crate) fn dial_quality(limits: Quality) -> Quality {
    Quality {
        max_bitrate_kbps: limits.max_bitrate_kbps.min(crate::bitrate::START_KBPS),
        ..limits
    }
}

/// Agreed limits for the live session: the offer clamped to the host monitor.
/// A zero monitor side is unknown, so that axis keeps the offered value.
pub(crate) fn agree_quality(
    offered: &Quality,
    monitor: (u16, u16),
    caps: &crate::video_caps::VideoCodecCaps,
) -> Quality {
    crate::video_caps::clamp_quality(offered, monitor, caps)
}

/// What the host starts a session at: the offer agreed, and its bitrate
/// held to what the agreed picture can use, so a small or slow picture
/// never begins above its ceiling and is cut at once.
pub(crate) fn start_quality(
    offered: &Quality,
    monitor: (u16, u16),
    caps: &crate::video_caps::VideoCodecCaps,
    codec: u8,
) -> Quality {
    let agreed = agree_quality(offered, monitor, caps);
    let Some(codec) = crate::VideoCodec::from_wire_id(codec) else {
        return agreed;
    };
    let size = crate::bitrate::encode_size((agreed.max_width, agreed.max_height), monitor);
    Quality {
        max_bitrate_kbps: agreed.max_bitrate_kbps.min(crate::bitrate::picture_kbps(
            codec,
            size,
            agreed.max_frame_rate,
        )),
        ..agreed
    }
}

pub(crate) fn phase_after_admit() -> SessionPhase {
    // Admission agrees the version and grants in one exchange, so the
    // session lands active.
    SessionPhase::Active
}

/// Emit the progression both roles synthesize after admission. The phases are
/// identical on host and controller.
pub(crate) fn announce_progression(
    events: &EventHub,
    id: u64,
    generation: u64,
    role: SessionRole,
    peer_id: &str,
) {
    for phase in [
        SessionPhase::Authenticated,
        SessionPhase::Authorized,
        SessionPhase::Negotiating,
        SessionPhase::Active,
    ] {
        events.publish(CoreEvent::SessionChanged {
            id,
            generation,
            role,
            phase,
            peer_id: peer_id.to_owned(),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::PROTOCOL_VERSION;
    use crate::video_caps::VideoCodecCaps;

    #[test]
    fn the_host_starts_a_small_or_slow_picture_at_what_it_can_use() {
        let caps = crate::video_caps::VideoCodecCaps::h264_full(4096, 4096, "test");
        let offer = dial_quality(controller_offer());
        // 640x360 at 30 fps can use 1,105 kb/s as H.264: no start above it.
        let small = start_quality(&offer, (640, 360), &caps, crate::protocol::CODEC_H264);
        assert_eq!(
            small,
            Quality::new(1105, 30, 640, 360).unwrap(),
            "a start above the ceiling is cut at once, and an encoder that cannot retune reopens"
        );
        // A picture that can use more starts at the common start.
        let large = start_quality(&offer, (1920, 1080), &caps, crate::protocol::CODEC_H264);
        assert_eq!(large.max_bitrate_kbps, crate::bitrate::START_KBPS);
        // The viewer's limit below both is kept.
        let saver = Quality::new(500, 30, 1920, 1080).unwrap();
        assert_eq!(
            start_quality(&saver, (1920, 1080), &caps, crate::protocol::CODEC_H264)
                .max_bitrate_kbps,
            500
        );
    }

    #[test]
    fn a_stream_starts_low_within_the_limit_the_viewer_set() {
        // Automatic sets no limit: the stream starts at the common rate.
        let automatic = dial_quality(controller_offer());
        assert_eq!(automatic.max_bitrate_kbps, crate::bitrate::START_KBPS);
        assert_eq!(
            (
                automatic.max_frame_rate,
                automatic.max_width,
                automatic.max_height
            ),
            (30, 1920, 1080)
        );
        // A limit below it, such as Data saver's, is where it starts.
        let saver = Quality::new(2_000, 24, 1280, 720).unwrap();
        assert_eq!(dial_quality(saver), saver);
        // The frame rate and size are ceilings as chosen.
        let large = Quality::new(30_000, 60, 3840, 2160).unwrap();
        assert_eq!(
            dial_quality(large),
            Quality::new(crate::bitrate::START_KBPS, 60, 3840, 2160).unwrap()
        );
    }

    #[test]
    fn admission_checks_hosting_version_pairing_and_capacity() {
        assert!(matches!(
            admit_services(false, false, true, PROTOCOL_VERSION),
            Err(Error::NotHosting)
        ));
        for version in [PROTOCOL_VERSION - 1, PROTOCOL_VERSION + 1] {
            assert!(matches!(
                admit_services(true, false, true, version),
                Err(Error::IncompatibleVersion)
            ));
        }
        assert!(matches!(
            admit_services(true, false, false, PROTOCOL_VERSION),
            Err(Error::NotPaired)
        ));
        assert!(matches!(
            admit_services(true, true, true, PROTOCOL_VERSION),
            Err(Error::Busy)
        ));
        assert!(admit_services(true, false, true, PROTOCOL_VERSION).is_ok());
    }

    #[test]
    fn admission_order_prefers_hosting_and_the_control_channel_is_active() {
        assert!(matches!(
            admit_services(false, true, false, 0),
            Err(Error::NotHosting)
        ));
        assert_eq!(phase_after_admit(), SessionPhase::Active);
    }

    #[test]
    fn the_offer_clamps_to_the_host_monitor() {
        // The default limits: no bitrate limit beyond what a quality holds. The
        // monitor clamps the size and leaves the bitrate to the controller.
        let offer = controller_offer();
        assert_eq!(offer.max_bitrate_kbps, crate::protocol::MAX_BITRATE_KBPS);
        let caps = VideoCodecCaps::h264_full(4096, 4096, "test");
        assert_eq!(
            agree_quality(&offer, (800, 600), &caps),
            Quality::new(crate::protocol::MAX_BITRATE_KBPS, 30, 800, 600).unwrap()
        );
        // What is dialled starts low inside those limits, on every path.
        assert_eq!(
            agree_quality(&dial_quality(offer), (800, 600), &caps),
            Quality::new(crate::bitrate::START_KBPS, 30, 800, 600).unwrap()
        );
        assert_eq!(agree_quality(&offer, (0, 0), &caps), offer);
        assert_eq!(agree_quality(&offer, (4000, 3000), &caps), offer);
    }

    #[test]
    fn progression_emits_the_four_phases_in_order() {
        let hub = EventHub::new(8);
        let sub = hub.subscribe().unwrap();
        announce_progression(&hub, 7, 3, SessionRole::Host, "peer");
        let mut phases = Vec::new();
        while let Some(CoreEvent::SessionChanged { phase, .. }) = sub.try_recv() {
            phases.push(phase);
        }
        assert_eq!(
            phases,
            [
                SessionPhase::Authenticated,
                SessionPhase::Authorized,
                SessionPhase::Negotiating,
                SessionPhase::Active,
            ]
        );
    }

    #[test]
    fn neutral_admission_needs_pairing_host_enablement_and_the_protocol_version() {
        assert!(admit_services(true, false, true, PROTOCOL_VERSION).is_ok());
        assert!(matches!(
            admit_services(false, false, true, PROTOCOL_VERSION),
            Err(Error::NotHosting)
        ));
        assert!(matches!(
            admit_services(true, false, false, PROTOCOL_VERSION),
            Err(Error::NotPaired)
        ));
        assert!(matches!(
            admit_services(true, true, true, PROTOCOL_VERSION),
            Err(Error::Busy)
        ));
        for version in [0, PROTOCOL_VERSION + 1, 5, 6] {
            assert!(matches!(
                admit_services(true, false, true, version),
                Err(Error::IncompatibleVersion)
            ));
        }
    }
}
