// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! One terminal's life beside its connection and its siblings: refused,
//! opened, ended by its shell, its close, a revoked grant or a disabled
//! backend, while the connection and other services carry on.

use expect_test::expect;
use std::sync::Arc;
use std::time::Duration;

use vnilink::terminal::{
    HostTerminalFactory, NativeTerminalFactory, ShellChoice, TerminalBackend, TerminalSize,
    TERMINAL_CHUNK,
};
use vnilink::{
    BackendStartup, Error, RemoteTerminalService, ServiceKind, ServicePhase, SessionPhase,
};
use vnilink_testkit::terminal::{read_until, shell, SIZE};
use vnilink_testkit::{holds, traced, wait_for, Pair, TestResult, DEFAULT_WAIT};

use super::support::{host_ended_shells, open, phase, read_to_exit, SHELL};

/// A terminal refused for want of the grant fails only itself; once granted,
/// a terminal opens beside a Files service, refuses a second open, carries
/// a resize and text outside ASCII, and ends with its shell's exit code
/// while the Files service and the connection carry on.
#[test]
fn a_terminal_ends_with_its_shell_and_leaves_its_connection_and_siblings() -> TestResult {
    let trace = traced();
    let pair = Pair::for_desktop();
    let (host, controller) = (&pair.host.core, &pair.controller.core);
    host.set_terminal_factory(Some(shell(&pair.host.folder())))?;
    let connection = pair.connect_services();
    assert!(
        controller
            .remote_service_support(connection.id, connection.generation)?
            .terminal,
        "a host with a shell backend offers Terminal"
    );
    let refused = pair.attach(&connection, ServiceKind::Terminal);
    assert!(
        matches!(
            controller.open_terminal_service(refused.id, refused.generation, SIZE),
            Err(Error::Unauthorized)
        ),
        "without the grant no terminal opens"
    );
    wait_for("the refused terminal to fail", DEFAULT_WAIT, || {
        matches!(
            controller.controller_service(refused.id).ok()?.phase,
            ServicePhase::Failed(_)
        )
        .then_some(())
    });
    host.set_service_sharing(ServiceKind::Terminal, true)?;
    let files = pair.attach(&connection, ServiceKind::Files);
    let files_lease = controller.service_lease(files.id, files.generation)?;
    let owner = pair.attach(&connection, ServiceKind::Terminal);
    let terminal = controller.open_terminal_service(owner.id, owner.generation, SIZE)?;
    assert!(
        matches!(
            controller.open_terminal_service(owner.id, owner.generation, SIZE),
            Err(Error::Busy)
        ),
        "one Terminal service opens one terminal"
    );
    assert!(
        !terminal.is_cancelled(),
        "an open terminal is not cancelled"
    );
    terminal.resize(TerminalSize {
        rows: 39,
        columns: 117,
    })?;
    terminal.input(b"printf 'VNI_READY\\n'; stty size; printf '\\316\\261\\n'; exit 7\n")?;
    let (text, exit) = read_to_exit(&terminal);
    assert_eq!(exit, 7, "the terminal ends with its shell's exit code");
    assert!(
        text.contains("39 117"),
        "the resize reached the shell: {text}"
    );
    assert!(
        text.contains('α'),
        "text outside ASCII arrives intact: {text}"
    );
    wait_for("the terminal to end on both sides", DEFAULT_WAIT, || {
        (terminal.is_cancelled() && pair.host.hosted(None).is_empty()).then_some(())
    });
    assert!(
        !files_lease.is_cancelled(),
        "a terminal's end leaves its sibling Files service open"
    );
    assert_eq!(
        phase(&pair.controller, &connection),
        SessionPhase::Active,
        "a terminal's end leaves its connection open"
    );
    assert!(
        matches!(terminal.input(b"x"), Err(Error::NoSession)),
        "an ended terminal takes no input"
    );
    trace.wait_for_records("service released", 1);
    vnilink_testkit::assert_story(
        expect![[r#"
        host:
          hosting: service refused connection=1 kind=Terminal status=not permitted
          hosting: service opened connection=1 kind=Some(Terminal) service=1
          terminal: terminal opened columns=80 persistent=false resumed=false rows=24 service=1
          terminal: terminal shell exited code=7 service=1
          terminal: terminal stream ended ended=shell exited explicit_close=true service=1
          hosting: service closed kind=Terminal phase=closed: OwnerClose service=1
          hosting: service released service=1
    "#]],
        || trace.story(&["terminal", "hosting"]),
    );
    trace.assert_unrecorded("terminal content", &["VNI_READY", "stty", "39 117", "α"]);
    Ok(())
}

/// Revoking the Terminal grant, or disabling the shell backend, ends the
/// running terminal on both sides but not the connection; a re-granted
/// terminal opens on the same connection, and a host without a backend no
/// longer offers Terminal.
#[test]
fn revoking_terminal_or_disabling_its_backend_ends_only_the_terminal() -> TestResult {
    let trace = traced();
    let pair = Pair::for_services(&[ServiceKind::Terminal]);
    let host = &pair.host.core;
    host.set_terminal_factory(Some(shell(&pair.host.folder())))?;
    let connection = pair.connect_services();
    for disable in [false, true] {
        let terminal = open(&pair.controller, &connection)?;
        if disable {
            host.set_terminal_factory(None)?;
        } else {
            host.set_service_sharing(ServiceKind::Terminal, false)?;
        }
        wait_for("the terminal to end on both sides", DEFAULT_WAIT, || {
            (terminal.is_cancelled() && pair.host.hosted(None).is_empty()).then_some(())
        });
        assert_eq!(
            phase(&pair.controller, &connection),
            SessionPhase::Active,
            "ending the terminal leaves its connection open"
        );
        host.set_service_sharing(ServiceKind::Terminal, true)?;
    }
    assert!(
        !pair
            .controller
            .core
            .remote_service_support(connection.id, connection.generation)?
            .terminal,
        "a host without a shell backend does not offer Terminal"
    );
    trace.wait_for_records("service released", 2);
    trace.wait_for_records("terminal pipe ended", 2);
    vnilink_testkit::assert_story(
        expect![[r#"
        controller:
          terminal: terminal pipe ended closed_here=false error=network service=1
          terminal: terminal pipe ended closed_here=false error=network service=2
        host:
          hosting: service opened connection=1 kind=Some(Terminal) service=1
          terminal: terminal opened columns=80 persistent=false resumed=false rows=24 service=1
          terminal: terminal stream ended ended=service closed explicit_close=false service=1
          hosting: service released service=1
          hosting: service opened connection=1 kind=Some(Terminal) service=2
          terminal: terminal opened columns=80 persistent=false resumed=false rows=24 service=2
          terminal: terminal stream ended ended=service closed explicit_close=false service=2
          hosting: service released service=2
    "#]],
        || trace.story(&["terminal", "hosting"]),
    );
    Ok(())
}

/// The local user's choice of shell is refused while a shell runs, unlike
/// the backend swap that disabling Terminal uses, and leaves that shell
/// running; once it ends the change applies, and the next shell is the new
/// one, its arguments passed exactly.
#[test]
fn changing_the_offered_shell_waits_for_running_shells() -> TestResult {
    let trace = traced();
    let pair = Pair::for_services(&[ServiceKind::Terminal]);
    let host = &pair.host.core;
    let first = ShellChoice {
        id: "/bin/sh".into(),
        label: "sh".into(),
        program: "/bin/sh".into(),
        args: vec![],
    };
    host.replace_terminal_factory(Arc::new(NativeTerminalFactory::for_shell(&first)?))?;
    assert_eq!(
        host.terminal_shell_id()?.as_deref(),
        Some("/bin/sh"),
        "the host runs the shell it was given"
    );
    let connection = pair.connect_services();
    let terminal = open(&pair.controller, &connection)?;
    wait_for("the host to run the shell", DEFAULT_WAIT, || {
        (!host.hosted_terminal_sessions().ok()?.is_empty()).then_some(())
    });
    // Arguments reach the shell exactly: with -c, the next two become $0
    // and $1.
    let second = ShellChoice {
        id: "custom".into(),
        label: "custom".into(),
        program: "/bin/sh".into(),
        args: ["-c", "echo VNI_ARGS:$0:$1; exit 3", "zero", "one"]
            .map(std::ffi::OsString::from)
            .to_vec(),
    };
    let replacement = Arc::new(NativeTerminalFactory::for_shell(&second)?);
    assert!(
        matches!(
            host.replace_terminal_factory(replacement.clone()),
            Err(Error::Busy)
        ),
        "the shell does not change while one runs"
    );
    assert!(
        !terminal.is_cancelled(),
        "a refused change ended the running shell"
    );
    terminal.input(b"exit\n")?;
    host_ended_shells(&pair.host);
    host.replace_terminal_factory(replacement)?;
    assert_eq!(
        host.terminal_shell_id()?.as_deref(),
        Some("custom"),
        "once no shell runs, the change applies"
    );
    let terminal = open(&pair.controller, &connection)?;
    let (text, exit) = read_to_exit(&terminal);
    assert_eq!(exit, 3, "the next terminal runs the new shell");
    assert!(
        text.contains("VNI_ARGS:zero:one"),
        "the new shell's arguments arrive exactly: {text}"
    );
    trace.wait_for_records("terminal stream ended", 2);
    vnilink_testkit::assert_story(
        expect![[r#"
        host:
          terminal: terminal opened columns=80 persistent=false resumed=false rows=24 service=1
          terminal: terminal shell exited code=0 service=1
          terminal: terminal stream ended ended=shell exited explicit_close=true service=1
          terminal: terminal opened columns=80 persistent=false resumed=false rows=24 service=2
          terminal: terminal shell exited code=3 service=2
          terminal: terminal stream ended ended=shell exited explicit_close=true service=2
    "#]],
        || trace.story(&["terminal"]),
    );
    trace.assert_unrecorded("terminal content", &["VNI_ARGS"]);
    Ok(())
}

/// Input and sizes a terminal cannot carry are refused, and `terminal`'s
/// shell then prints more than its viewer reads, which keeps the terminal
/// open.
fn flood_unread(terminal: &RemoteTerminalService) -> Result<(), Error> {
    assert!(
        terminal.input(&vec![0; TERMINAL_CHUNK + 1]).is_err(),
        "input longer than a chunk is refused"
    );
    assert!(
        terminal
            .resize(TerminalSize {
                rows: 1,
                columns: 0
            })
            .is_err(),
        "a size without columns is refused"
    );
    terminal.input(b"yes VNI_STREAM\n")?;
    // Deliberately leave the bounded controller queue unread.
    holds(
        "the terminal whose output goes unread stays open",
        Duration::from_millis(100),
        || !terminal.is_cancelled(),
    );
    Ok(())
}

/// Closing a terminal whose output goes unread does not wait for it to be
/// read: the terminal ends on both sides and the connection carries on.
#[test]
fn closing_a_terminal_does_not_wait_for_unread_output() -> TestResult {
    let trace = traced();
    let pair = Pair::for_services(&[ServiceKind::Terminal]);
    pair.host
        .core
        .set_terminal_factory(Some(shell(&pair.host.folder())))?;
    let connection = pair.connect_services();
    let terminal = open(&pair.controller, &connection)?;
    flood_unread(&terminal)?;
    terminal.close();
    wait_for("the terminal to end on both sides", DEFAULT_WAIT, || {
        (terminal.is_cancelled() && pair.host.hosted(None).is_empty()).then_some(())
    });
    assert_eq!(
        phase(&pair.controller, &connection),
        SessionPhase::Active,
        "closing the terminal leaves its connection open"
    );
    trace.wait_for_records("terminal stream ended", 1);
    trace.wait_for_records("terminal pipe ended", 1);
    vnilink_testkit::assert_story(
        expect![[r#"
        controller:
          terminal: terminal pipe ended closed_here=true error=no session service=1
        host:
          terminal: terminal opened columns=80 persistent=false resumed=false rows=24 service=1
          terminal: terminal stream ended ended=viewer closed explicit_close=false service=1
    "#]],
        || trace.tell(&["terminal"]).to_string(),
    );
    trace.assert_unrecorded("terminal content", &["VNI_STREAM"]);
    Ok(())
}

/// Disconnecting while a terminal's output goes unread does not wait for
/// it to be read: the terminal ends on both sides.
///
/// The story states only the terminal opening: on a disconnect the host
/// sometimes records no end of the terminal's stream, and the controller's
/// record of its pipe's end names either cause.
#[test]
fn disconnecting_does_not_wait_for_unread_terminal_output() -> TestResult {
    let trace = traced();
    let pair = Pair::for_services(&[ServiceKind::Terminal]);
    pair.host
        .core
        .set_terminal_factory(Some(shell(&pair.host.folder())))?;
    let connection = pair.connect_services();
    let terminal = open(&pair.controller, &connection)?;
    flood_unread(&terminal)?;
    pair.controller.core.disconnect_session(connection.id)?;
    wait_for("the terminal to end on both sides", DEFAULT_WAIT, || {
        (terminal.is_cancelled() && pair.host.hosted(None).is_empty()).then_some(())
    });
    vnilink_testkit::assert_story(
        expect![[r#"
        host:
          terminal: terminal opened columns=80 persistent=false resumed=false rows=24 service=1
    "#]],
        || {
            trace
                .tell(&["terminal"])
                .only(&["terminal opened"])
                .to_string()
        },
    );
    trace.assert_unrecorded("terminal content", &["VNI_STREAM"]);
    Ok(())
}

/// A shell backend that cannot start refuses the terminal with its reason,
/// and the host releases it while the connection carries on.
#[test]
fn a_shell_that_cannot_start_refuses_the_terminal_and_leaves_the_connection() -> TestResult {
    struct Failed;
    impl HostTerminalFactory for Failed {
        fn open(&self, _: TerminalSize, _: BackendStartup) -> Result<TerminalBackend, Error> {
            Err(Error::Unsupported("test terminal startup"))
        }
    }
    let trace = traced();
    let pair = Pair::for_services(&[ServiceKind::Terminal]);
    pair.host
        .core
        .set_terminal_factory(Some(Arc::new(Failed)))?;
    let connection = pair.connect_services();
    let owner = pair.attach(&connection, ServiceKind::Terminal);
    assert!(
        matches!(
            pair.controller
                .core
                .open_terminal_service(owner.id, owner.generation, SIZE),
            Err(Error::Unsupported(_))
        ),
        "a backend that cannot start refuses with its reason"
    );
    pair.host.wait_released(None);
    assert_eq!(
        phase(&pair.controller, &connection),
        SessionPhase::Active,
        "a refused terminal leaves its connection open"
    );
    trace.wait_for_records("service released", 1);
    vnilink_testkit::assert_story(
        expect![[r#"
        host:
          hosting: service opened connection=1 kind=Some(Terminal) service=1
          terminal: terminal not opened error=unsupported reason=terminal backend startup resume=false service=1
          terminal: terminal stream ended ended=unsupported explicit_close=false service=1
          hosting: service failed kind=Terminal reason=unsupported service=1
          hosting: service released service=1
    "#]],
        || trace.story(&["terminal", "hosting"]),
    );
    Ok(())
}

/// Two terminals on one connection run separate shells; closing one leaves
/// the other running.
#[test]
fn simultaneous_terminals_run_independent_shells() -> TestResult {
    let trace = traced();
    let pair = Pair::for_services(&[ServiceKind::Terminal]);
    pair.host
        .core
        .set_terminal_factory(Some(shell(&pair.host.folder())))?;
    let connection = pair.connect_services();
    let first = open(&pair.controller, &connection)?;
    let second = open(&pair.controller, &connection)?;
    assert_ne!(
        first.session().id,
        second.session().id,
        "each terminal is its own session"
    );
    assert_ne!(
        first.session().process_id,
        second.session().process_id,
        "each terminal runs its own shell"
    );
    first.close();
    wait_for("the first terminal to end", DEFAULT_WAIT, || {
        first.is_cancelled().then_some(())
    });
    trace.wait_for_records("terminal stream ended", 1);
    assert!(
        !second.is_cancelled(),
        "closing one terminal leaves the other open"
    );
    second.input(b"printf 'SECOND_ALIVE\\n'; exit 0\n")?;
    let mut output = Vec::new();
    let exit = read_until(
        &second,
        &mut output,
        "the second shell to exit",
        SHELL,
        |_| false,
    );
    assert_eq!(exit, Some(0), "the second shell runs to its own exit");
    assert!(
        String::from_utf8_lossy(&output).contains("SECOND_ALIVE"),
        "the second shell still answers"
    );
    trace.wait_for_records("terminal stream ended", 2);
    trace.wait_for_records("terminal pipe ended", 1);
    vnilink_testkit::assert_story(
        expect![[r#"
        controller:
          terminal: terminal pipe ended closed_here=true error=no session service=1
        host:
          terminal: terminal opened columns=80 persistent=false resumed=false rows=24 service=1
          terminal: terminal opened columns=80 persistent=false resumed=false rows=24 service=2
          terminal: terminal stream ended ended=viewer closed explicit_close=false service=1
          terminal: terminal shell exited code=0 service=2
          terminal: terminal stream ended ended=shell exited explicit_close=true service=2
    "#]],
        || trace.tell(&["terminal"]).to_string(),
    );
    trace.assert_unrecorded("terminal content", &["SECOND_ALIVE"]);
    Ok(())
}
