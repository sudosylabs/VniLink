//! Native OS-action ownership: lock, restart, shutdown, privacy mode.

use crate::{error::map_error, FfiError, VnilinkCore};
use std::sync::Arc;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FfiSystemAction {
    Lock,
    Restart,
    Shutdown,
}

impl From<FfiSystemAction> for vnilink::SystemAction {
    fn from(value: FfiSystemAction) -> Self {
        match value {
            FfiSystemAction::Lock => Self::Lock,
            FfiSystemAction::Restart => Self::Restart,
            FfiSystemAction::Shutdown => Self::Shutdown,
        }
    }
}

impl From<vnilink::SystemAction> for FfiSystemAction {
    fn from(value: vnilink::SystemAction) -> Self {
        match value {
            vnilink::SystemAction::Lock => Self::Lock,
            vnilink::SystemAction::Restart => Self::Restart,
            vnilink::SystemAction::Shutdown => Self::Shutdown,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FfiOperationState {
    Admitted,
    Refused,
    Executed,
    Failed,
}

impl From<vnilink::OperationState> for FfiOperationState {
    fn from(value: vnilink::OperationState) -> Self {
        match value {
            vnilink::OperationState::Admitted => Self::Admitted,
            vnilink::OperationState::Refused => Self::Refused,
            vnilink::OperationState::Executed => Self::Executed,
            vnilink::OperationState::Failed => Self::Failed,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FfiSystemOperation {
    pub id: u64,
    pub action: FfiSystemAction,
    pub state: FfiOperationState,
    pub completion_observable: bool,
    pub detail: String,
}

impl From<vnilink::SystemOperation> for FfiSystemOperation {
    fn from(value: vnilink::SystemOperation) -> Self {
        Self {
            id: value.id,
            action: value.action.into(),
            state: value.state.into(),
            completion_observable: value.completion_observable,
            detail: value.detail,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FfiSystemSupport {
    pub lock: bool,
    pub restart: bool,
    pub shutdown: bool,
    pub privacy: bool,
    pub input_block: bool,
}

impl From<vnilink::SystemSupport> for FfiSystemSupport {
    fn from(value: vnilink::SystemSupport) -> Self {
        Self {
            lock: value.lock,
            restart: value.restart,
            shutdown: value.shutdown,
            privacy: value.privacy,
            input_block: value.input_block,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FfiSessionState {
    pub locked: Option<bool>,
    pub login_screen: Option<bool>,
    pub headless: bool,
    pub session: String,
}

impl From<vnilink::HostSessionState> for FfiSessionState {
    fn from(value: vnilink::HostSessionState) -> Self {
        Self {
            locked: value.locked,
            login_screen: value.login_screen,
            headless: value.headless,
            session: value.session,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FfiHostSystemReport {
    pub support: FfiSystemSupport,
    pub session: FfiSessionState,
}

pub use vnilink::SystemPolicy as FfiSystemPolicy;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FfiPrivacyState {
    pub display_private: bool,
    pub input_blocked: bool,
    pub holder_connection: Option<u64>,
}

impl From<vnilink::PrivacyState> for FfiPrivacyState {
    fn from(value: vnilink::PrivacyState) -> Self {
        Self {
            display_private: value.display_private,
            input_blocked: value.input_blocked,
            holder_connection: value.holder_connection,
        }
    }
}

/// One opened System pipe. Dropping it ends only this owner; close it
/// explicitly to release a held privacy lease deterministically.
pub struct SystemServiceHandle {
    inner: vnilink::RemoteSystemService,
}

impl SystemServiceHandle {
    pub fn is_cancelled(&self) -> bool {
        self.inner.is_cancelled()
    }

    pub fn close(&self) {
        self.inner.close();
    }

    pub fn execute(&self, action: FfiSystemAction) -> Result<FfiSystemOperation, FfiError> {
        self.inner
            .execute(action.into())
            .map(Into::into)
            .map_err(map_error)
    }

    pub fn operation(&self, id: u64) -> Result<FfiSystemOperation, FfiError> {
        self.inner.operation(id).map(Into::into).map_err(map_error)
    }

    pub fn support(&self) -> Result<FfiHostSystemReport, FfiError> {
        self.inner
            .support()
            .map(|(support, session)| FfiHostSystemReport {
                support: support.into(),
                session: session.into(),
            })
            .map_err(map_error)
    }

    pub fn set_privacy(
        &self,
        display_private: bool,
        input_blocked: bool,
    ) -> Result<FfiPrivacyState, FfiError> {
        self.inner
            .set_privacy(display_private, input_blocked)
            .map(Into::into)
            .map_err(map_error)
    }

    pub fn privacy(&self) -> Result<FfiPrivacyState, FfiError> {
        self.inner.privacy().map(Into::into).map_err(map_error)
    }
}

impl VnilinkCore {
    pub fn open_system_service(
        &self,
        id: u64,
        generation: u64,
    ) -> Result<Arc<SystemServiceHandle>, FfiError> {
        self.inner
            .open_system_service(id, generation)
            .map(|inner| Arc::new(SystemServiceHandle { inner }))
            .map_err(map_error)
    }

    pub fn system_policy(&self) -> Result<FfiSystemPolicy, FfiError> {
        self.inner
            .system_policy()
            .map(Into::into)
            .map_err(map_error)
    }

    pub fn set_system_policy(&self, policy: FfiSystemPolicy) -> Result<(), FfiError> {
        self.inner
            .set_system_policy(policy.into())
            .map_err(map_error)
    }

    pub fn system_support(&self) -> Result<FfiHostSystemReport, FfiError> {
        self.inner
            .system_support()
            .map(|(support, session)| FfiHostSystemReport {
                support: support.into(),
                session: session.into(),
            })
            .map_err(map_error)
    }

    pub fn system_privacy(&self) -> Result<FfiPrivacyState, FfiError> {
        self.inner
            .system_privacy()
            .map(Into::into)
            .map_err(map_error)
    }

    /// Installs the platform OS-action backend: macOS workstation lock,
    /// Linux logind/ScreenSaver actions, Windows lock/restart/shutdown and
    /// input blocking. Facilities without a supported API stay unsupported
    /// and are reported as such; nothing is faked.
    pub fn enable_native_system_actions(&self) -> Result<(), FfiError> {
        #[cfg(target_os = "macos")]
        {
            self.inner
                .set_system_actions(Some(Arc::new(vnilink_macos::MacSystemActions::default())))
                .map_err(map_error)
        }
        #[cfg(target_os = "linux")]
        {
            self.inner
                .set_system_actions(Some(Arc::new(vnilink_linux::LinuxSystemActions::default())))
                .map_err(map_error)
        }
        #[cfg(windows)]
        {
            self.inner
                .set_system_actions(Some(Arc::new(
                    vnilink_windows::WindowsSystemActions::default(),
                )))
                .map_err(map_error)
        }
        #[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
        {
            Err(map_error(vnilink::Error::Unsupported(
                "native system actions",
            )))
        }
    }

    pub fn disable_native_system_actions(&self) -> Result<(), FfiError> {
        self.inner.set_system_actions(None).map_err(map_error)
    }
}
