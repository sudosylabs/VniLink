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
/// Opt-in installation policy. Connections carrying only Files, detached shells,
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
    /// grant-narrowing count as control ends. Only control that was used
    /// arms it: input applied here, or a picture delivered while
    /// controlling. Viewing alone, or control ended before it was used,
    /// never arms it.
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
    /// Control was used: input from it was applied, or a picture reached
    /// it. A grant that was never used, such as a viewer cancelled while
    /// still connecting, does not lock this computer when it ends.
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
    /// `controlling` is whether a hosted desktop holds control; `used` is whether control was used since
    /// the last tick, which arms even when control already ended within
    /// it; `parent_live` is any Active incoming session. Disabling the
    /// policy disarms immediately.
    pub(crate) fn tick(
        &mut self,
        now: std::time::Instant,
        enabled: bool,
        controlling: bool,
        used: bool,
        parent_live: bool,
    ) -> LockDecision {
        if !enabled {
            self.armed = false;
            self.grace_until = None;
            return LockDecision::Idle;
        }
        if used {
            self.armed = true;
        }
        if controlling {
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    /// Every policy of the five known bits round-trips its record byte, and
    /// a byte with any other bit set is a corrupt record.
    #[test]
    fn a_power_policy_is_its_five_bits_and_no_other() {
        for bits in 0..=31 {
            assert_eq!(
                PowerPolicy::from_bits(bits).unwrap().bits(),
                bits,
                "a known policy round-trips"
            );
        }
        for bits in 32..=255 {
            assert!(
                matches!(
                    PowerPolicy::from_bits(bits),
                    Err(crate::Error::CorruptRecord)
                ),
                "an unknown bit is a corrupt record"
            );
        }
    }

    /// Pure policy, no network: the machine never fires unused, fires once on
    /// an explicit end with a live parent, waits out the grace without one, and
    /// a redial inside the window resumes instead of locking.
    #[test]
    fn lock_machine_fires_once_and_graces_parent_loss() {
        let start = Instant::now();
        let mut machine = LockAfterControl::default();
        // tick(now, enabled, controlling, used, parent_live)
        assert_eq!(
            machine.tick(start, true, false, false, false),
            LockDecision::Idle
        );
        assert_eq!(
            machine.tick(start, true, true, true, true),
            LockDecision::Idle
        );
        assert_eq!(
            machine.tick(start, false, false, false, true),
            LockDecision::Idle
        );
        assert_eq!(
            machine.tick(start, true, true, true, true),
            LockDecision::Idle
        );
        assert_eq!(
            machine.tick(start, true, false, false, true),
            LockDecision::Fire
        );
        assert_eq!(
            machine.tick(start, true, false, false, true),
            LockDecision::Idle
        );
        assert_eq!(
            machine.tick(start, true, true, true, false),
            LockDecision::Idle
        );
        assert_eq!(
            machine.tick(start, true, false, false, false),
            LockDecision::Wait
        );
        assert_eq!(
            machine.tick(start + Duration::from_secs(29), true, false, false, false),
            LockDecision::Wait
        );
        assert_eq!(
            machine.tick(start + Duration::from_secs(30), true, false, false, false),
            LockDecision::Fire
        );
        assert_eq!(
            machine.tick(start, true, true, true, false),
            LockDecision::Idle
        );
        assert_eq!(
            machine.tick(start, true, false, false, false),
            LockDecision::Wait
        );
        assert_eq!(
            machine.tick(start + Duration::from_secs(5), true, true, false, true),
            LockDecision::Idle
        );
        assert_eq!(
            machine.tick(start + Duration::from_secs(31), true, false, false, true),
            LockDecision::Fire
        );
        // A use and an end within one tick still fire on the next sample.
        let mut quick = LockAfterControl::default();
        assert_eq!(
            quick.tick(start, true, false, true, true),
            LockDecision::Fire
        );
    }

    /// Control granted but never used, such as a viewer cancelled while still
    /// connecting, ends without locking; using it once arms the lock until
    /// control ends.
    #[test]
    fn lock_machine_ignores_control_that_was_never_used() {
        let start = Instant::now();
        let mut machine = LockAfterControl::default();
        assert_eq!(
            machine.tick(start, true, true, false, true),
            LockDecision::Idle
        );
        assert_eq!(
            machine.tick(start, true, false, false, true),
            LockDecision::Idle
        );
        assert_eq!(
            machine.tick(start, true, false, false, false),
            LockDecision::Idle
        );
        assert_eq!(
            machine.tick(start, true, true, true, true),
            LockDecision::Idle
        );
        assert_eq!(
            machine.tick(start, true, true, false, true),
            LockDecision::Idle
        );
        assert_eq!(
            machine.tick(start, true, false, false, true),
            LockDecision::Fire
        );
    }
}
