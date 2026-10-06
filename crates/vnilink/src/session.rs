use crate::error::Error;
use crate::events::{CoreEvent, EventHub};
use crate::protocol::{Quality, SessionStamp};

/// Which side of a session this installation is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionRole {
    Controller,
    Host,
}

/// Session lifecycle from the README. `Idle` is the absence of a session.
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
    pub quality: Option<Quality>,
    /// Pixel size of the one host monitor. Zero when the host has not reported it.
    pub monitor_width: u16,
    pub monitor_height: u16,
}

/// Separate grants. Pairing does not set any of them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Grants {
    pub control: bool,
    pub clipboard: bool,
    pub transfer: bool,
}

/// One named grant. [`crate::trust::Trust::granted`] answers all three
/// through this so the mapping lives in one method.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Grant {
    Control,
    Clipboard,
    Transfer,
}

/// Host admission. Iroh has already authenticated the peer key.
pub(crate) fn admit(
    hosting: bool,
    session_open: bool,
    control_granted: bool,
    version: u16,
    video_unsupported: bool,
    codec_supported: bool,
    peer_decode_unsupported: bool,
) -> Result<(), Error> {
    if !hosting {
        return Err(Error::NotHosting);
    }
    if !(crate::protocol::MIN_PROTOCOL_VERSION..=crate::protocol::PROTOCOL_VERSION)
        .contains(&version)
    {
        return Err(Error::IncompatibleVersion);
    }
    if session_open {
        return Err(Error::Busy);
    }
    if !control_granted {
        return Err(Error::Unauthorized);
    }
    if video_unsupported {
        return Err(Error::Unsupported("no H.264 encoder is available"));
    }
    if !codec_supported {
        return Err(Error::IncompatibleVersion);
    }
    if peer_decode_unsupported {
        return Err(Error::Unsupported("the controller cannot decode video"));
    }
    Ok(())
}

/// Agreed H.264 codec for the live session: the lower of offered and locally
/// encodable. A Baseline offer always agrees Baseline; a High offer agrees
/// High only when this host encodes it. Downgrade is silent and safe because
/// every High decoder also decodes Baseline. Returns `None` for an unknown
/// codec id, which the host rejects.
pub fn negotiate_codec(offered: u8, host_high: bool) -> Option<u8> {
    match offered {
        crate::protocol::CODEC_H264 => Some(crate::protocol::CODEC_H264),
        crate::protocol::CODEC_H264_HIGH if host_high => Some(crate::protocol::CODEC_H264_HIGH),
        crate::protocol::CODEC_H264_HIGH => Some(crate::protocol::CODEC_H264),
        _ => None,
    }
}
/// Default encoder limits the controller offers when it dials.
pub(crate) fn controller_offer() -> Quality {
    Quality::new(2_000, 30, 1920, 1080).expect("default offer is valid")
}

/// Ordered quality ladder for automatic mode, low to high. Clear paths can
/// reach Retina detail without a manual override; congestion steps back down.
const LADDER: [(u32, u8, u16, u16); 6] = [
    (800, 10, 854, 480),
    (1_500, 15, 1280, 720),
    (2_000, 30, 1920, 1080),
    (5_000, 30, 1920, 1080),
    (12_000, 60, 2560, 1440),
    (24_000, 60, 3840, 2160),
];

/// One adaptive step based on transport congestion. Downshift is eager
/// (queue, drops, high RTT/relay); upshift needs all-clear. Returns `None`
/// to hold. Pure function so hosts, controllers, and tests share one policy.
pub fn suggest_quality(
    current: &Quality,
    rtt_ms: u64,
    relay: bool,
    queued: usize,
    dropped_delta: u64,
) -> Option<Quality> {
    let index = LADDER
        .iter()
        .position(|&(bitrate, fps, _, _)| {
            current.max_bitrate_kbps == bitrate && current.max_frame_rate == fps
        })
        .unwrap_or(2);
    let strained = queued >= 6 || dropped_delta > 0 || rtt_ms >= 150 || (relay && rtt_ms >= 80);
    if strained {
        if index == 0 {
            return None;
        }
        let (bitrate, fps, width, height) = LADDER[index - 1];
        return Quality::new(bitrate, fps, width, height).ok();
    }
    let clear = queued == 0 && dropped_delta == 0 && rtt_ms < 50 && !relay;
    if clear && index + 1 < LADDER.len() {
        let (bitrate, fps, width, height) = LADDER[index + 1];
        return Quality::new(bitrate, fps, width, height).ok();
    }
    None
}

/// Agreed limits for the live session: the offer clamped to the host monitor.
/// A zero monitor side is unknown, so that axis keeps the offered value.
pub(crate) fn agree_quality(offered: &Quality, monitor: (u16, u16)) -> Quality {
    let width = if monitor.0 > 0 {
        offered.max_width.min(monitor.0)
    } else {
        offered.max_width
    };
    let height = if monitor.1 > 0 {
        offered.max_height.min(monitor.1)
    } else {
        offered.max_height
    };
    Quality::new(
        offered.max_bitrate_kbps,
        offered.max_frame_rate,
        width.max(1),
        height.max(1),
    )
    .unwrap_or(*offered)
}

/// Input is accepted only for the generation that owns the active session.
pub(crate) fn check_input(active: Option<SessionStamp>, stamp: SessionStamp) -> Result<(), Error> {
    match active {
        Some(current) if current == stamp => Ok(()),
        Some(_) | None => Err(Error::StaleSession),
    }
}

pub(crate) fn phase_after_admit() -> SessionPhase {
    // Admission agrees version, monitor, codec, and encoder limits, so the
    // session lands active with the negotiated quality already stored.
    SessionPhase::Active
}

/// Emit the progression both roles synthesize after admission. The phases are
/// identical on host and controller today; the agreed version, monitor,
/// codec, and quality land in the session beside it.
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
