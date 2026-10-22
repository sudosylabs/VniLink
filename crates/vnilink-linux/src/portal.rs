//! Wayland screen cast as a portal session.
//!
//! The phases are the ones the RemoteDesktop and ScreenCast portals require:
//! create a session, select a monitor, start, then close. A restore token is
//! remembered for a later session. One started session is that compositor's
//! permission. It is not universal unattended access, and it is not an X11
//! capture.

use std::fmt;

use vnilink_media::Error;

/// One portal permission does not cover every compositor.
pub const ONE_PORTAL_SESSION_IS_UNIVERSAL: bool = false;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PortalPhase {
    Idle,
    SessionCreated,
    SourcesSelected,
    Started,
    Closed,
    /// The user cancelled the portal's dialog, or the portal refused.
    Refused,
}

/// Local record of a portal screen-cast session.
///
/// [`Self::open_monitor`] talks to the session bus. The phase methods are the
/// same transitions, so tests can walk them without D-Bus.
pub struct PortalSession {
    phase: PortalPhase,
    restore_token: Option<String>,
    pipewire_node: Option<u32>,
}

impl PortalSession {
    pub fn new() -> Self {
        Self {
            phase: PortalPhase::Idle,
            restore_token: None,
            pipewire_node: None,
        }
    }

    pub fn phase(&self) -> PortalPhase {
        self.phase
    }

    pub fn pipewire_node(&self) -> Option<u32> {
        self.pipewire_node
    }

    /// Opaque token from a previous grant. Empty tokens are rejected.
    /// The value is not included in [`Debug`].
    pub fn set_restore_token(&mut self, token: impl Into<String>) -> Result<(), Error> {
        let token = token.into();
        if token.is_empty() {
            return Err(Error::InvalidSetting("restore token is empty"));
        }
        self.restore_token = Some(token);
        Ok(())
    }

    pub fn restore_token(&self) -> Option<&str> {
        self.restore_token.as_deref()
    }

    pub fn create_session(&mut self) -> Result<(), Error> {
        self.require(PortalPhase::Idle, "portal session was already created")?;
        self.phase = PortalPhase::SessionCreated;
        Ok(())
    }

    pub fn select_monitor(&mut self) -> Result<(), Error> {
        self.require(
            PortalPhase::SessionCreated,
            "portal sources were selected before the session existed",
        )?;
        self.phase = PortalPhase::SourcesSelected;
        Ok(())
    }

    pub fn start(&mut self, pipewire_node: u32) -> Result<(), Error> {
        self.require(
            PortalPhase::SourcesSelected,
            "portal stream started before a monitor was selected",
        )?;
        self.pipewire_node = Some(pipewire_node);
        self.phase = PortalPhase::Started;
        Ok(())
    }

    /// The portal answered a request with a refusal: its `Response` code 1
    /// (the user cancelled) or 2 (it ended the interaction otherwise).
    pub fn refuse(&mut self) -> Result<(), Error> {
        if matches!(
            self.phase,
            PortalPhase::Idle | PortalPhase::Closed | PortalPhase::Refused
        ) {
            return Err(Error::InvalidSetting("portal session is not open"));
        }
        self.pipewire_node = None;
        self.phase = PortalPhase::Refused;
        Ok(())
    }

    pub fn close(&mut self) -> Result<(), Error> {
        if matches!(self.phase, PortalPhase::Idle | PortalPhase::Closed) {
            return Err(Error::InvalidSetting("portal session is not open"));
        }
        self.pipewire_node = None;
        self.phase = PortalPhase::Closed;
        Ok(())
    }

    fn require(&self, expected: PortalPhase, message: &'static str) -> Result<(), Error> {
        if self.phase == expected {
            Ok(())
        } else {
            Err(Error::InvalidSetting(message))
        }
    }
}

impl Default for PortalSession {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for PortalSession {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PortalSession")
            .field("phase", &self.phase)
            .field("has_restore_token", &self.restore_token.is_some())
            .field("pipewire_node", &self.pipewire_node)
            .finish()
    }
}
