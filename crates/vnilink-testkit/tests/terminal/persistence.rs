//! Persistent terminals: a shell the host keeps through a disconnect, found
//! and resumed by its own peer only, and closed past input it cannot take.

use expect_test::expect;

use vnilink::terminal::{TerminalOptions, TerminalSize, TERMINAL_CHUNK};
use vnilink::{Error, ServiceKind, SessionPhase};
use vnilink_testkit::terminal::{contains, read_until, shell};
use vnilink_testkit::{traced, wait_for, Engine, Pair, TestResult, DEFAULT_WAIT};

use super::support::{host_ended_shells, options, phase, SHELL};

/// The story of a scenario that disconnects: the terminals the host opened
/// or refused. On a disconnect the host sometimes records no end of the
/// terminal's stream, and the controller's record of its pipe's end names
/// either cause, so neither is part of it.
const ACROSS_A_DISCONNECT: &[&str] = &["terminal opened", "terminal not opened"];

/// Waits until `host` keeps the shell `id` with no terminal attached.
fn host_keeps_detached(host: &Engine, id: &str) {
    wait_for("the host to keep the shell detached", DEFAULT_WAIT, || {
        host.core
            .hosted_terminal_sessions()
            .ok()?
            .iter()
            .any(|shell| shell.id == id && !shell.attached)
            .then_some(())
    });
}

/// A persistent shell outlives its connection: after reconnecting, the
/// controller finds it, resumes the same process at a new size with its
/// history replayed and its environment intact, and the old terminal is
/// gone; closing it by id ends it on the host.
#[test]
fn a_persistent_shell_resumes_with_its_identity_history_and_environment() -> TestResult {
    let trace = traced();
    let pair = Pair::for_services(&[ServiceKind::Terminal]);
    let controller = &pair.controller.core;
    pair.host
        .core
        .set_terminal_factory(Some(shell(&pair.host.folder())))?;
    let connection = pair.connect_services();
    let terminal = pair
        .controller
        .open_terminal(&connection, options(true, None))?;
    let metadata = terminal.session();
    assert!(metadata.process_id > 0, "the shell's process is named");
    terminal.input(b"VNI_KEEP=kept; printf 'BEFORE_%s\\n' DETACH\n")?;
    let mut before = Vec::new();
    read_until(
        &terminal,
        &mut before,
        "the shell's answer",
        SHELL,
        |output| contains(output, "BEFORE_DETACH"),
    );
    controller.disconnect_session(connection.id)?;
    host_keeps_detached(&pair.host, &metadata.id);
    let connection = pair.connect_services();
    let discovered = controller.remote_terminal_sessions(connection.id, connection.generation)?;
    assert_eq!(
        discovered.len(),
        1,
        "the controller finds exactly its kept shell: {discovered:?}"
    );
    assert_eq!(discovered[0].id, metadata.id, "the kept shell is the same");
    assert!(!discovered[0].attached, "the kept shell is detached");
    let resumed = pair.controller.open_terminal(
        &connection,
        TerminalOptions {
            size: TerminalSize {
                rows: 40,
                columns: 120,
            },
            persistent: true,
            resume_id: Some(metadata.id.clone()),
        },
    )?;
    assert_eq!(
        resumed.session().id,
        metadata.id,
        "the resumed terminal is the kept shell"
    );
    assert_eq!(
        resumed.session().process_id,
        metadata.process_id,
        "the resumed terminal runs the same process"
    );
    resumed.input(b"printf 'AFTER:%s\\n' \"$VNI_KEEP\"; stty size\n")?;
    let mut replay = Vec::new();
    read_until(
        &resumed,
        &mut replay,
        "the resumed shell's answer",
        SHELL,
        |output| contains(output, "AFTER:kept") && contains(output, "40 120"),
    );
    assert!(
        String::from_utf8_lossy(&replay).contains("BEFORE_DETACH"),
        "the history before the disconnect is replayed"
    );
    assert!(
        terminal.input(b"wrong\n").is_err(),
        "the terminal of the old connection takes no input"
    );
    controller.close_remote_terminal_session(connection.id, connection.generation, &metadata.id)?;
    wait_for("the resumed terminal to end", DEFAULT_WAIT, || {
        resumed.is_cancelled().then_some(())
    });
    host_ended_shells(&pair.host);
    trace.wait_for_records("terminal opened", 2);
    expect![[r#"
        host:
          terminal: terminal opened columns=80 persistent=true resumed=false rows=24 service=1
          terminal: terminal opened columns=120 persistent=true resumed=true rows=40 service=2
    "#]]
    .assert_eq(
        &trace
            .tell(&["terminal"])
            .only(ACROSS_A_DISCONNECT)
            .to_string(),
    );
    trace.assert_unrecorded("terminal content", &["VNI_KEEP", "BEFORE_", "AFTER:"]);
    Ok(())
}

/// A kept shell belongs to the peer that opened it: another peer neither
/// finds, closes nor resumes it, and revoking Terminal ends it on the host
/// while that other peer's connection carries on.
#[test]
fn a_kept_shell_is_its_peers_alone_and_revoking_terminal_ends_it() -> TestResult {
    let trace = traced();
    let pair = Pair::for_services(&[ServiceKind::Terminal]);
    let other = pair.add_viewer("other");
    let (host, owner) = (&pair.host, &pair.controller);
    host.core
        .set_terminal_factory(Some(shell(&host.folder())))?;
    let connection = owner.connect_services(host)?;
    let terminal = owner.open_terminal(&connection, options(true, None))?;
    let retained = terminal.session();
    owner.core.disconnect_session(connection.id)?;
    host_keeps_detached(host, &retained.id);
    let foreign = other.connect_services(host)?;
    assert!(
        other
            .core
            .remote_terminal_sessions(foreign.id, foreign.generation)?
            .is_empty(),
        "another peer does not find the kept shell"
    );
    assert!(
        other
            .core
            .close_remote_terminal_session(foreign.id, foreign.generation, &retained.id)
            .is_err(),
        "another peer cannot close the kept shell"
    );
    assert!(
        matches!(
            other.open_terminal(&foreign, options(true, Some(retained.id.clone()))),
            Err(Error::Unauthorized)
        ),
        "another peer cannot resume the kept shell"
    );
    assert_eq!(
        host.core.hosted_terminal_sessions()?[0].id,
        retained.id,
        "another peer's attempts leave the kept shell alone"
    );
    host.core
        .set_service_sharing(ServiceKind::Terminal, false)?;
    host_ended_shells(host);
    assert_eq!(
        phase(&other, &foreign),
        SessionPhase::Active,
        "revoking Terminal leaves the other peer's connection open"
    );
    trace.wait_for_records("terminal not opened", 1);
    expect![[r#"
        host:
          terminal: terminal opened columns=80 persistent=true resumed=false rows=24 service=1
          terminal: terminal not opened error=unauthorized resume=true service=2
    "#]]
    .assert_eq(
        &trace
            .tell(&["terminal"])
            .only(ACROSS_A_DISCONNECT)
            .to_string(),
    );
    Ok(())
}

/// Closing a persistent terminal whose shell stopped reading input ends it
/// on the host, though its input queue is full, and leaves the connection.
#[test]
fn closing_a_persistent_terminal_bypasses_stalled_input() -> TestResult {
    let trace = traced();
    let pair = Pair::for_services(&[ServiceKind::Terminal]);
    pair.host
        .core
        .set_terminal_factory(Some(shell(&pair.host.folder())))?;
    let connection = pair.connect_services();
    let terminal = pair
        .controller
        .open_terminal(&connection, options(true, None))?;
    terminal.input(b"stty -echo; printf 'SLEEP_%s\\n' STARTED; sleep 60\n")?;
    let mut output = Vec::new();
    read_until(
        &terminal,
        &mut output,
        "the shell to sleep",
        SHELL,
        |output| contains(output, "SLEEP_STARTED"),
    );
    // Fill the input queue a shell that sleeps never reads; some is
    // refused once it is full.
    for _ in 0..128 {
        let _ = terminal.input(&vec![b'x'; TERMINAL_CHUNK]);
    }
    terminal.close();
    host_ended_shells(&pair.host);
    assert_eq!(
        phase(&pair.controller, &connection),
        SessionPhase::Active,
        "closing the terminal leaves its connection open"
    );
    trace.wait_for_records("terminal stream ended", 1);
    trace.wait_for_records("terminal pipe ended", 1);
    expect![[r#"
        controller:
          terminal: terminal pipe ended closed_here=true error=no session service=1
        host:
          terminal: terminal opened columns=80 persistent=true resumed=false rows=24 service=1
          terminal: terminal stream ended ended=viewer closed explicit_close=false service=1
    "#]]
    .assert_eq(&trace.tell(&["terminal"]).to_string());
    trace.assert_unrecorded("terminal content", &["SLEEP_"]);
    Ok(())
}
