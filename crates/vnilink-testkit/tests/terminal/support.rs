//! What the terminal scenarios share: terminals opened on a connection, their output read back, and the check
//! that nothing typed or printed reached the engines' records.

use std::time::Duration;

use vnilink::terminal::{TerminalEvent, TerminalOptions};
use vnilink::{Error, RemoteTerminalService, ServiceKind, SessionInfo, SessionPhase};
use vnilink_testkit::terminal::SIZE;
use vnilink_testkit::{wait_for, Engine, Traced, DEFAULT_WAIT};

/// How long a shell has to answer what was typed.
pub(super) const SHELL: Duration = Duration::from_secs(10);

/// A Terminal service attached on `connection` and opened with `options`.
pub(super) fn open_with(
    controller: &Engine,
    connection: &SessionInfo,
    options: TerminalOptions,
) -> Result<RemoteTerminalService, Error> {
    let owner = controller.core.attach_controller_service(
        connection.id,
        connection.generation,
        ServiceKind::Terminal,
    )?;
    controller
        .core
        .open_terminal_service_with_options(owner.id, owner.generation, options)
}

/// A terminal of [`SIZE`] on `connection`, its shell ending with it.
pub(super) fn open(
    controller: &Engine,
    connection: &SessionInfo,
) -> Result<RemoteTerminalService, Error> {
    open_with(controller, connection, options(false, None))
}

pub(super) fn options(persistent: bool, resume_id: Option<String>) -> TerminalOptions {
    TerminalOptions {
        size: SIZE,
        persistent,
        resume_id,
    }
}

/// `terminal`'s output until it holds `text` or the shell ends; the exit
/// code, if it exited. A failed backend or lost output fails the test.
pub(super) fn read_until(
    terminal: &RemoteTerminalService,
    output: &mut Vec<u8>,
    what: &str,
    mut done: impl FnMut(&str) -> bool,
) -> Option<u32> {
    let mut exit = None;
    wait_for(what, SHELL, || {
        while let Some(event) = terminal.poll() {
            match event {
                TerminalEvent::Output(bytes) => output.extend(bytes),
                TerminalEvent::Exited(code) => exit = Some(code),
                TerminalEvent::Failed(message) => panic!("the shell failed: {message}"),
                TerminalEvent::OutputGap(lost) => panic!("{lost} bytes of output were lost"),
            }
        }
        (exit.is_some() || done(&String::from_utf8_lossy(output))).then_some(())
    });
    exit
}

/// `terminal`'s output until the shell exits, and its exit code.
pub(super) fn read_to_exit(terminal: &RemoteTerminalService) -> (String, u32) {
    let mut output = Vec::new();
    let exit = read_until(terminal, &mut output, "the shell to exit", |_| false);
    let text = String::from_utf8_lossy(&output).into_owned();
    (text, exit.expect("the shell exited"))
}

/// Waits until `host` keeps no shells.
pub(super) fn host_ended_shells(host: &Engine) {
    wait_for("the host to end its shells", DEFAULT_WAIT, || {
        host.core
            .hosted_terminal_sessions()
            .ok()?
            .is_empty()
            .then_some(())
    });
}

/// Terminal content is the user's: none of `typed`, which the scenario
/// typed or its shell printed, is in what the engines recorded.
pub(super) fn assert_content_unrecorded(trace: &Traced, typed: &[&str]) {
    let everything = trace.events().join("\n");
    for text in typed {
        assert!(
            !everything.contains(text),
            "terminal content is never recorded, yet {text:?} was"
        );
    }
}

pub(super) fn phase(controller: &Engine, connection: &SessionInfo) -> SessionPhase {
    controller
        .core
        .controller_session(connection.id)
        .expect("the connection")
        .phase
}

/// Whether `event` is the host's record of how a terminal in `services`
/// ended, for a story to mask its `ended`: a viewer's close or disconnect
/// reaches the host both as the end of its stream and as a close or the
/// connection's end, and the host records whichever came first (`network`
/// or `no session`), so the cause varies between runs.
pub(super) fn ended_by_viewer(services: &[u64]) -> impl Fn(&serde_json::Value) -> bool + '_ {
    |event| {
        event["engine"] == "host"
            && event["message"] == "terminal stream ended"
            && event["service"]
                .as_u64()
                .is_some_and(|service| services.contains(&service))
    }
}
