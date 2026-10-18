//! Opt-in terminal backends. Remote peers cannot choose a program or directory.
use crate::Error;

pub const TERMINAL_CHUNK: usize = 16 * 1024;
pub const TERMINAL_QUEUE: usize = 32;
/// How long a busy shell's output may gather into one chunk; typing echoes
/// arrive this much later at most.
#[cfg(unix)]
const OUTPUT_GATHER: std::time::Duration = std::time::Duration::from_millis(1);

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
    /// The [`ShellChoice::id`] this factory runs, when it came from one.
    fn shell_id(&self) -> Option<&str> {
        None
    }
}

/// A shell this computer can offer to paired computers. Its own user picks one
/// on the host; a peer never names a program, its arguments or a directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShellChoice {
    /// Stable on this computer: the shell's path on Unix, a name on Windows.
    pub id: String,
    pub label: String,
    pub program: std::path::PathBuf,
    pub args: Vec<std::ffi::OsString>,
}

/// Bounds on host-defined shell arguments.
pub const MAX_SHELL_ARGS: usize = 8;
pub const MAX_SHELL_ARG_LEN: usize = 256;

/// Rejects arguments no detected shell needs: too many, too long, or with NUL.
pub fn validate_shell_args(args: &[std::ffi::OsString]) -> Result<(), Error> {
    if args.len() > MAX_SHELL_ARGS
        || args
            .iter()
            .any(|arg| arg.len() > MAX_SHELL_ARG_LEN || arg.to_string_lossy().contains('\0'))
    {
        return Err(Error::InvalidSetting("shell arguments are out of range"));
    }
    Ok(())
}

/// Labels shells by file name, numbering repeats as VS Code does (`bash (2)`).
pub fn label_shells(programs: Vec<std::path::PathBuf>) -> Vec<(std::path::PathBuf, String)> {
    let mut seen = std::collections::HashMap::<String, usize>::new();
    programs
        .into_iter()
        .map(|program| {
            let name = program
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| program.to_string_lossy().into_owned());
            let count = seen.entry(name.clone()).or_default();
            *count += 1;
            let label = if *count == 1 {
                name
            } else {
                format!("{name} ({count})")
            };
            (program, label)
        })
        .collect()
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
    fn account() -> Result<nix::unistd::User, Error> {
        nix::unistd::User::from_uid(nix::unistd::Uid::effective())
            .map_err(|_| Error::Unsupported("host account lookup"))?
            .ok_or(Error::Unsupported("host account"))
    }
    fn is_executable(path: &std::path::Path) -> bool {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(path)
            .is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
    }
    /// Every shell runs as a login shell, as the account's own login would.
    fn login_args() -> Vec<std::ffi::OsString> {
        vec!["-l".into()]
    }
    /// The shells `/etc/shells` lists that exist here, each once by its real
    /// path. `canonical` resolves a listed path, or rejects a missing one.
    pub fn parse_shells(
        text: &str,
        canonical: impl Fn(&std::path::Path) -> Option<std::path::PathBuf>,
    ) -> Vec<std::path::PathBuf> {
        let mut seen = std::collections::HashSet::new();
        text.lines()
            .map(|line| line.split('#').next().unwrap_or_default().trim())
            .filter(|line| line.starts_with('/'))
            .map(std::path::PathBuf::from)
            .filter(|path| canonical(path).is_some_and(|real| seen.insert(real)))
            .collect()
    }
    /// The account's login shell, which is what "automatic" offers.
    pub fn automatic_shell() -> Result<ShellChoice, Error> {
        let program = account()?.shell;
        let label = label_shells(vec![program.clone()]).remove(0).1;
        Ok(ShellChoice {
            id: program.to_string_lossy().into_owned(),
            label,
            program,
            args: login_args(),
        })
    }
    /// Installed shells from `/etc/shells`, with the login shell first if it is
    /// not listed there.
    pub fn available_shells() -> Vec<ShellChoice> {
        let text = std::fs::read_to_string("/etc/shells").unwrap_or_default();
        let mut programs = parse_shells(&text, |path| {
            is_executable(path)
                .then(|| std::fs::canonicalize(path).ok())
                .flatten()
        });
        if let Ok(login) = account().map(|account| account.shell) {
            let login_real = std::fs::canonicalize(&login).ok();
            let listed = programs
                .iter()
                .any(|path| path == &login || std::fs::canonicalize(path).ok() == login_real);
            if !listed && is_executable(&login) {
                programs.insert(0, login);
            }
        }
        label_shells(programs)
            .into_iter()
            .map(|(program, label)| ShellChoice {
                id: program.to_string_lossy().into_owned(),
                label,
                program,
                args: login_args(),
            })
            .collect()
    }
    /// Host-selected shell, run as the account running VniLink, without privilege elevation.
    pub struct NativeTerminalFactory {
        program: std::path::PathBuf,
        directory: std::path::PathBuf,
        args: Vec<std::ffi::OsString>,
        id: Option<String>,
    }
    impl NativeTerminalFactory {
        /// Account login shell and home directory. Nothing in
        /// a remote request can override this executable or home directory.
        pub fn for_current_user() -> Result<Self, Error> {
            Self::for_shell(&automatic_shell()?)
        }
        /// A shell the local user chose, in the account's home directory.
        pub fn for_shell(shell: &ShellChoice) -> Result<Self, Error> {
            validate_shell_args(&shell.args)?;
            let mut factory = Self::new(shell.program.clone(), account()?.dir)?;
            factory.args = shell.args.clone();
            factory.id = Some(shell.id.clone());
            Ok(factory)
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
            Ok(Self {
                program,
                directory,
                args: Vec::new(),
                id: None,
            })
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
        fn shell_id(&self) -> Option<&str> {
            self.id.as_deref()
        }
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
            for arg in &self.args {
                command.arg(arg);
            }
            command.cwd(&self.directory);
            command.env("TERM", "xterm-256color");
            command.env("SHELL", &self.program);
            command.env("HOME", &self.directory);
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
                    // The worker sleeps only when a pass did nothing, so a busy
                    // shell runs at the consumer's pace and an idle one costs little.
                    let mut progressed = false;
                    if let Some(value) = event.take() {
                        match output.try_send(value) {
                            Ok(()) => progressed = true,
                            Err(tokio::sync::mpsc::error::TrySendError::Full(value)) => {
                                event = Some(value)
                            }
                            Err(_) => break,
                        }
                    }
                    if pending.is_empty() {
                        match rx.try_recv() {
                            Ok(TerminalCommand::Input(bytes)) => {
                                progressed = true;
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
                                progressed = true;
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
                                progressed = true;
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
                                progressed = true;
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
                        // Drain what the PTY holds into one chunk: a read returns
                        // only what the shell wrote since the last one, often a
                        // line. Output that is still arriving gets one short wait
                        // to fill the chunk, since each chunk crosses the network
                        // and the viewer's acknowledgement on its own.
                        let mut bytes = vec![0; TERMINAL_CHUNK];
                        let mut filled = 0;
                        let mut failed = false;
                        let mut gathered = false;
                        while filled < TERMINAL_CHUNK {
                            match reader.read(&mut bytes[filled..]) {
                                Ok(0) => {
                                    eof = true;
                                    break;
                                }
                                Ok(n) => filled += n,
                                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                                    if filled == 0 || gathered {
                                        break;
                                    }
                                    gathered = true;
                                    std::thread::sleep(OUTPUT_GATHER);
                                }
                                Err(e) if e.raw_os_error() == Some(nix::libc::EIO) => {
                                    eof = true;
                                    break;
                                }
                                Err(_) => {
                                    failed = true;
                                    break;
                                }
                            }
                        }
                        if failed {
                            event = Some(TerminalEvent::Failed("terminal read failed".into()));
                            break;
                        }
                        if filled > 0 {
                            bytes.truncate(filled);
                            event = Some(TerminalEvent::Output(bytes));
                            progressed = true;
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
                    if !progressed {
                        std::thread::sleep(Duration::from_millis(5));
                    }
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
    #[cfg(test)]
    mod account_tests {
        use super::*;
        #[test]
        fn default_terminal_uses_account_shell_and_home() {
            let account = nix::unistd::User::from_uid(nix::unistd::Uid::effective())
                .unwrap()
                .unwrap();
            let factory = NativeTerminalFactory::for_current_user().unwrap();
            assert_eq!(factory.program, account.shell);
            assert_eq!(factory.directory, account.dir);
            assert_eq!(factory.args, vec![std::ffi::OsString::from("-l")]);
            assert_eq!(factory.shell_id(), Some(account.shell.to_str().unwrap()));
            let plain = NativeTerminalFactory::new(factory.program, factory.directory).unwrap();
            assert!(plain.args.is_empty() && plain.shell_id().is_none());
        }

        #[test]
        fn etc_shells_lists_existing_shells_once_with_numbered_labels() {
            let text = "# List of acceptable shells\n/bin/bash\n\n/bin/zsh # default\n/usr/bin/bash\nrelative/sh\n/missing/fish\n/bin/sh\n/usr/local/bin/bash\n";
            // /usr/bin/bash is the same file as /bin/bash; /missing/fish does not exist.
            let shells = parse_shells(text, |path| match path.to_str().unwrap() {
                "/bin/bash" | "/usr/bin/bash" => Some("/usr/bin/bash".into()),
                "/missing/fish" => None,
                other => Some(other.into()),
            });
            assert_eq!(
                shells,
                ["/bin/bash", "/bin/zsh", "/bin/sh", "/usr/local/bin/bash"]
                    .map(std::path::PathBuf::from)
            );
            let labels: Vec<_> = label_shells(shells)
                .into_iter()
                .map(|(_, label)| label)
                .collect();
            assert_eq!(labels, ["bash", "zsh", "sh", "bash (2)"]);
        }

        #[test]
        fn available_shells_include_the_login_shell_as_automatic() {
            let automatic = automatic_shell().unwrap();
            let shells = available_shells();
            assert!(
                shells.iter().any(|shell| shell.id == automatic.id),
                "{shells:?}"
            );
            assert!(shells
                .iter()
                .all(|shell| shell.args == [std::ffi::OsString::from("-l")]));
            let ids: std::collections::HashSet<_> = shells.iter().map(|shell| &shell.id).collect();
            assert_eq!(ids.len(), shells.len(), "a shell is listed twice");
        }
    }
}
#[cfg(unix)]
pub use native::{automatic_shell, available_shells, parse_shells, NativeTerminalFactory};

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
