use crate::error::Error;
use crate::protocol::{Capabilities, Quality, PROTOCOL_VERSION};

/// Which side of a session this installation is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionRole {
    Controller,
    Host,
}

/// Session lifecycle from the README. `Idle` is the absence of a session.
/// `Reconnecting` is part of the model and is not entered yet.
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

/// Host admission. Iroh has already authenticated the peer key.
pub(crate) fn admit(
    hosting: bool,
    session_open: bool,
    control_granted: bool,
    version: u16,
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
    Ok(())
}

/// Input is accepted only for the generation that owns the active session.
pub(crate) fn check_input(
    active: Option<(u64, u64)>,
    session_id: u64,
    generation: u64,
) -> Result<(), Error> {
    match active {
        Some((id, current)) if id == session_id && current == generation => Ok(()),
        Some(_) | None => Err(Error::StaleSession),
    }
}

pub(crate) fn phase_after_admit() -> SessionPhase {
    // Capture and codecs are still unsupported, so negotiation has nothing to
    // agree beyond the control channel itself.
    let _capabilities = Capabilities::UNSUPPORTED;
    SessionPhase::Active
}
