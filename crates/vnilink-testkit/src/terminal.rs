//! What the terminal suites share: the platform's real shell backend, a
//! terminal opened on it as the application opens one, and its output read
//! back.

use std::sync::Arc;
use std::time::Duration;

use vnilink::terminal::{HostTerminalFactory, TerminalEvent, TerminalOptions, TerminalSize};
use vnilink::{Error, RemoteTerminalService, ServiceKind, SessionInfo};

use crate::{wait_for, Engine, Pair};

pub const SIZE: TerminalSize = TerminalSize {
    rows: 24,
    columns: 80,
};

/// The shell this platform's backend runs: `sh` on Unix, `cmd` on Windows,
/// in `directory`.
pub fn shell(directory: &std::path::Path) -> Arc<dyn HostTerminalFactory> {
    #[cfg(unix)]
    {
        Arc::new(
            vnilink::terminal::NativeTerminalFactory::new("/bin/sh".into(), directory.into())
                .expect("a shell"),
        )
    }
    #[cfg(windows)]
    {
        let program = std::env::var_os("ComSpec").expect("ComSpec names cmd");
        Arc::new(
            vnilink_windows::WindowsTerminalFactory::new(program.into(), directory.into())
                .expect("a shell"),
        )
    }
}

/// A host offering its shell and a controller with a terminal open on it.
pub struct TerminalSession {
    pub pair: Pair,
    pub terminal: RemoteTerminalService,
}

impl TerminalSession {
    pub fn open() -> Self {
        let pair = Pair::for_services(&[ServiceKind::Terminal]);
        pair.host
            .core
            .set_terminal_factory(Some(shell(&pair.host.root())))
            .expect("a terminal backend");
        let connection = pair.connect_services();
        let owner = pair.attach(&connection, ServiceKind::Terminal);
        let terminal = pair
            .controller
            .core
            .open_terminal_service(owner.id, owner.generation, SIZE)
            .expect("a terminal");
        Self { pair, terminal }
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
