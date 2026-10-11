//! Opt-in terminal backends. Remote peers cannot choose a program or directory.
use crate::Error;

pub const TERMINAL_CHUNK: usize = 16 * 1024;
pub const TERMINAL_QUEUE: usize = 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct TerminalSize {
    pub rows: u16,
    pub columns: u16,
}
impl TerminalSize {
    pub fn validate(self) -> Result<Self, Error> {
        if self.rows == 0 || self.columns == 0 || self.rows > 512 || self.columns > 1024 {
            return Err(Error::InvalidSetting(
                "terminal dimensions are out of range",
            ));
        }
        Ok(self)
    }
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TerminalEvent {
    Output(Vec<u8>),
    Exited(u32),
    Failed(String),
    OutputGap(u64),
}
pub enum TerminalCommand {
    Input(Vec<u8>),
    Resize(TerminalSize),
    /// One queue admission for the reconnect redraw and final dimensions.
    Redraw(TerminalSize),
}

/// Installed by the host, independently from a peer's Terminal permission.
/// Open checks startup cancellation between allocation/spawn steps and releases
/// partially acquired PTY/process resources before returning an error.
pub trait HostTerminalFactory: Send + Sync {
    fn open(
        &self,
        size: TerminalSize,
        startup: crate::BackendStartup,
    ) -> Result<TerminalBackend, Error>;
}
/// A bounded backend bridge. Dropping it cancels the native terminal worker.
pub struct TerminalBackend {
    pub process_id: u32,
    pub shell: String,
    pub commands: std::sync::mpsc::SyncSender<TerminalCommand>,
    pub output: tokio::sync::mpsc::Receiver<TerminalEvent>,
    pub stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
}
impl Drop for TerminalBackend {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::Release);
    }
}

#[cfg(unix)]
mod native {
    use super::*;
    use portable_pty::{native_pty_system, CommandBuilder, PtySize};
    use std::{
        io::{Read, Write},
        sync::{
            atomic::{AtomicBool, Ordering},
            Arc,
        },
        time::Duration,
    };
    struct NativeProcess {
        master: Box<dyn portable_pty::MasterPty + Send>,
        child: Box<dyn portable_pty::Child + Send + Sync>,
    }
    impl Drop for NativeProcess {
        fn drop(&mut self) {
            // Stop both the foreground job and shell group. This is an ordinary
            // user terminal, not containment of deliberately detached processes.
            if let Some(group) = self.master.process_group_leader().filter(|pid| *pid > 0) {
                let _ = nix::sys::signal::killpg(
                    nix::unistd::Pid::from_raw(group),
                    nix::sys::signal::Signal::SIGKILL,
                );
            }
            if let Some(pid) = self.child.process_id().filter(|pid| *pid > 0) {
                let _ = nix::sys::signal::killpg(
                    nix::unistd::Pid::from_raw(pid as i32),
                    nix::sys::signal::Signal::SIGKILL,
                );
            }
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
    /// Host-selected shell, run as the account running VniLink, without privilege elevation.
    pub struct NativeTerminalFactory {
        program: std::path::PathBuf,
        directory: std::path::PathBuf,
    }
    impl NativeTerminalFactory {
        /// Locally selected login shell, with predictable fallbacks. Nothing in
        /// a remote request can override this executable or home directory.
        pub fn for_current_user() -> Result<Self, Error> {
            let directory = std::env::var_os("HOME")
                .map(std::path::PathBuf::from)
                .filter(|p| p.is_absolute() && p.is_dir())
                .ok_or(Error::InvalidSetting(
                    "the host home directory is unavailable",
                ))?;
            let selected = std::env::var_os("SHELL").map(std::path::PathBuf::from);
            let program = selected
                .into_iter()
                .chain(
                    ["/bin/zsh", "/bin/bash", "/bin/sh"]
                        .into_iter()
                        .map(std::path::PathBuf::from),
                )
                .find(|p| p.is_absolute() && p.is_file())
                .ok_or(Error::Unsupported("host shell"))?;
            Self::new(program, directory)
        }
        pub fn new(
            program: std::path::PathBuf,
            directory: std::path::PathBuf,
        ) -> Result<Self, Error> {
            if !program.is_absolute()
                || !program.is_file()
                || !directory.is_absolute()
                || !directory.is_dir()
            {
                return Err(Error::InvalidSetting(
                    "choose an absolute shell path and an existing working directory",
                ));
            }
            Ok(Self { program, directory })
        }
    }
    fn size(value: TerminalSize) -> PtySize {
        PtySize {
            rows: value.rows,
            cols: value.columns,
            pixel_width: 0,
            pixel_height: 0,
        }
    }
    impl HostTerminalFactory for NativeTerminalFactory {
        fn open(
            &self,
            dimensions: TerminalSize,
            startup: crate::BackendStartup,
        ) -> Result<TerminalBackend, Error> {
            startup.check()?;
            dimensions.validate()?;
            let pair = native_pty_system()
                .openpty(size(dimensions))
                .map_err(|_| Error::Unsupported("native terminal allocation"))?;
            let fd = pair
                .master
                .as_raw_fd()
                .ok_or(Error::Unsupported("nonblocking terminal I/O"))?;
            // All cloned master descriptors share O_NONBLOCK. No slow reader or
            // stopped foreground process can prevent this worker observing close.
            let flags = nix::fcntl::fcntl(fd, nix::fcntl::FcntlArg::F_GETFL)
                .map_err(|_| Error::Unsupported("terminal flags"))?;
            nix::fcntl::fcntl(
                fd,
                nix::fcntl::FcntlArg::F_SETFL(
                    nix::fcntl::OFlag::from_bits_truncate(flags) | nix::fcntl::OFlag::O_NONBLOCK,
                ),
            )
            .map_err(|_| Error::Unsupported("nonblocking terminal I/O"))?;
            let mut reader = pair
                .master
                .try_clone_reader()
                .map_err(|_| Error::Unsupported("terminal reader"))?;
            let mut writer = pair
                .master
                .take_writer()
                .map_err(|_| Error::Unsupported("terminal writer"))?;
            startup.check()?;
            let mut command = CommandBuilder::new(&self.program);
            command.cwd(&self.directory);
            command.env("TERM", "xterm-256color");
            let child = pair
                .slave
                .spawn_command(command)
                .map_err(|_| Error::Unsupported("terminal shell startup"))?;
            let process_id = child.process_id().unwrap_or(0);
            drop(pair.slave);
            let mut process = NativeProcess {
                master: pair.master,
                child,
            };
            startup.check()?;
            let (tx, rx) = std::sync::mpsc::sync_channel(TERMINAL_QUEUE);
            let (output, events) = tokio::sync::mpsc::channel(TERMINAL_QUEUE);
            let stop = Arc::new(AtomicBool::new(false));
            let ended = stop.clone();
            let worker = move || {
                let mut pending = Vec::new();
                let mut offset = 0;
                let mut event = None;
                let mut exit = None;
                let mut eof = false;
                while !ended.load(Ordering::Acquire) {
                    if let Some(value) = event.take() {
                        match output.try_send(value) {
                            Ok(()) => {}
                            Err(tokio::sync::mpsc::error::TrySendError::Full(value)) => {
                                event = Some(value)
                            }
                            Err(_) => break,
                        }
                    }
                    if pending.is_empty() {
                        match rx.try_recv() {
                            Ok(TerminalCommand::Input(bytes)) => {
                                if bytes.is_empty() || bytes.len() > TERMINAL_CHUNK {
                                    event = Some(TerminalEvent::Failed(
                                        "terminal input chunk is out of range".into(),
                                    ));
                                    break;
                                }
                                pending = bytes;
                                offset = 0;
                            }
                            Ok(TerminalCommand::Resize(value)) => {
                                if value.validate().is_err()
                                    || process.master.resize(size(value)).is_err()
                                {
                                    event = Some(TerminalEvent::Failed(
                                        "terminal resize failed".into(),
                                    ));
                                    break;
                                }
                            }
                            Ok(TerminalCommand::Redraw(value)) => {
                                let temporary = TerminalSize {
                                    rows: if value.rows == 1 { 2 } else { value.rows - 1 },
                                    columns: value.columns,
                                };
                                if value.validate().is_err()
                                    || process.master.resize(size(temporary)).is_err()
                                    || process.master.resize(size(value)).is_err()
                                {
                                    event = Some(TerminalEvent::Failed(
                                        "terminal redraw failed".into(),
                                    ));
                                    break;
                                }
                            }
                            Err(std::sync::mpsc::TryRecvError::Disconnected) => break,
                            Err(_) => {}
                        }
                    }
                    if !pending.is_empty() {
                        match writer.write(&pending[offset..]) {
                            Ok(0) => break,
                            Ok(n) => {
                                offset += n;
                                if offset == pending.len() {
                                    pending.clear();
                                }
                            }
                            Err(e)
                                if matches!(
                                    e.kind(),
                                    std::io::ErrorKind::WouldBlock
                                        | std::io::ErrorKind::Interrupted
                                ) => {}
                            Err(_) => break,
                        }
                    }
                    if event.is_none() && !eof {
                        let mut bytes = vec![0; TERMINAL_CHUNK];
                        match reader.read(&mut bytes) {
                            Ok(0) => eof = true,
                            Ok(n) => {
                                bytes.truncate(n);
                                event = Some(TerminalEvent::Output(bytes));
                            }
                            Err(e)
                                if matches!(
                                    e.kind(),
                                    std::io::ErrorKind::WouldBlock
                                        | std::io::ErrorKind::Interrupted
                                ) => {}
                            Err(e) if e.raw_os_error() == Some(nix::libc::EIO) => eof = true,
                            Err(_) => {
                                event = Some(TerminalEvent::Failed("terminal read failed".into()));
                                break;
                            }
                        }
                    }
                    if exit.is_none() {
                        match process.child.try_wait() {
                            Ok(Some(status)) => exit = Some(status.exit_code()),
                            Ok(None) => {}
                            Err(_) => break,
                        }
                    }
                    if eof && event.is_none() {
                        if let Some(code) = exit {
                            event = Some(TerminalEvent::Exited(code));
                            break;
                        }
                    }
                    std::thread::sleep(Duration::from_millis(5));
                }
                if let Some(value) = event {
                    // Preserve final output/status under backpressure, while close
                    // still cancels immediately instead of blocking on the consumer.
                    let mut value = value;
                    while !ended.load(Ordering::Acquire) {
                        match output.try_send(value) {
                            Ok(()) => break,
                            Err(tokio::sync::mpsc::error::TrySendError::Full(v)) => {
                                value = v;
                                std::thread::sleep(Duration::from_millis(5));
                            }
                            Err(_) => break,
                        }
                    }
                }
                drop(process);
            };
            std::thread::Builder::new()
                .name("vnilink-terminal".into())
                .spawn(worker)
                .map_err(|_| Error::Unsupported("terminal worker"))?;
            Ok(TerminalBackend {
                process_id,
                shell: self
                    .program
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned(),
                commands: tx,
                output: events,
                stop,
            })
        }
    }
}
#[cfg(unix)]
pub use native::NativeTerminalFactory;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TerminalOptions {
    pub size: TerminalSize,
    pub persistent: bool,
    pub resume_id: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct TerminalSessionInfo {
    pub id: String,
    pub process_id: u32,
    pub shell: String,
    pub size: TerminalSize,
    pub persistent: bool,
    pub attached: bool,
}
pub const TERMINAL_HISTORY_BYTES: usize = 1024 * 1024;
pub const TERMINAL_DETACHED_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(300);
pub const MAX_TERMINAL_SESSIONS: usize = 16;

/// Host-local limits. Changes are admitted only while no shell is alive or
/// starting; an acknowledged policy therefore describes every hosted shell.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct TerminalPolicy {
    pub allow_persistence: bool,
    pub max_sessions: u16,
    pub max_sessions_per_peer: u16,
    pub history_bytes: u32,
    pub replay_bytes: u32,
    pub detached_timeout_secs: u32,
}
impl Default for TerminalPolicy {
    fn default() -> Self {
        Self {
            allow_persistence: true,
            max_sessions: 16,
            max_sessions_per_peer: 16,
            history_bytes: TERMINAL_HISTORY_BYTES as u32,
            replay_bytes: 8 * 1024,
            detached_timeout_secs: TERMINAL_DETACHED_TIMEOUT.as_secs() as u32,
        }
    }
}
impl TerminalPolicy {
    pub fn validate(self) -> Result<Self, Error> {
        if self.max_sessions == 0
            || usize::from(self.max_sessions) > MAX_TERMINAL_SESSIONS
            || self.max_sessions_per_peer == 0
            || self.max_sessions_per_peer > self.max_sessions
            || !(16 * 1024..=4 * 1024 * 1024).contains(&self.history_bytes)
            || self.replay_bytes > self.history_bytes
            || !(1..=24 * 60 * 60).contains(&self.detached_timeout_secs)
        {
            return Err(Error::InvalidSetting("terminal policy is out of range"));
        }
        Ok(self)
    }
}
