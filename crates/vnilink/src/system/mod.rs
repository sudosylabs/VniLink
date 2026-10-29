//! Remote operating-system actions: workstation lock, restart, shutdown,
//! privacy mode and local-input blocking.
//!
//! Actions are discrete operations on an explicitly granted System service,
//! not consequences of desktop permission. Sending a request is receipt, not
//! proof the machine acted: restart and shutdown complete without an
//! observable reply, and operations are never replayed after reconnect.
//!
//! A restart or shutdown first waits [`SYSTEM_ACTION_NOTICE`], so the person
//! at the computer can save their work or cancel; a lock runs at once.

/// Persistent owner thread for Windows local-input blocking. The native
/// contract binds blocking to its thread; this module is also compiled
/// under `test` on every platform so the thread-affine fake validates
/// the protocol without Windows.
#[cfg(any(target_os = "windows", test))]
pub mod owner;

use crate::error::Error;

/// How long a restart or shutdown waits before it runs. The person at the
/// computer can cancel or start it sooner; so can the computer that asked.
pub const SYSTEM_ACTION_NOTICE: std::time::Duration = std::time::Duration::from_secs(30);

/// One destructive or session-level host operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SystemAction {
    Lock,
    Restart,
    Shutdown,
}

impl SystemAction {
    pub(crate) fn wire(self) -> u8 {
        match self {
            Self::Lock => 1,
            Self::Restart => 2,
            Self::Shutdown => 3,
        }
    }

    pub(crate) fn decode(byte: u8) -> Result<Self, Error> {
        match byte {
            1 => Ok(Self::Lock),
            2 => Ok(Self::Restart),
            3 => Ok(Self::Shutdown),
            _ => Err(Error::Malformed),
        }
    }

    pub(crate) fn completion_observable(self) -> bool {
        matches!(self, Self::Lock)
    }
}

/// Outcome of one admitted operation. Operations never move again after
/// reaching `Refused`, `Executed`, `Failed` or `Cancelled`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OperationState {
    /// The host admitted the request; execution is pending or unobservable.
    Admitted,
    /// Grant, policy or backend support refused the request.
    Refused,
    /// The native call returned success.
    Executed,
    /// The native call failed.
    Failed,
    /// A restart or shutdown is counting down its notice.
    Scheduled,
    /// The notice ended without running: cancelled at either computer, or
    /// the computer that asked went away.
    Cancelled,
}

impl OperationState {
    pub(crate) fn wire(self) -> u8 {
        match self {
            Self::Admitted => 1,
            Self::Refused => 2,
            Self::Executed => 3,
            Self::Failed => 4,
            Self::Scheduled => 5,
            Self::Cancelled => 6,
        }
    }

    pub(crate) fn decode(byte: u8) -> Result<Self, Error> {
        match byte {
            1 => Ok(Self::Admitted),
            2 => Ok(Self::Refused),
            3 => Ok(Self::Executed),
            4 => Ok(Self::Failed),
            5 => Ok(Self::Scheduled),
            6 => Ok(Self::Cancelled),
            _ => Err(Error::Malformed),
        }
    }
}

/// Identity and outcome of one action request. The host assigns `id`; the
/// controller queries it on the same connection generation. A restart or
/// shutdown reply may never arrive; its absence is not proof either way.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SystemOperation {
    pub id: u64,
    pub action: SystemAction,
    pub state: OperationState,
    pub completion_observable: bool,
    /// Short human-readable reason. Never carries secrets or paths.
    pub detail: String,
    /// Time left in the notice while `Scheduled`; zero otherwise.
    pub notice_remaining_ms: u32,
}

/// A restart or shutdown counting down on this computer, for the notice its
/// user sees.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScheduledSystemAction {
    pub operation_id: u64,
    pub action: SystemAction,
    /// The computer that asked.
    pub endpoint_id: String,
    pub remaining_ms: u32,
}

/// Native facilities this installation can actually invoke. A `true` bit
/// means the call exists and is attempted; OS policy can still refuse at
/// execution time, which surfaces as a `Failed` operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SystemSupport {
    pub lock: bool,
    pub restart: bool,
    pub shutdown: bool,
    pub privacy: bool,
    pub input_block: bool,
}

impl SystemSupport {
    pub(crate) fn bits(self) -> u8 {
        u8::from(self.lock)
            | u8::from(self.restart) << 1
            | u8::from(self.shutdown) << 2
            | u8::from(self.privacy) << 3
            | u8::from(self.input_block) << 4
    }

    pub(crate) fn decode(bits: u8) -> Result<Self, Error> {
        if bits & !31 != 0 {
            return Err(Error::Malformed);
        }
        Ok(Self {
            lock: bits & 1 != 0,
            restart: bits & 2 != 0,
            shutdown: bits & 4 != 0,
            privacy: bits & 8 != 0,
            input_block: bits & 16 != 0,
        })
    }
}

/// Current local privacy/input state and which connection holds it, if any.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PrivacyState {
    pub display_private: bool,
    pub input_blocked: bool,
    pub holder_connection: Option<u64>,
}

/// Best-effort host session boundaries. `None` means the platform cannot
/// determine the value; unknown is reported, never guessed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostSessionState {
    pub locked: Option<bool>,
    pub login_screen: Option<bool>,
    pub headless: bool,
    /// Short platform session label, e.g. `aqua`, `x11`, `wayland`, `win32`.
    pub session: String,
}

impl HostSessionState {
    pub fn unknown(session: &str) -> Self {
        Self {
            locked: None,
            login_screen: None,
            headless: false,
            session: session.into(),
        }
    }
}

/// Host-local policy for remote OS actions. Restart and shutdown stay denied
/// until the local owner explicitly allows them; the per-peer System grant is
/// still required for every action.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SystemPolicy {
    pub allow_lock: bool,
    pub allow_restart: bool,
    pub allow_shutdown: bool,
    pub allow_privacy: bool,
}

impl Default for SystemPolicy {
    fn default() -> Self {
        Self {
            allow_lock: true,
            allow_restart: false,
            allow_shutdown: false,
            allow_privacy: false,
        }
    }
}

impl SystemPolicy {
    pub fn validate(self) -> Result<Self, Error> {
        Ok(self)
    }

    pub(crate) fn bits(self) -> u8 {
        u8::from(self.allow_lock)
            | u8::from(self.allow_restart) << 1
            | u8::from(self.allow_shutdown) << 2
            | u8::from(self.allow_privacy) << 3
    }

    pub(crate) fn from_bits(bits: u8) -> Result<Self, Error> {
        if bits & !15 != 0 {
            return Err(Error::CorruptRecord);
        }
        Ok(Self {
            allow_lock: bits & 1 != 0,
            allow_restart: bits & 2 != 0,
            allow_shutdown: bits & 4 != 0,
            allow_privacy: bits & 8 != 0,
        })
    }

    pub(crate) fn allows(self, action: SystemAction) -> bool {
        match action {
            SystemAction::Lock => self.allow_lock,
            SystemAction::Restart => self.allow_restart,
            SystemAction::Shutdown => self.allow_shutdown,
        }
    }
}

/// Native OS-action backend. Calls run on a blocking worker, never on a
/// Tokio I/O thread. Implementations must be quick and idempotent: restore
/// calls run on revoke, parent loss and shutdown.
pub trait HostSystemActions: Send + Sync {
    fn support(&self) -> SystemSupport;
    fn lock_workstation(&self) -> Result<(), Error>;
    fn restart(&self) -> Result<(), Error>;
    fn shutdown(&self) -> Result<(), Error>;
    fn set_display_private(&self, private: bool) -> Result<(), Error>;
    fn set_input_blocked(&self, blocked: bool) -> Result<(), Error>;
    /// Native observation when restrictions can expire or be locally released.
    fn observed_privacy(&self) -> Option<(bool, bool)> {
        None
    }
    fn session_state(&self) -> HostSessionState;
}

/// One retained operation with the connection generation that owns it.
/// Queries on another generation fail; operations never replay.
#[derive(Debug, Clone)]
pub(crate) struct SystemOperationRecord {
    pub operation: SystemOperation,
    pub connection_id: u64,
    pub generation: u64,
    pub peer: [u8; 32],
    /// While `Scheduled`: when it runs, and what wakes its countdown early.
    pub notice: Option<Notice>,
}

#[derive(Debug, Clone)]
pub(crate) struct Notice {
    pub deadline: std::time::Instant,
    pub run_now: bool,
    pub wake: std::sync::Arc<tokio::sync::Notify>,
}

/// Single global privacy/input-block holder. Only the holding System owner
/// can change it; close, revoke, parent loss and shutdown restore it.
/// The lease retains the backend that applied the restriction: restoration
/// always targets it, never the current adapter slot (a replacement may
/// have swung the slot since the lease was granted).
#[derive(Clone)]
pub(crate) struct PrivacyLease {
    pub connection_id: u64,
    pub service_id: u64,
    pub display_private: bool,
    pub input_blocked: bool,
    pub actions: std::sync::Arc<dyn HostSystemActions>,
}

impl std::fmt::Debug for PrivacyLease {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PrivacyLease")
            .field("connection_id", &self.connection_id)
            .field("service_id", &self.service_id)
            .field("display_private", &self.display_private)
            .field("input_blocked", &self.input_blocked)
            .finish_non_exhaustive()
    }
}

pub(crate) fn truncate_detail(detail: String) -> String {
    const MAX: usize = 256;
    if detail.len() <= MAX {
        return detail;
    }
    let mut end = MAX;
    while !detail.is_char_boundary(end) {
        end -= 1;
    }
    detail[..end].to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn operation_detail_truncates_on_char_boundaries() {
        let long = "é".repeat(300);
        let truncated = truncate_detail(long);
        assert!(truncated.len() <= 256);
        assert!(truncated.chars().all(|c| c == 'é'));
    }
}
