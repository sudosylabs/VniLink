// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! Admitting a terminal: a shell still starting is cancelled with its
//! grant, and the host's limits count shells starting as well as running,
//! per peer and in all.

use expect_test::expect;

use vnilink::terminal::TerminalPolicy;
use vnilink::{Error, ServiceKind, SessionPhase};
use vnilink_testkit::terminal::{
    open_shell, options, wait_shells_ended, Held, HeldStartup, Hosting,
};
use vnilink_testkit::{traced, wait_for, Engine, Pair, TestResult, DEFAULT_WAIT};

/// Revoking Terminal while its shell is still starting cancels the startup
/// without a release from the test: it ends cancelled, not by its own
/// deadline, no terminal is admitted, the host keeps no attached shell,
/// and the connection carries on.
#[test]
fn revoking_terminal_cancels_a_shell_still_starting() -> TestResult {
    let trace = traced();
    let mut held = None;
    let hosting = Hosting::offering(true, |host| {
        let factory = HeldStartup::new(open_shell(&host.folder())?);
        held = Some(factory.clone());
        Ok(factory)
    })?;
    let held = held.ok_or("the host's backend")?;
    let host = &hosting.pair.host.core;
    let opened = std::thread::scope(|scope| -> Result<_, Error> {
        let opening = scope.spawn(|| hosting.open());
        held.wait_holding();
        host.set_service_sharing(ServiceKind::Terminal, false)?;
        assert_eq!(
            held.wait_finished(),
            Held::Cancelled,
            "revoking Terminal cancels the shell still starting"
        );
        Ok(opening.join().expect("the opening thread returns"))
    })?;
    assert!(
        opened.is_err(),
        "a cancelled startup never admits a terminal"
    );
    wait_for("the host to release the terminal", DEFAULT_WAIT, || {
        (host.hosted_services().ok()?.is_empty()
            && host
                .hosted_terminal_sessions()
                .ok()?
                .iter()
                .all(|shell| !shell.attached))
        .then_some(())
    });
    assert_eq!(
        hosting.phase()?,
        SessionPhase::Active,
        "a cancelled startup leaves its connection open"
    );
    trace.wait_for_records("service released", 1);
    vnilink_testkit::assert_story(
        expect![[r#"
        host:
          hosting: service opened connection=1 kind=Some(Terminal) service=1
          terminal: terminal stream ended ended=service closed explicit_close=false service=1
          hosting: service released service=1
    "#]],
        || trace.story(&["terminal", "hosting"]),
    );
    Ok(())
}

/// The host's terminal limits count a shell still starting: while one
/// starts its policy cannot change and its peer's second terminal is
/// refused before a shell starts, while another peer's opens. Closing one
/// leaves the other, and a policy without persistence refuses a persistent
/// terminal before a shell starts.
#[test]
fn terminal_limits_count_shells_starting_and_keep_peers_independent() -> TestResult {
    let trace = traced();
    let pair = Pair::for_services(&[ServiceKind::Terminal]);
    let b = pair.add_viewer("b");
    let (host, a) = (&pair.host, &pair.controller);
    let policy = TerminalPolicy {
        max_sessions: 2,
        max_sessions_per_peer: 1,
        ..Default::default()
    };
    host.core.set_terminal_policy(policy)?;
    let factory = HeldStartup::new(open_shell(&host.folder())?);
    host.core.set_terminal_factory(Some(factory.clone()))?;
    let to_a = a.connect_services(host)?;
    let to_b = b.connect_services(host)?;
    let open = |peer: &Engine, connection| peer.open_terminal(connection, options(false, None));
    let (ta, tb) = std::thread::scope(|scope| -> Result<_, Error> {
        let opening = scope.spawn(|| open(a, &to_a));
        factory.wait_holding();
        assert!(
            matches!(host.core.set_terminal_policy(policy), Err(Error::Busy)),
            "the policy does not change while a shell starts"
        );
        assert!(
            matches!(open(a, &to_a), Err(Error::Busy)),
            "a peer's shell still starting counts against its limit"
        );
        assert_eq!(factory.startups(), 1, "refusal must precede native startup");
        let tb = open(&b, &to_b)?;
        assert_eq!(
            factory.startups(),
            2,
            "another peer's terminal opens under its own limit"
        );
        factory.release();
        assert_eq!(
            factory.wait_finished(),
            Held::Released,
            "the held shell starts once released"
        );
        Ok((opening.join().expect("the opening thread returns")?, tb))
    })?;
    assert_eq!(
        host.core.hosted_terminal_sessions()?.len(),
        2,
        "both peers' shells run"
    );
    ta.close();
    wait_for("the host to end the first shell", DEFAULT_WAIT, || {
        (host.core.hosted_terminal_sessions().ok()?.len() == 1).then_some(())
    });
    assert!(
        !tb.is_cancelled(),
        "closing one peer's terminal leaves the other's"
    );
    // The shell ends before its stream records why; the other closes after.
    trace.wait_for_records("terminal stream ended", 2);
    tb.close();
    wait_shells_ended(host);
    let denied = TerminalPolicy {
        allow_persistence: false,
        ..policy
    };
    host.core.set_terminal_policy(denied)?;
    assert!(
        matches!(
            a.open_terminal(&to_a, options(true, None)),
            Err(Error::Unauthorized)
        ),
        "a policy without persistence refuses a persistent terminal"
    );
    assert_eq!(factory.startups(), 2, "the refusal precedes native startup");
    trace.wait_for_records("terminal stream ended", 4);
    trace.wait_for_records("terminal pipe ended", 2);
    vnilink_testkit::assert_story(
        expect![[r#"
        b:
          terminal: terminal pipe ended closed_here=true error=no session service=1
        controller:
          terminal: terminal pipe ended closed_here=true error=no session service=1
        host:
          terminal: terminal not opened error=busy resume=false service=2
          terminal: terminal stream ended ended=busy explicit_close=false service=2
          terminal: terminal opened columns=80 persistent=false resumed=false rows=24 service=3
          terminal: terminal opened columns=80 persistent=false resumed=false rows=24 service=1
          terminal: terminal stream ended ended=viewer closed explicit_close=false service=1
          terminal: terminal stream ended ended=viewer closed explicit_close=false service=3
          terminal: terminal not opened error=unauthorized resume=false service=4
          terminal: terminal stream ended ended=unauthorized explicit_close=false service=4
    "#]],
        || trace.tell(&["terminal"]).to_string(),
    );
    Ok(())
}
