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

/// Host admission. Iroh has already authenticated the peer key.
/// Files-only sessions (Transfer grant without Control) are admitted without
/// video requirements and without screen-capture permission beyond hosting:
/// file roots/listings/transfers enforce the Transfer grant per operation.
pub(crate) fn admit(
    hosting: bool,
    session_open: bool,
    control_granted: bool,
    transfer_granted: bool,
    files_only: bool,
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
    if files_only {
        // Files-only: Transfer grant suffices, no Control, video, input or
        // screen sharing required. Old hosts without v4 reject as incompatible.
        if !transfer_granted {
            return Err(Error::Unauthorized);
        }
        return Ok(());
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
    if !(5..=crate::protocol::PROTOCOL_VERSION).contains(&version) {
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

/// Default encoder limits the controller offers when it dials.
pub(crate) fn controller_offer() -> Quality {
    Quality::new(2_000, 30, 1920, 1080).expect("default offer is valid")
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
