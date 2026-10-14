//! Host-local sleep inhibition, independent from remote input and peer grants.
use crate::{BackendStartup, Error};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PowerKind {
    Display,
    System,
}
impl PowerKind {
    pub(crate) fn index(self) -> usize {
        match self {
            Self::Display => 0,
            Self::System => 1,
        }
    }
    pub(crate) const ALL: [Self; 2] = [Self::Display, Self::System];
}
/// Opt-in installation policy. Files-only connections, detached shells,
/// keepalives and ordinary frame traffic do not acquire these assertions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PowerPolicy {
    pub incoming_display: bool,
    pub incoming_system: bool,
    pub outgoing_display: bool,
    pub outgoing_system: bool,
    /// Zero disables owner-scoped Desktop inactivity expiry; otherwise <= 24h.
    pub incoming_idle_secs: u32,
    pub outgoing_idle_secs: u32,
    /// Lock this workstation after the last remote-control owner ends:
    /// explicit owner close with a live parent locks at once; an end with
    /// no live parent waits out the reconnect grace so temporary network
    /// loss does not lock a session that is about to resume. Yielding and
    /// grant-narrowing count as control ends. Viewing alone never arms it.
    pub lock_after_control: bool,
}
impl PowerPolicy {
    pub(crate) fn validate(self) -> Result<Self, Error> {
        if self.incoming_idle_secs > 86400 || self.outgoing_idle_secs > 86400 {
            return Err(Error::InvalidSetting(
                "Desktop idle timeout exceeds 24 hours",
            ));
        }
        Ok(self)
    }
    pub(crate) fn bits(self) -> u8 {
        u8::from(self.incoming_display)
            | (u8::from(self.incoming_system) << 1)
            | (u8::from(self.outgoing_display) << 2)
            | (u8::from(self.outgoing_system) << 3)
            | (u8::from(self.lock_after_control) << 4)
    }
    pub(crate) fn from_bits(bits: u8) -> Result<Self, Error> {
        if bits & !31 != 0 {
            return Err(Error::CorruptRecord);
        }
        Ok(Self {
            incoming_display: bits & 1 != 0,
            incoming_system: bits & 2 != 0,
            outgoing_display: bits & 4 != 0,
            outgoing_system: bits & 8 != 0,
            lock_after_control: bits & 16 != 0,
            ..Default::default()
        })
    }
    pub(crate) fn wants(self, hosted: bool, kind: PowerKind) -> bool {
        match (hosted, kind) {
            (true, PowerKind::Display) => self.incoming_display,
            (true, PowerKind::System) => self.incoming_system,
            (false, PowerKind::Display) => self.outgoing_display,
            (false, PowerKind::System) => self.outgoing_system,
        }
    }
}
/// Reconnect grace after control ends with no live parent session. A
/// redial inside the window resumes instead of locking; past it the end
/// stands. Monotonic: wall-clock changes never shorten or extend it.
pub(crate) const LOCK_GRACE: std::time::Duration = std::time::Duration::from_secs(30);

/// Edge-triggered lock-after-control state machine. Fed live facts by the
/// power tick; pure over its inputs so the policy is unit-testable.
#[derive(Debug, Default)]
pub(crate) struct LockAfterControl {
    armed: bool,
    grace_until: Option<std::time::Instant>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LockDecision {
    /// Fire the workstation lock once.
    Fire,
    /// Armed and waiting out the reconnect grace.
    Wait,
    /// Nothing to do.
    Idle,
}

impl LockAfterControl {
    /// Arms synchronously on a control grant. Grant sites call this with
    /// the state lock held so a grant-and-end cycle briefer than a tick
    /// still locks; ends are observed by [`Self::tick`].
    pub(crate) fn note_controlling(&mut self) {
        self.armed = true;
        self.grace_until = None;
    }

    /// `controlling` is any live remote-control owner (hosted Controlling
    /// or legacy control session); `parent_live` is any Active incoming
    /// session. Disabling the policy disarms immediately.
    pub(crate) fn tick(
        &mut self,
        now: std::time::Instant,
        enabled: bool,
        controlling: bool,
        parent_live: bool,
    ) -> LockDecision {
        if !enabled {
            self.armed = false;
            self.grace_until = None;
            return LockDecision::Idle;
        }
        if controlling {
            self.armed = true;
            self.grace_until = None;
            return LockDecision::Idle;
        }
        if !self.armed {
            return LockDecision::Idle;
        }
        if parent_live {
            // The owner ended but its session lives: explicit close, yield
            // or revoke. Lock at once.
            self.armed = false;
            self.grace_until = None;
            return LockDecision::Fire;
        }
        match self.grace_until {
            None => {
                self.grace_until = Some(now + LOCK_GRACE);
                LockDecision::Wait
            }
            Some(deadline) if now >= deadline => {
                self.armed = false;
                self.grace_until = None;
                LockDecision::Fire
            }
            Some(_) => LockDecision::Wait,
        }
    }
}

/// Dropping the last engine lease must release its OS assertion/FD/cookie.
pub trait PowerInhibitor: Send {}
pub trait HostPowerFactory: Send + Sync {
    /// Native calls run off the async runtime. Check startup between calls and
    /// release any acquired assertion if cancellation or the deadline wins.
    fn acquire(
        &self,
        kind: PowerKind,
        startup: &BackendStartup,
    ) -> Result<Box<dyn PowerInhibitor>, Error>;
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PowerPhase {
    Idle,
    Unavailable,
    Starting,
    Active,
    Failed(String),
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PowerStatus {
    pub kind: PowerKind,
    pub owners: u32,
    pub phase: PowerPhase,
}
impl PowerStatus {
    pub(crate) fn idle(kind: PowerKind) -> Self {
        Self {
            kind,
            owners: 0,
            phase: PowerPhase::Idle,
        }
    }
}
