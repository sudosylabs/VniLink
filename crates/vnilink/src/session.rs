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
pub(crate) fn check_input(active: Option<SessionStamp>, stamp: SessionStamp) -> Result<(), Error> {
    match active {
        Some(current) if current == stamp => Ok(()),
        Some(_) | None => Err(Error::StaleSession),
    }
}

pub(crate) fn phase_after_admit() -> SessionPhase {
    // Negotiation has nothing to agree beyond the control channel itself
    // until capture and codecs are negotiated, so admission lands active.
    SessionPhase::Active
}

/// Emit the progression both roles synthesize after admission. The phases are
/// identical on host and controller today; when negotiation becomes real, the
/// agreed version, monitor, codec, and grants land here instead of beside it.
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
