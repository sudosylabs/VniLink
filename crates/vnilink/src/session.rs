use crate::error::Error;
use crate::events::{CoreEvent, EventHub};
use crate::protocol::{Quality, SessionStamp, PROTOCOL_VERSION};

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
    if version != PROTOCOL_VERSION {
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

/// Default encoder limits the controller offers when it dials.
pub(crate) fn controller_offer() -> Quality {
    Quality::new(2_000, 30, 1920, 1080).expect("default offer is valid")
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
