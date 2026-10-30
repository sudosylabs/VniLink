// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! Headless terminal operations for future native terminal views.
use crate::{error::map_error, FfiError, FfiServiceInfo, VnilinkCore};
use std::sync::Arc;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FfiTerminalSize {
    pub rows: u16,
    pub columns: u16,
}
impl From<FfiTerminalSize> for vnilink::terminal::TerminalSize {
    fn from(size: FfiTerminalSize) -> Self {
        Self {
            rows: size.rows,
            columns: size.columns,
        }
    }
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FfiTerminalEvent {
    Output { bytes: Vec<u8> },
    Exited { code: u32 },
    Failed { message: String },
    OutputGap { lost_bytes: u64 },
}
pub struct TerminalService {
    inner: vnilink::RemoteTerminalService,
}

impl TerminalService {
    pub fn info(&self) -> FfiServiceInfo {
        self.inner.info().clone().into()
    }
    pub fn is_cancelled(&self) -> bool {
        self.inner.is_cancelled()
    }
    pub fn input(&self, bytes: Vec<u8>) -> Result<(), FfiError> {
        self.inner.input(&bytes).map_err(map_error)
    }
    pub fn resize(&self, size: FfiTerminalSize) -> Result<(), FfiError> {
        self.inner.resize(size.into()).map_err(map_error)
    }
    pub fn poll(&self) -> Option<FfiTerminalEvent> {
        self.inner.poll().map(|event| match event {
            vnilink::terminal::TerminalEvent::Output(bytes) => FfiTerminalEvent::Output { bytes },
            vnilink::terminal::TerminalEvent::Exited(code) => FfiTerminalEvent::Exited { code },
            vnilink::terminal::TerminalEvent::OutputGap(lost_bytes) => {
                FfiTerminalEvent::OutputGap { lost_bytes }
            }
            vnilink::terminal::TerminalEvent::Failed(message) => {
                FfiTerminalEvent::Failed { message }
            }
        })
    }
    pub fn close(&self) {
        self.inner.close();
    }
}

impl VnilinkCore {
    /// Host chooses an absolute shell and working directory. This does not grant
    /// any peer permission and is not enabled at application startup.
    pub fn enable_native_terminal(
        &self,
        program: String,
        directory: String,
    ) -> Result<(), FfiError> {
        #[cfg(unix)]
        {
            let factory =
                vnilink::terminal::NativeTerminalFactory::new(program.into(), directory.into())
                    .map_err(map_error)?;
            self.inner
                .set_terminal_factory(Some(Arc::new(factory)))
                .map_err(map_error)
        }
        #[cfg(windows)]
        {
            let factory =
                vnilink_windows::WindowsTerminalFactory::new(program.into(), directory.into())
                    .map_err(map_error)?;
            self.inner
                .set_terminal_factory(Some(Arc::new(factory)))
                .map_err(map_error)
        }
        #[cfg(not(any(unix, windows)))]
        {
            let _ = (program, directory);
            Err(map_error(vnilink::Error::Unsupported(
                "native terminal backend on this platform",
            )))
        }
    }
    pub fn disable_terminal(&self) -> Result<(), FfiError> {
        self.inner.set_terminal_factory(None).map_err(map_error)
    }
    pub fn open_terminal_service(
        &self,
        id: u64,
        generation: u64,
        size: FfiTerminalSize,
    ) -> Result<Arc<TerminalService>, FfiError> {
        self.inner
            .open_terminal_service(id, generation, size.into())
            .map(|inner| Arc::new(TerminalService { inner }))
            .map_err(map_error)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FfiTerminalSessionInfo {
    pub id: String,
    pub process_id: u32,
    pub shell: String,
    pub rows: u16,
    pub columns: u16,
    pub persistent: bool,
    pub attached: bool,
}
impl From<vnilink::terminal::TerminalSessionInfo> for FfiTerminalSessionInfo {
    fn from(value: vnilink::terminal::TerminalSessionInfo) -> Self {
        Self {
            id: value.id,
            process_id: value.process_id,
            shell: value.shell,
            rows: value.size.rows,
            columns: value.size.columns,
            persistent: value.persistent,
            attached: value.attached,
        }
    }
}

impl TerminalService {
    pub fn session(&self) -> FfiTerminalSessionInfo {
        self.inner.session().into()
    }
}

impl VnilinkCore {
    pub fn open_persistent_terminal_service(
        &self,
        id: u64,
        generation: u64,
        size: FfiTerminalSize,
        resume_id: Option<String>,
    ) -> Result<Arc<TerminalService>, FfiError> {
        self.inner
            .open_terminal_service_with_options(
                id,
                generation,
                vnilink::terminal::TerminalOptions {
                    size: size.into(),
                    persistent: true,
                    resume_id,
                },
            )
            .map(|inner| Arc::new(TerminalService { inner }))
            .map_err(map_error)
    }
    pub fn hosted_terminal_sessions(&self) -> Result<Vec<FfiTerminalSessionInfo>, FfiError> {
        self.inner
            .hosted_terminal_sessions()
            .map(|s| s.into_iter().map(Into::into).collect())
            .map_err(map_error)
    }
    pub fn close_terminal_session(&self, id: String) -> Result<(), FfiError> {
        self.inner.close_terminal_session(&id).map_err(map_error)
    }
}

impl VnilinkCore {
    pub fn remote_terminal_sessions(
        &self,
        connection_id: u64,
        generation: u64,
    ) -> Result<Vec<FfiTerminalSessionInfo>, FfiError> {
        self.inner
            .remote_terminal_sessions(connection_id, generation)
            .map(|sessions| sessions.into_iter().map(Into::into).collect())
            .map_err(map_error)
    }
    pub fn close_remote_terminal_session(
        &self,
        connection_id: u64,
        generation: u64,
        id: String,
    ) -> Result<(), FfiError> {
        self.inner
            .close_remote_terminal_session(connection_id, generation, &id)
            .map_err(map_error)
    }
}

impl VnilinkCore {
    pub fn enable_default_native_terminal(&self) -> Result<(), FfiError> {
        #[cfg(unix)]
        {
            self.inner
                .set_terminal_factory(Some(Arc::new(
                    vnilink::terminal::NativeTerminalFactory::for_current_user()
                        .map_err(map_error)?,
                )))
                .map_err(map_error)
        }
        #[cfg(windows)]
        {
            self.inner
                .set_terminal_factory(Some(Arc::new(
                    vnilink_windows::WindowsTerminalFactory::for_current_user()
                        .map_err(map_error)?,
                )))
                .map_err(map_error)
        }
        #[cfg(not(any(unix, windows)))]
        {
            Err(map_error(vnilink::Error::Unsupported(
                "native terminal backend on this platform",
            )))
        }
    }
}

/// A shell this computer can offer, by the id the local user picks it with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FfiShell {
    pub id: String,
    pub label: String,
}
/// The shells this computer can offer, what "automatic" resolves to, and the
/// one paired computers get now (`None` when terminals are not offered).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FfiTerminalShells {
    pub shells: Vec<FfiShell>,
    pub automatic: String,
    pub current: Option<String>,
}

fn host_shells() -> Result<
    (
        Vec<vnilink::terminal::ShellChoice>,
        vnilink::terminal::ShellChoice,
    ),
    FfiError,
> {
    #[cfg(unix)]
    {
        Ok((
            vnilink::terminal::available_shells(),
            vnilink::terminal::automatic_shell().map_err(map_error)?,
        ))
    }
    #[cfg(windows)]
    {
        let shells = vnilink_windows::available_shells();
        let automatic = vnilink_windows::automatic_of(&shells)
            .cloned()
            .ok_or_else(|| map_error(vnilink::Error::Unsupported("host shell")))?;
        Ok((shells, automatic))
    }
    #[cfg(not(any(unix, windows)))]
    {
        Err(map_error(vnilink::Error::Unsupported(
            "native terminal backend on this platform",
        )))
    }
}

impl VnilinkCore {
    /// The shells the local user may offer to paired computers.
    pub fn terminal_shells(&self) -> Result<FfiTerminalShells, FfiError> {
        let (shells, automatic) = host_shells()?;
        Ok(FfiTerminalShells {
            shells: shells
                .into_iter()
                .map(|shell| FfiShell {
                    id: shell.id,
                    label: shell.label,
                })
                .collect(),
            automatic: automatic.id,
            current: self.inner.terminal_shell_id().map_err(map_error)?,
        })
    }
    /// Offers the shell the local user chose, or the automatic one for `None`.
    /// A peer never reaches this: it names a detected shell by id, and is
    /// refused with Busy while any shell is alive or starting.
    pub fn set_terminal_shell(&self, id: Option<String>) -> Result<(), FfiError> {
        let (shells, automatic) = host_shells()?;
        let shell = match id {
            None => automatic,
            Some(id) => shells
                .into_iter()
                .find(|shell| shell.id == id)
                .ok_or_else(|| {
                    map_error(vnilink::Error::InvalidSetting(
                        "that shell is not installed on this computer",
                    ))
                })?,
        };
        self.inner
            .replace_terminal_factory(factory_for(&shell)?)
            .map_err(map_error)
    }
}

fn factory_for(
    shell: &vnilink::terminal::ShellChoice,
) -> Result<Arc<dyn vnilink::terminal::HostTerminalFactory>, FfiError> {
    #[cfg(unix)]
    {
        Ok(Arc::new(
            vnilink::terminal::NativeTerminalFactory::for_shell(shell).map_err(map_error)?,
        ))
    }
    #[cfg(windows)]
    {
        Ok(Arc::new(
            vnilink_windows::WindowsTerminalFactory::for_shell(shell).map_err(map_error)?,
        ))
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = shell;
        Err(map_error(vnilink::Error::Unsupported(
            "native terminal backend on this platform",
        )))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FfiTerminalPolicy {
    pub allow_persistence: bool,
    pub max_sessions: u16,
    pub max_sessions_per_peer: u16,
    pub history_bytes: u32,
    pub replay_bytes: u32,
    pub detached_timeout_secs: u32,
}
impl From<vnilink::terminal::TerminalPolicy> for FfiTerminalPolicy {
    fn from(p: vnilink::terminal::TerminalPolicy) -> Self {
        Self {
            allow_persistence: p.allow_persistence,
            max_sessions: p.max_sessions,
            max_sessions_per_peer: p.max_sessions_per_peer,
            history_bytes: p.history_bytes,
            replay_bytes: p.replay_bytes,
            detached_timeout_secs: p.detached_timeout_secs,
        }
    }
}
impl From<FfiTerminalPolicy> for vnilink::terminal::TerminalPolicy {
    fn from(p: FfiTerminalPolicy) -> Self {
        Self {
            allow_persistence: p.allow_persistence,
            max_sessions: p.max_sessions,
            max_sessions_per_peer: p.max_sessions_per_peer,
            history_bytes: p.history_bytes,
            replay_bytes: p.replay_bytes,
            detached_timeout_secs: p.detached_timeout_secs,
        }
    }
}

impl VnilinkCore {
    pub fn terminal_policy(&self) -> Result<FfiTerminalPolicy, FfiError> {
        self.inner
            .terminal_policy()
            .map(Into::into)
            .map_err(map_error)
    }
    pub fn set_terminal_policy(&self, policy: FfiTerminalPolicy) -> Result<(), FfiError> {
        self.inner
            .set_terminal_policy(policy.into())
            .map_err(map_error)
    }
}
