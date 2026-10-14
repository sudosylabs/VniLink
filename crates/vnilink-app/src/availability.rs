//! Native capability-availability queries and policy registry.

use crate::{error::map_error, FfiError, VnilinkCore};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FfiCapability {
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

impl From<FfiCapability> for vnilink::Capability {
    fn from(value: FfiCapability) -> Self {
        match value {
            FfiCapability::DesktopView => Self::DesktopView,
            FfiCapability::DesktopControl => Self::DesktopControl,
            FfiCapability::Clipboard => Self::Clipboard,
            FfiCapability::Files => Self::Files,
            FfiCapability::Terminal => Self::Terminal,
            FfiCapability::Camera => Self::Camera,
            FfiCapability::SystemAudio => Self::SystemAudio,
            FfiCapability::Microphone => Self::Microphone,
            FfiCapability::SystemLock => Self::SystemLock,
            FfiCapability::SystemRestart => Self::SystemRestart,
            FfiCapability::SystemShutdown => Self::SystemShutdown,
            FfiCapability::SystemPrivacy => Self::SystemPrivacy,
            FfiCapability::Recording => Self::Recording,
        }
    }
}

impl From<vnilink::Capability> for FfiCapability {
    fn from(value: vnilink::Capability) -> Self {
        match value {
            vnilink::Capability::DesktopView => Self::DesktopView,
            vnilink::Capability::DesktopControl => Self::DesktopControl,
            vnilink::Capability::Clipboard => Self::Clipboard,
            vnilink::Capability::Files => Self::Files,
            vnilink::Capability::Terminal => Self::Terminal,
            vnilink::Capability::Camera => Self::Camera,
            vnilink::Capability::SystemAudio => Self::SystemAudio,
            vnilink::Capability::Microphone => Self::Microphone,
            vnilink::Capability::SystemLock => Self::SystemLock,
            vnilink::Capability::SystemRestart => Self::SystemRestart,
            vnilink::Capability::SystemShutdown => Self::SystemShutdown,
            vnilink::Capability::SystemPrivacy => Self::SystemPrivacy,
            vnilink::Capability::Recording => Self::Recording,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FfiAvailabilityReason {
    MissingImplementation,
    DeniedOsPermission,
    UnavailableDevice,
    DisabledLocalPolicy,
    DeniedPeerGrant,
    BusyResource { used: u32, limit: u32 },
    BackendFailed { detail: String },
}

impl From<vnilink::AvailabilityReason> for FfiAvailabilityReason {
    fn from(value: vnilink::AvailabilityReason) -> Self {
        match value {
            vnilink::AvailabilityReason::MissingImplementation => Self::MissingImplementation,
            vnilink::AvailabilityReason::DeniedOsPermission => Self::DeniedOsPermission,
            vnilink::AvailabilityReason::UnavailableDevice => Self::UnavailableDevice,
            vnilink::AvailabilityReason::DisabledLocalPolicy => Self::DisabledLocalPolicy,
            vnilink::AvailabilityReason::DeniedPeerGrant => Self::DeniedPeerGrant,
            vnilink::AvailabilityReason::BusyResource { used, limit } => {
                Self::BusyResource { used, limit }
            }
            vnilink::AvailabilityReason::BackendFailed { detail } => Self::BackendFailed { detail },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FfiAvailability {
    pub capability: FfiCapability,
    pub available: bool,
    pub reasons: Vec<FfiAvailabilityReason>,
    pub summary: String,
}

/// Last-known backend facts for one capability. Native adapters publish OS
/// consent and device presence they observe; unknown stays unpublished.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FfiBackendState {
    pub os_denied: bool,
    pub device_missing: bool,
    pub failed: Option<String>,
}

impl From<FfiBackendState> for vnilink::BackendState {
    fn from(value: FfiBackendState) -> Self {
        Self {
            os_denied: value.os_denied,
            device_missing: value.device_missing,
            failed: value.failed,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FfiPolicyChangeScope {
    Immediate,
    NextOwner,
    IdleGated,
}

impl From<vnilink::PolicyChangeScope> for FfiPolicyChangeScope {
    fn from(value: vnilink::PolicyChangeScope) -> Self {
        match value {
            vnilink::PolicyChangeScope::Immediate => Self::Immediate,
            vnilink::PolicyChangeScope::NextOwner => Self::NextOwner,
            vnilink::PolicyChangeScope::IdleGated => Self::IdleGated,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FfiPolicyDescriptor {
    pub name: String,
    pub scope: FfiPolicyChangeScope,
    pub summary: String,
}

impl VnilinkCore {
    /// Effective availability of one capability. `endpoint_id` selects the
    /// peer whose grant is evaluated; `None` reports this installation's
    /// support without grant checks. Reasons are ordered most fundamental
    /// first; counts are a snapshot, not a reservation.
    pub fn capability_availability(
        &self,
        endpoint_id: Option<String>,
        capability: FfiCapability,
    ) -> Result<FfiAvailability, FfiError> {
        self.inner
            .capability_availability(endpoint_id.as_deref(), capability.into())
            .map(|availability| {
                let summary = availability.summarize();
                FfiAvailability {
                    capability: availability.capability.into(),
                    available: availability.available,
                    reasons: availability.reasons.into_iter().map(Into::into).collect(),
                    summary,
                }
            })
            .map_err(map_error)
    }

    /// Bounded JSON diagnostic snapshot: stages, phases, counts and queue
    /// depths. Never carries screen, clipboard, terminal, token, secret or
    /// path payload; peers appear as short fingerprints. At most 64 KiB;
    /// oversized lists shed entries with an explicit truncation marker.
    pub fn diagnostic_snapshot_json(&self) -> Result<String, FfiError> {
        self.inner
            .diagnostic_snapshot()
            .and_then(|snapshot| vnilink::export_json(&snapshot))
            .map_err(map_error)
    }

    /// Publishes last-known backend facts for one capability, replacing
    /// any earlier entry. Failure detail is truncated to 256 bytes.
    pub fn report_backend_state(
        &self,
        capability: FfiCapability,
        state: FfiBackendState,
    ) -> Result<(), FfiError> {
        self.inner
            .report_backend_state(capability.into(), state.into())
            .map_err(map_error)
    }

    pub fn report_os_permission(
        &self,
        capability: FfiCapability,
        denied: bool,
    ) -> Result<(), FfiError> {
        self.inner
            .report_os_permission(capability.into(), denied)
            .map_err(map_error)
    }
    pub fn report_backend_failure(
        &self,
        capability: FfiCapability,
        failure: Option<String>,
    ) -> Result<(), FfiError> {
        self.inner
            .report_backend_failure(capability.into(), failure)
            .map_err(map_error)
    }
    /// Drops any published backend facts for one capability.
    pub fn clear_backend_state(&self, capability: FfiCapability) -> Result<(), FfiError> {
        self.inner
            .clear_backend_state(capability.into())
            .map_err(map_error)
    }

    /// Every durable behavior policy with its change semantics.
    pub fn policy_descriptors(&self) -> Vec<FfiPolicyDescriptor> {
        vnilink::POLICIES
            .iter()
            .map(|descriptor| FfiPolicyDescriptor {
                name: descriptor.name.into(),
                scope: descriptor.scope.into(),
                summary: descriptor.summary.into(),
            })
            .collect()
    }
}
