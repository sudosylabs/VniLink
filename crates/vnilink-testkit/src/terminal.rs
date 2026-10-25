//! What the terminal suites share: the platform's real shell backend, and a
//! terminal opened on it as the application opens one.

use std::sync::Arc;
use std::time::Duration;

use vnilink::terminal::{HostTerminalFactory, TerminalEvent, TerminalSize};
use vnilink::{Error, RemoteTerminalService, ServiceKind};

use crate::{wait_for, Pair};

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
        mut done: impl FnMut(&[u8]) -> bool,
    ) -> (Vec<u8>, Option<u32>) {
        let mut output = Vec::new();
        let mut exit = None;
        wait_for(what, within, || {
            while let Some(event) = self.terminal.poll() {
                match event {
                    TerminalEvent::Output(bytes) => output.extend(bytes),
                    TerminalEvent::Exited(code) => exit = Some(code),
                    TerminalEvent::Failed(message) => panic!("the shell failed: {message}"),
                    TerminalEvent::OutputGap(lost) => panic!("{lost} bytes of output were lost"),
                }
            }
            (exit.is_some() || done(&output)).then_some(())
        });
        (output, exit)
    }

    pub fn input(&self, text: &str) -> Result<(), Error> {
        self.terminal.input(text.as_bytes())
    }
}

/// The line ending a shell's input takes here.
pub const ENTER: &str = if cfg!(windows) { "\r\n" } else { "\n" };
