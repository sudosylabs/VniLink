// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! What the terminal suites share: the platform's real shell backend, a
//! host offering it and terminals opened on it as the application opens
//! them, their output read back, backends that hold or fail their startup,
//! and [`check`], which types into a shell and states what it answered and
//! the story the host told.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use expect_test::Expect;
use vnilink::terminal::{
    HostTerminalFactory, ShellChoice, TerminalBackend, TerminalEvent, TerminalOptions, TerminalSize,
};
use vnilink::{
    BackendStartup, Error, RemoteTerminalService, ServiceKind, SessionInfo, SessionPhase,
};

use crate::{assert_story, lock, wait_for, Engine, Pair, TestResult, Traced, DEFAULT_WAIT};

pub const SIZE: TerminalSize = TerminalSize {
    rows: 24,
    columns: 80,
};

/// The shell this platform's backend runs: `sh` on Unix, `cmd` on Windows,
/// in `directory`.
pub fn shell(directory: &std::path::Path) -> Arc<dyn HostTerminalFactory> {
    open_shell(directory).expect("a shell")
}

/// As [`shell`], or why this computer has none to run there.
pub fn open_shell(directory: &std::path::Path) -> Result<Arc<dyn HostTerminalFactory>, Error> {
    #[cfg(unix)]
    {
        Ok(Arc::new(vnilink::terminal::NativeTerminalFactory::new(
            "/bin/sh".into(),
            directory.into(),
        )?))
    }
    #[cfg(windows)]
    {
        let program = std::env::var_os("ComSpec").ok_or(Error::InvalidSetting(
            "ComSpec names no command interpreter",
        ))?;
        Ok(Arc::new(vnilink_windows::WindowsTerminalFactory::new(
            program.into(),
            directory.into(),
        )?))
    }
}

/// The file a shell with no quick generator of output (cmd) types to show
/// how fast its output arrives, in its working folder ([`output_text`]).
pub const OUTPUT_FILE: &str = "output.txt";

/// What ends that output. The command line that asks for it never holds
/// it, so the shell's echo of that line is not taken for the end.
pub const OUTPUT_END: &str = "VNI_42";

/// About `bytes` of full lines that fit [`SIZE`], [`OUTPUT_END`] last, so
/// the console neither wraps nor reflows them.
pub fn output_text(bytes: usize) -> String {
    let line = format!("{}\r\n", "a".repeat(SIZE.columns as usize - 2));
    line.repeat(bytes / line.len()) + OUTPUT_END + "\r\n"
}

/// Starts, from `terminal`'s shell, processes a terminal's own ending
/// leaves running: one in the background, one under `nohup` and one in a
/// process group of its own; their ids. None on Windows, whose `cmd` does
/// not say what it started.
pub fn start_lingering(terminal: &RemoteTerminalService) -> Vec<u32> {
    if cfg!(windows) {
        return Vec::new();
    }
    const MARK: &str = "vnilink-lingering=";
    terminal
        .input(
            // An interactive bash expands `!` from its history, line by line.
            format!(
                "set +H 2>/dev/null\nsleep 1000 & echo {MARK}$!; nohup sleep 1000 >/dev/null 2>&1 & echo {MARK}$!; set -m; sleep 1000 & echo {MARK}$!; set +m\n"
            )
            .as_bytes(),
        )
        .expect("the shell takes input");
    let ids = |output: &[u8]| -> Vec<u32> {
        String::from_utf8_lossy(output)
            .split(MARK)
            .skip(1)
            .filter_map(|after| {
                let digits: String = after.chars().take_while(char::is_ascii_digit).collect();
                digits.parse().ok()
            })
            .collect()
    };
    let mut output = Vec::new();
    read_until(
        terminal,
        &mut output,
        "the shell to say what it started",
        DEFAULT_WAIT,
        |output| ids(output).len() == 3,
    );
    ids(&output)
}

/// A host offering a shell backend and a controller connected to it as the
/// application connects; each terminal opens from it.
pub struct Hosting {
    pub pair: Pair,
    pub connection: SessionInfo,
}

impl Hosting {
    /// The host's real shell, Terminal granted.
    pub fn shell() -> Result<Self, Error> {
        Self::offering(true, |host| open_shell(&host.folder()))
    }

    /// The backend `factory` makes for the host, Terminal granted when
    /// `granted`.
    pub fn offering(
        granted: bool,
        factory: impl FnOnce(&Engine) -> Result<Arc<dyn HostTerminalFactory>, Error>,
    ) -> Result<Self, Error> {
        let kinds: &[ServiceKind] = if granted {
            &[ServiceKind::Terminal]
        } else {
            &[]
        };
        let pair = Pair::for_services(kinds);
        pair.host
            .core
            .set_terminal_factory(Some(factory(&pair.host)?))?;
        let connection = pair.controller.connect_services(&pair.host)?;
        Ok(Self { pair, connection })
    }

    /// A terminal of [`SIZE`] whose shell ends with it.
    pub fn open(&self) -> Result<RemoteTerminalService, Error> {
        self.open_with(options(false, None))
    }

    /// A terminal opened with `options`.
    pub fn open_with(&self, options: TerminalOptions) -> Result<RemoteTerminalService, Error> {
        self.pair
            .controller
            .open_terminal(&self.connection, options)
    }

    /// The phase of the controller's connection.
    pub fn phase(&self) -> Result<SessionPhase, Error> {
        Ok(self
            .pair
            .controller
            .core
            .controller_session(self.connection.id)?
            .phase)
    }

    /// Waits until `terminal` ended on the controller and the host hosts no
    /// service.
    pub fn wait_ended(&self, terminal: &RemoteTerminalService) {
        wait_for("the terminal to end on both sides", DEFAULT_WAIT, || {
            (terminal.is_cancelled() && self.pair.host.hosted(None).is_empty()).then_some(())
        });
    }

    /// Waits until the host keeps no shells.
    pub fn wait_shells_ended(&self) {
        wait_shells_ended(&self.pair.host);
    }
}

/// Waits until `host` keeps no shells.
pub fn wait_shells_ended(host: &Engine) {
    wait_for("the host to end its shells", DEFAULT_WAIT, || {
        host.core
            .hosted_terminal_sessions()
            .ok()?
            .is_empty()
            .then_some(())
    });
}

/// Terminal options of [`SIZE`]: kept by the host past a disconnect when
/// `persistent`, resuming the kept shell `resume_id`.
pub fn options(persistent: bool, resume_id: Option<String>) -> TerminalOptions {
    TerminalOptions {
        size: SIZE,
        persistent,
        resume_id,
    }
}

/// What a scenario types into a terminal on the host's shell, and what the
/// shell's output then shows.
pub struct Typed<'a> {
    /// The size the terminal takes before anything is typed.
    pub size: Option<TerminalSize>,
    /// Lines typed in order, each ended as the shell's input ends one; the
    /// last ends the shell.
    pub lines: &'a [&'a str],
    /// Text the output holds before the shell exits. No line holds it, so
    /// only the shell's answer shows it, and no record holds it.
    pub shows: &'a [&'a str],
}

/// Opens a terminal on the host's shell, types `typed` into it and reads
/// its output until the shell exits; asserts the output shows each of
/// `typed.shows`, that no record holds any of it, and that the host told
/// the story `expect` of its terminal.
#[track_caller]
pub fn check(trace: &Traced, typed: Typed<'_>, expect: Expect) -> TestResult {
    let hosting = Hosting::shell()?;
    let terminal = hosting.open()?;
    if let Some(size) = typed.size {
        terminal.resize(size)?;
    }
    for line in typed.lines {
        terminal.input(format!("{line}{ENTER}").as_bytes())?;
    }
    let mut output = Vec::new();
    let exit = read_until(
        &terminal,
        &mut output,
        "the shell to exit",
        DEFAULT_WAIT,
        |_| false,
    );
    let text = String::from_utf8_lossy(&output);
    assert!(exit.is_some(), "the typed lines end the shell: {text}");
    for shown in typed.shows {
        assert!(
            text.contains(shown),
            "the shell's answer {shown:?} reaches the controller: {text}"
        );
    }
    hosting.pair.host.wait_released(None);
    assert_story(expect, || trace.story(&["terminal"]));
    trace.assert_unrecorded("terminal content", typed.shows);
    Ok(())
}

/// The platform's backend for the shell the local user chose.
pub fn for_shell(choice: &ShellChoice) -> Result<Arc<dyn HostTerminalFactory>, Error> {
    #[cfg(unix)]
    {
        Ok(Arc::new(
            vnilink::terminal::NativeTerminalFactory::for_shell(choice)?,
        ))
    }
    #[cfg(windows)]
    {
        Ok(Arc::new(
            vnilink_windows::WindowsTerminalFactory::for_shell(choice)?,
        ))
    }
}

/// How a held startup ended.
#[derive(Debug, PartialEq, Eq)]
pub enum Held {
    /// The test released it, and it went on to start its shell.
    Released,
    /// The startup was cancelled while it held.
    Cancelled,
    /// It ended for another reason, as its startup's deadline.
    Ended(String),
}

/// A backend whose first startup holds, its shell not yet started, until
/// the test releases it or the startup is cancelled; later startups start
/// the shell at once. It counts its startups, so a test sees whether a
/// refusal came before one.
pub struct HeldStartup {
    shell: Arc<dyn HostTerminalFactory>,
    startups: AtomicUsize,
    holding: Signal<()>,
    release: Signal<()>,
    finished: Signal<Held>,
}

/// What one side tells the other, in order.
struct Signal<T> {
    send: Mutex<Sender<T>>,
    receive: Mutex<Receiver<T>>,
}

impl<T> Signal<T> {
    fn new() -> Self {
        let (send, receive) = channel();
        Self {
            send: Mutex::new(send),
            receive: Mutex::new(receive),
        }
    }

    fn send(&self, value: T) {
        // The receiver lives as long as the signal.
        let _ = lock(&self.send).send(value);
    }

    #[track_caller]
    fn wait(&self, what: &str) -> T {
        match lock(&self.receive).recv_timeout(DEFAULT_WAIT) {
            Ok(value) => value,
            Err(_) => panic!("waited {DEFAULT_WAIT:?} for {what}, which did not happen"),
        }
    }
}

impl HeldStartup {
    /// Holds the first startup of `shell`.
    pub fn new(shell: Arc<dyn HostTerminalFactory>) -> Arc<Self> {
        Arc::new(Self {
            shell,
            startups: AtomicUsize::new(0),
            holding: Signal::new(),
            release: Signal::new(),
            finished: Signal::new(),
        })
    }

    /// Waits until the first startup holds.
    #[track_caller]
    pub fn wait_holding(&self) {
        self.holding.wait("the first shell's startup to hold");
    }

    /// Lets the held startup go on to start its shell.
    pub fn release(&self) {
        self.release.send(());
    }

    /// Waits until the held startup stops holding, and says how.
    #[track_caller]
    pub fn wait_finished(&self) -> Held {
        self.finished.wait("the held startup to end")
    }

    /// How many startups began.
    pub fn startups(&self) -> usize {
        self.startups.load(Ordering::SeqCst)
    }
}

impl HostTerminalFactory for HeldStartup {
    fn open(&self, size: TerminalSize, startup: BackendStartup) -> Result<TerminalBackend, Error> {
        if self.startups.fetch_add(1, Ordering::SeqCst) == 0 {
            self.holding.send(());
            let held = startup.receive(&lock(&self.release.receive));
            self.finished.send(match &held {
                Ok(()) => Held::Released,
                Err(Error::NoSession) => Held::Cancelled,
                Err(error) => Held::Ended(error.to_string()),
            });
            held?;
        }
        self.shell.open(size, startup)
    }
}

/// A backend that cannot start, and says so.
pub struct Failing;

impl HostTerminalFactory for Failing {
    fn open(&self, _: TerminalSize, _: BackendStartup) -> Result<TerminalBackend, Error> {
        Err(Error::Unsupported("test terminal startup"))
    }
}

/// A host's shell and a controller with a terminal open on it.
pub struct TerminalSession {
    pub pair: Pair,
    pub terminal: RemoteTerminalService,
}

impl TerminalSession {
    pub fn open() -> Self {
        let hosting = Hosting::shell().expect("a host offering its shell");
        let terminal = hosting.open().expect("a terminal");
        Self {
            pair: hosting.pair,
            terminal,
        }
    }

    /// Output until `done` says it has what it waited for, or the shell
    /// ends; the exit code, if it exited.
    pub fn read_until(
        &self,
        what: &str,
        within: Duration,
        done: impl FnMut(&[u8]) -> bool,
    ) -> (Vec<u8>, Option<u32>) {
        let mut output = Vec::new();
        let exit = read_until(&self.terminal, &mut output, what, within, done);
        (output, exit)
    }

    pub fn input(&self, text: &str) -> Result<(), Error> {
        self.terminal.input(text.as_bytes())
    }
}

/// The line ending a shell's input takes here.
pub const ENTER: &str = if cfg!(windows) { "\r\n" } else { "\n" };

/// `terminal`'s output, added to `output`, until `done` says `output` has
/// what it waited for or the shell ends, within `within`; the exit code, if
/// it exited. A failed backend or lost output fails the test.
pub fn read_until(
    terminal: &RemoteTerminalService,
    output: &mut Vec<u8>,
    what: &str,
    within: Duration,
    mut done: impl FnMut(&[u8]) -> bool,
) -> Option<u32> {
    let mut exit = None;
    wait_for(what, within, || {
        while let Some(event) = terminal.poll() {
            match event {
                TerminalEvent::Output(bytes) => output.extend(bytes),
                TerminalEvent::Exited(code) => exit = Some(code),
                TerminalEvent::Failed(message) => panic!("the shell failed: {message}"),
                TerminalEvent::OutputGap(lost) => panic!("{lost} bytes of output were lost"),
            }
        }
        (exit.is_some() || done(output)).then_some(())
    });
    exit
}

/// Whether `output`, read as text, holds `text`.
pub fn contains(output: &[u8], text: &str) -> bool {
    String::from_utf8_lossy(output).contains(text)
}

/// Opening services: the Terminal service, beside [`Engine::open_desktop`].
impl Engine {
    /// A Terminal service attached on `connection` and opened with
    /// `options`, as the application opens one.
    pub fn open_terminal(
        &self,
        connection: &SessionInfo,
        options: TerminalOptions,
    ) -> Result<RemoteTerminalService, Error> {
        let owner = self.core.attach_controller_service(
            connection.id,
            connection.generation,
            ServiceKind::Terminal,
        )?;
        self.core
            .open_terminal_service_with_options(owner.id, owner.generation, options)
    }
}
