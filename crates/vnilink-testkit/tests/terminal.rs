// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! The Terminal service alone: the host's real shell, a terminal opened
//! from the controller as the application opens one.

#[path = "terminal/admission.rs"]
mod admission;
#[path = "terminal/lifecycle.rs"]
mod lifecycle;
#[path = "terminal/persistence.rs"]
mod persistence;
#[path = "terminal/support.rs"]
mod support;

use expect_test::expect;

use vnilink::terminal::TerminalSize;
use vnilink::{Error, SessionPhase};
use vnilink_testkit::terminal::{check, contains, read_until, Hosting, Typed, ENTER};
use vnilink_testkit::{traced, wait_for, TestResult, DEFAULT_WAIT};

/// What is typed reaches the shell, its output comes back, and its exit
/// code ends the terminal; the host records the shell opening and its exit.
#[test]
fn a_shell_runs_what_is_typed_and_ends_with_its_exit_code() -> TestResult {
    let trace = traced();
    check(
        &trace,
        Typed {
            size: None,
            lines: if cfg!(windows) {
                &["echo VNI_^READY", "exit 7"]
            } else {
                &["printf 'VNI_%s\\n' READY", "exit 7"]
            },
            shows: &["VNI_READY"],
        },
        expect![[r#"
            host:
              terminal: terminal opened columns=80 persistent=false resumed=false rows=24 service=1
              terminal: terminal shell exited code=7 service=1
              terminal: terminal stream ended ended=shell exited explicit_close=true service=1
        "#]],
    )
}

/// A resize reaches the shell's terminal, applied in order with input,
/// before the next command.
#[test]
fn a_resize_reaches_the_shell() -> TestResult {
    let trace = traced();
    check(
        &trace,
        Typed {
            size: Some(TerminalSize {
                rows: 39,
                columns: 117,
            }),
            lines: if cfg!(windows) {
                &["mode con", "exit 0"]
            } else {
                &["stty size", "exit 0"]
            },
            shows: if cfg!(windows) {
                &["Lines:          39", "Columns:        117"]
            } else {
                &["39 117"]
            },
        },
        expect![[r#"
            host:
              terminal: terminal opened columns=80 persistent=false resumed=false rows=24 service=1
              terminal: terminal shell exited code=0 service=1
              terminal: terminal stream ended ended=shell exited explicit_close=true service=1
        "#]],
    )
}

/// Text outside ASCII passes both ways intact.
#[test]
fn text_outside_ascii_passes_intact() -> TestResult {
    let trace = traced();
    check(
        &trace,
        Typed {
            size: None,
            lines: if cfg!(windows) {
                &["chcp 65001 >nul", "echo VNI_^α€", "exit 0"]
            } else {
                &["printf 'VNI_\\316\\261\\342\\202\\254\\n'", "exit 0"]
            },
            shows: &["VNI_α€"],
        },
        expect![[r#"
            host:
              terminal: terminal opened columns=80 persistent=false resumed=false rows=24 service=1
              terminal: terminal shell exited code=0 service=1
              terminal: terminal stream ended ended=shell exited explicit_close=true service=1
        "#]],
    )
}

/// Without the host's Terminal grant no terminal opens, the refusal is
/// recorded, and the connection carries on.
#[test]
fn without_the_grant_no_terminal_opens_and_the_connection_lives() -> TestResult {
    let trace = traced();
    let hosting = Hosting::offering(false, |host| {
        vnilink_testkit::terminal::open_shell(&host.folder())
    })?;
    let opened = hosting.open();
    assert!(
        matches!(opened, Err(Error::Unauthorized)),
        "without the grant no terminal opens: {:?}",
        opened.err()
    );
    assert_eq!(
        hosting.phase()?,
        SessionPhase::Active,
        "a refused terminal leaves its connection open"
    );
    trace.wait_for_records("service refused", 1);
    vnilink_testkit::assert_story(
        expect![[r#"
        controller:
          service: service phase from=active generation=1 kind=Terminal service=1 to=failed
        host:
          hosting: service refused connection=1 kind=Terminal status=not permitted
    "#]],
        || trace.story(&["hosting", "service"]),
    );
    Ok(())
}

/// Closing the terminal ends the shell on the host.
#[test]
fn closing_the_terminal_ends_the_shell() -> TestResult {
    let trace = traced();
    let hosting = Hosting::shell()?;
    let terminal = hosting.open()?;
    let ready = if cfg!(windows) {
        "echo VNI_^READY"
    } else {
        "printf 'VNI_%s\\n' READY"
    };
    terminal.input(format!("{ready}{ENTER}").as_bytes())?;
    let mut output = Vec::new();
    let exit = read_until(
        &terminal,
        &mut output,
        "the shell",
        DEFAULT_WAIT,
        |output| contains(output, "VNI_READY"),
    );
    assert_eq!(exit, None, "the shell runs until the terminal closes");
    terminal.close();
    wait_for("the host to end the shell", DEFAULT_WAIT, || {
        let host = &hosting.pair.host.core;
        (host.hosted_services().ok()?.is_empty()
            && host.hosted_terminal_sessions().ok()?.is_empty())
        .then_some(())
    });
    trace.wait_for_records("terminal stream ended", 1);
    vnilink_testkit::assert_story(
        expect![[r#"
        host:
          terminal: terminal opened columns=80 persistent=false resumed=false rows=24 service=1
          terminal: terminal stream ended ended=viewer closed explicit_close=false service=1
    "#]],
        || trace.engine_story("host", &["terminal"]),
    );
    Ok(())
}
