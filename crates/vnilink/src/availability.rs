//! Effective capability availability and durable-policy registry.
//!
//! Availability is the intersection of implemented support, OS
//! authorization, device state, host policy and peer grant. This module owns
//! the pure model: [`evaluate`] combines caller-supplied facts into an
//! [`Availability`] with every applicable reason, ordered so the first reason
//! names the most fundamental blocker. The [`Core`](crate::Core) query wires
//! the facts this installation actually tracks, including published
//! [`BackendState`]; facts nobody published stay `false`/`None` rather
//! than being guessed.

use crate::error::Error;

/// One assessable capability. Finer than [`crate::ServiceKind`]: viewing is
/// distinct from input injection, and each OS action stands alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Capability {
    DesktopView,
    DesktopControl,
    Clipboard,
    Files,
    Terminal,
    Camera,
    SystemAudio,
    Microphone,
    SystemLock,
    SystemRestart,
    SystemShutdown,
    SystemPrivacy,
    Recording,
}

impl Capability {
    /// Short stable label for logs and diagnostics. No user data.
    pub fn label(self) -> &'static str {
        match self {
            Self::DesktopView => "desktop-view",
            Self::DesktopControl => "desktop-control",
            Self::Clipboard => "clipboard",
            Self::Files => "files",
            Self::Terminal => "terminal",
            Self::Camera => "camera",
            Self::SystemAudio => "system-audio",
            Self::Microphone => "microphone",
            Self::SystemLock => "system-lock",
            Self::SystemRestart => "system-restart",
            Self::SystemShutdown => "system-shutdown",
            Self::SystemPrivacy => "system-privacy",
            Self::Recording => "recording",
        }
    }
}

/// One reason a capability is not usable right now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AvailabilityReason {
    /// No backend, factory or adapter is installed for this capability.
    MissingImplementation,
    /// The OS refused or was never granted the required consent.
    DeniedOsPermission,
    /// No device is present, or the device is busy or lost.
    UnavailableDevice,
    /// A host-local policy switch or bound forbids it.
    DisabledLocalPolicy,
    /// The paired peer lacks the grant.
    DeniedPeerGrant,
    /// At capacity; counts are a snapshot, not a reservation.
    BusyResource { used: u32, limit: u32 },
    /// The backend failed. Detail is a short human-readable reason.
    BackendFailed { detail: String },
}

impl AvailabilityReason {
    /// Short stable label for logs and diagnostics. No user data.
    pub fn label(&self) -> &'static str {
        match self {
            Self::MissingImplementation => "missing-implementation",
            Self::DeniedOsPermission => "denied-os-permission",
            Self::UnavailableDevice => "unavailable-device",
            Self::DisabledLocalPolicy => "disabled-local-policy",
            Self::DeniedPeerGrant => "denied-peer-grant",
            Self::BusyResource { .. } => "busy-resource",
            Self::BackendFailed { .. } => "backend-failed",
        }
    }
}

/// Facts about one capability at one moment. Every field must be something
/// the caller actually knows; unknown OS/device state stays `false` and the
/// operation itself reports the failure.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AvailabilityInputs {
    pub implemented: bool,
    pub os_denied: bool,
    pub device_missing: bool,
    pub policy_allowed: bool,
    pub peer_granted: bool,
    /// `Some((used, limit))` when the capability has a countable capacity.
    pub capacity: Option<(u32, u32)>,
    /// `Some(detail)` when the backend is known failed.
    pub backend_failed: Option<String>,
}

/// Last-known backend facts for one capability, published by whoever
/// observed them: native adapters report OS consent and device presence
/// they can see, and Core serve loops record factory failures they hit.
/// Last writer wins; a fresh success clears only the failure detail, and
/// an explicit clear drops the whole entry. Absent means unknown, which
/// the query reports as no blocker rather than guessing.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BackendState {
    pub os_denied: bool,
    pub device_missing: bool,
    /// `Some(detail)` while the backend is known failed. Bounded to the
    /// same 256 bytes `evaluate` keeps; publishers send short labels.
    pub failed: Option<String>,
}

/// Effective availability with every applicable reason, most fundamental
/// first: implementation, OS permission, device, policy, grant, capacity,
/// backend failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Availability {
    pub capability: Capability,
    pub available: bool,
    pub reasons: Vec<AvailabilityReason>,
}

impl Availability {
    /// One-line human summary without secrets: `files: unavailable
    /// (denied-peer-grant)`.
    pub fn summarize(&self) -> String {
        if self.available {
            return format!("{}: available", self.capability.label());
        }
        let reasons: Vec<&str> = self.reasons.iter().map(|r| r.label()).collect();
        format!(
            "{}: unavailable ({})",
            self.capability.label(),
            reasons.join(", ")
        )
    }
}

pub fn evaluate(capability: Capability, inputs: &AvailabilityInputs) -> Availability {
    let mut reasons = Vec::new();
    if !inputs.implemented {
        reasons.push(AvailabilityReason::MissingImplementation);
    }
    if inputs.os_denied {
        reasons.push(AvailabilityReason::DeniedOsPermission);
    }
    if inputs.device_missing {
        reasons.push(AvailabilityReason::UnavailableDevice);
    }
    if !inputs.policy_allowed {
        reasons.push(AvailabilityReason::DisabledLocalPolicy);
    }
    if !inputs.peer_granted {
        reasons.push(AvailabilityReason::DeniedPeerGrant);
    }
    if let Some((used, limit)) = inputs.capacity {
        if used >= limit {
            reasons.push(AvailabilityReason::BusyResource { used, limit });
        }
    }
    if let Some(detail) = inputs.backend_failed.as_ref() {
        reasons.push(AvailabilityReason::BackendFailed {
            detail: truncate(detail.clone()),
        });
    }
    Availability {
        capability,
        available: reasons.is_empty(),
        reasons,
    }
}

/// When a durable policy change takes effect.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PolicyChangeScope {
    /// Applies to the next admission or check immediately after save.
    Immediate,
    /// Applies to newly admitted work; live work keeps its prior terms.
    NextOwner,
    /// Refused while governed work is live; the saved policy therefore
    /// describes every live instance.
    IdleGated,
}

/// One durable host policy with its change semantics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PolicyDescriptor {
    pub name: &'static str,
    pub scope: PolicyChangeScope,
    pub summary: &'static str,
}

/// Every durable behavior policy in the installation record. Scopes match
/// the setters: terminal refuses while shells live, file-resource
/// reductions govern new reservations only, the rest apply immediately.
pub const POLICIES: &[PolicyDescriptor] = &[
    PolicyDescriptor {
        name: "terminal",
        scope: PolicyChangeScope::IdleGated,
        summary: "admitted only while no shell is alive or starting",
    },
    PolicyDescriptor {
        name: "file-retention",
        scope: PolicyChangeScope::Immediate,
        summary: "GC reads the latest policy; active jobs stay protected",
    },
    PolicyDescriptor {
        name: "file-resource",
        scope: PolicyChangeScope::NextOwner,
        summary: "reductions govern new reservations; live work keeps its own",
    },
    PolicyDescriptor {
        name: "power",
        scope: PolicyChangeScope::Immediate,
        summary: "inhibitor and idle checks read the saved policy",
    },
    PolicyDescriptor {
        name: "system",
        scope: PolicyChangeScope::Immediate,
        summary: "the next action checks the saved policy",
    },
];

pub fn policy_descriptor(name: &str) -> Result<PolicyDescriptor, Error> {
    POLICIES
        .iter()
        .find(|descriptor| descriptor.name == name)
        .copied()
        .ok_or(Error::InvalidSetting("unknown policy"))
}

pub(crate) fn truncate(detail: String) -> String {
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
