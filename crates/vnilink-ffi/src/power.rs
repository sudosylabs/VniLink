//! Host-local policy and observed native inhibition; no peer authority is implied.
use crate::{error::map_error, FfiError, VnilinkCore};
use std::sync::Arc;
use vnilink::power::{PowerKind, PowerPhase, PowerPolicy};

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Record)]
pub struct FfiPowerPolicy {
    pub incoming_display: bool,
    pub incoming_system: bool,
    pub outgoing_display: bool,
    pub outgoing_system: bool,
    pub incoming_idle_secs: u32,
    pub outgoing_idle_secs: u32,
    pub lock_after_control: bool,
}
impl From<PowerPolicy> for FfiPowerPolicy {
    fn from(p: PowerPolicy) -> Self {
        Self {
            incoming_display: p.incoming_display,
            incoming_system: p.incoming_system,
            outgoing_display: p.outgoing_display,
            outgoing_system: p.outgoing_system,
            incoming_idle_secs: p.incoming_idle_secs,
            outgoing_idle_secs: p.outgoing_idle_secs,
            lock_after_control: p.lock_after_control,
        }
    }
}
impl From<FfiPowerPolicy> for PowerPolicy {
    fn from(p: FfiPowerPolicy) -> Self {
        Self {
            incoming_display: p.incoming_display,
            incoming_system: p.incoming_system,
            outgoing_display: p.outgoing_display,
            outgoing_system: p.outgoing_system,
            incoming_idle_secs: p.incoming_idle_secs,
            outgoing_idle_secs: p.outgoing_idle_secs,
            lock_after_control: p.lock_after_control,
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum FfiPowerKind {
    Display,
    System,
}
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Enum)]
pub enum FfiPowerPhase {
    Idle,
    Unavailable,
    Starting,
    Active,
    Failed { message: String },
}
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct FfiPowerStatus {
    pub kind: FfiPowerKind,
    pub owners: u32,
    pub phase: FfiPowerPhase,
}
#[uniffi::export]
impl VnilinkCore {
    pub fn power_policy(&self) -> Result<FfiPowerPolicy, FfiError> {
        self.inner.power_policy().map(Into::into).map_err(map_error)
    }
    pub fn set_power_policy(&self, policy: FfiPowerPolicy) -> Result<(), FfiError> {
        self.inner
            .set_power_policy(policy.into())
            .map_err(map_error)
    }
    /// Registration alone does not acquire a native assertion. Policy and live
    /// Desktop owners determine demand; OS rejection is observable in status.
    pub fn enable_native_power(&self, enabled: bool) -> Result<(), FfiError> {
        let factory: Option<Arc<dyn vnilink::power::HostPowerFactory>> = if enabled {
            #[cfg(target_os = "macos")]
            {
                Some(Arc::new(vnilink_macos::MacPowerFactory))
            }
            #[cfg(target_os = "linux")]
            {
                Some(Arc::new(vnilink_linux::LinuxPowerFactory))
            }
            #[cfg(windows)]
            {
                Some(Arc::new(vnilink_windows::WindowsPowerFactory))
            }
            #[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
            {
                return Err(map_error(vnilink::Error::Unsupported(
                    "native power backend on this platform",
                )));
            }
        } else {
            None
        };
        self.inner.set_power_factory(factory).map_err(map_error)
    }
    pub fn power_status(&self) -> Result<Vec<FfiPowerStatus>, FfiError> {
        self.inner
            .power_status()
            .map(|states| {
                states
                    .into_iter()
                    .map(|s| FfiPowerStatus {
                        kind: match s.kind {
                            PowerKind::Display => FfiPowerKind::Display,
                            PowerKind::System => FfiPowerKind::System,
                        },
                        owners: s.owners,
                        phase: match s.phase {
                            PowerPhase::Idle => FfiPowerPhase::Idle,
                            PowerPhase::Unavailable => FfiPowerPhase::Unavailable,
                            PowerPhase::Starting => FfiPowerPhase::Starting,
                            PowerPhase::Active => FfiPowerPhase::Active,
                            PowerPhase::Failed(message) => FfiPowerPhase::Failed { message },
                        },
                    })
                    .collect()
            })
            .map_err(map_error)
    }
}
