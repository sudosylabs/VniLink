//! The Terminal service alone: the host's real shell, a terminal opened
//! from the controller as the application opens one.

use std::time::Duration;

use vnilink::{Error, ServiceKind};
use vnilink_testkit::terminal::{TerminalSession, ENTER};
use vnilink_testkit::{traced, wait_for, Pair, DEFAULT_WAIT};

const SHELL: Duration = Duration::from_secs(10);

fn contains(output: &[u8], text: &str) -> bool {
    String::from_utf8_lossy(output).contains(text)
}

/// What is typed reaches the shell, its output comes back, and its exit
/// code ends the terminal; the host records the shell opening and its exit.
#[test]
fn a_shell_runs_what_is_typed_and_ends_with_its_exit_code() {
    let trace = traced();
    let session = TerminalSession::open();
    session.input(&format!("echo VNI_READY{ENTER}")).unwrap();
    let (output, exit) = session.read_until("the shell's echo", SHELL, |output| {
        contains(output, "VNI_READY\r\n") || contains(output, "VNI_READY\n")
    });
    assert_eq!(exit, None, "{}", String::from_utf8_lossy(&output));
    session.input(&format!("exit 7{ENTER}")).unwrap();
    let (_, exit) = session.read_until("the shell to exit", SHELL, |_| false);
    assert_eq!(exit, Some(7));
    wait_for("the host to release the terminal", DEFAULT_WAIT, || {
        session
            .pair
            .host
            .core
            .hosted_services()
            .ok()?
            .is_empty()
            .then_some(())
    });
    let opened = trace.named("terminal opened");
    assert_eq!(opened.len(), 1, "{opened:?}");
    assert_eq!(opened[0]["rows"], 24);
    let exited = trace.named("terminal shell exited");
    assert_eq!(exited.len(), 1, "{exited:?}");
    assert_eq!(exited[0]["code"], 7);
}

/// A resize reaches the shell's terminal.
#[cfg(unix)]
#[test]
fn a_resize_reaches_the_shell() {
    let _trace = traced();
    let session = TerminalSession::open();
    session
        .terminal
        .resize(vnilink::terminal::TerminalSize {
            rows: 39,
            columns: 117,
        })
        .unwrap();
    // The size is applied in order with input, before the next command.
    session.input("stty size\n").unwrap();
    let (output, _) =
        session.read_until("the new size", SHELL, |output| contains(output, "39 117"));
    assert!(
        contains(&output, "39 117"),
        "{}",
        String::from_utf8_lossy(&output)
    );
}

/// Text outside ASCII passes both ways intact.
#[cfg(unix)]
#[test]
fn text_outside_ascii_passes_intact() {
    let _trace = traced();
    let session = TerminalSession::open();
    session
        .input("printf '\\316\\261\\342\\202\\254\\n'\n")
        .unwrap();
    let (output, _) = session.read_until("the characters", SHELL, |output| contains(output, "α€"));
    assert!(
        contains(&output, "α€"),
        "{}",
        String::from_utf8_lossy(&output)
    );
}

/// Without the host's Terminal grant no terminal opens, the refusal is
/// recorded, and the connection carries on.
#[test]
fn without_the_grant_no_terminal_opens_and_the_connection_lives() {
    let trace = traced();
    let pair = Pair::for_services(&[]);
    pair.host
        .core
        .set_terminal_factory(Some(vnilink_testkit::terminal::shell(&pair.host.root())))
        .unwrap();
    let connection = pair.connect_services();
    let owner = pair.attach(&connection, ServiceKind::Terminal);
    let opened = pair.controller.core.open_terminal_service(
        owner.id,
        owner.generation,
        vnilink_testkit::terminal::SIZE,
    );
    assert!(
        matches!(opened, Err(Error::Unauthorized)),
        "{:?}",
        opened.err()
    );
    assert_eq!(
        pair.controller
            .core
            .controller_session(connection.id)
            .unwrap()
            .phase,
        vnilink::SessionPhase::Active
    );
    let refused = wait_for("the refusal to be recorded", DEFAULT_WAIT, || {
        let refused = trace.named("service refused");
        (!refused.is_empty()).then_some(refused)
    });
    assert_eq!(refused[0]["status"], "not permitted", "{refused:?}");
}

/// Closing the terminal ends the shell on the host.
#[test]
fn closing_the_terminal_ends_the_shell() {
    let _trace = traced();
    let session = TerminalSession::open();
    session.input(&format!("echo VNI_READY{ENTER}")).unwrap();
    session.read_until("the shell", SHELL, |output| contains(output, "VNI_READY"));
    session.terminal.close();
    wait_for("the host to end the shell", DEFAULT_WAIT, || {
        let host = &session.pair.host.core;
        (host.hosted_services().ok()?.is_empty()
            && host.hosted_terminal_sessions().ok()?.is_empty())
        .then_some(())
    });
}
