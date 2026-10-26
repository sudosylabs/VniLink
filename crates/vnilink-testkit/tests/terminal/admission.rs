//! Admitting a terminal: a shell still starting is cancelled with its
//! grant, and the host's limits count shells starting as well as running,
//! per peer and in all.

use expect_test::expect;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{channel, sync_channel, Receiver, SyncSender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use vnilink::terminal::{
    HostTerminalFactory, TerminalBackend, TerminalOptions, TerminalPolicy, TerminalSize,
};
use vnilink::{BackendStartup, Error, ServiceInfo, ServiceKind, SessionInfo, SessionPhase};
use vnilink_testkit::terminal::{shell, SIZE};
use vnilink_testkit::{traced, wait_for, Engine, Pair, TestResult, DEFAULT_WAIT};

use super::support::{host_ended_shells, phase};

/// Within this a cancelled startup reaches the backend still starting.
const CANCELLED_WITHIN: Duration = Duration::from_secs(1);

/// Revoking Terminal while its shell is still starting cancels the startup
/// without a release from the test: no terminal is admitted, the host
/// keeps no attached shell, and the connection carries on.
#[test]
fn revoking_terminal_cancels_a_shell_still_starting() -> TestResult {
    /// A backend that waits for a release that never comes, so only
    /// cancellation ends its startup.
    struct Blocking {
        entered: SyncSender<()>,
        release: Mutex<Receiver<()>>,
        opened: AtomicBool,
        finished: SyncSender<()>,
        shell: Arc<dyn HostTerminalFactory>,
    }
    impl HostTerminalFactory for Blocking {
        fn open(
            &self,
            size: TerminalSize,
            startup: BackendStartup,
        ) -> Result<TerminalBackend, Error> {
            self.opened.store(true, Ordering::SeqCst);
            let _ = self.entered.try_send(());
            let result = startup.receive(&self.release.lock().unwrap());
            self.finished.send(()).unwrap();
            result?;
            self.shell.open(size, startup)
        }
    }

    let trace = traced();
    let pair = Pair::for_services(&[ServiceKind::Terminal]);
    let (host, controller) = (&pair.host.core, &pair.controller.core);
    let (entered, startup_entered) = sync_channel(1);
    let (_release, released) = channel();
    let (finished, startup_finished) = sync_channel(1);
    let factory = Arc::new(Blocking {
        entered,
        release: Mutex::new(released),
        opened: AtomicBool::new(false),
        finished,
        shell: shell(&pair.host.root()),
    });
    host.set_terminal_factory(Some(factory.clone()))?;
    let connection = pair.connect_services();
    let owner = pair.attach(&connection, ServiceKind::Terminal);
    let opened = std::thread::scope(|scope| -> Result<_, Error> {
        let opening =
            scope.spawn(|| controller.open_terminal_service(owner.id, owner.generation, SIZE));
        startup_entered
            .recv_timeout(Duration::from_secs(8))
            .expect("the shell to start");
        assert!(
            factory.opened.load(Ordering::SeqCst),
            "the host started the shell"
        );
        let revoked = Instant::now();
        host.set_service_sharing(ServiceKind::Terminal, false)?;
        let left = CANCELLED_WITHIN.saturating_sub(revoked.elapsed());
        assert!(
            startup_finished.recv_timeout(left).is_ok(),
            "cancellation reaches the starting shell within {CANCELLED_WITHIN:?}"
        );
        Ok(opening.join().expect("the opening thread"))
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
        phase(&pair.controller, &connection),
        SessionPhase::Active,
        "a cancelled startup leaves its connection open"
    );
    trace.wait_for_records("service released", 1);
    expect![[r#"
        host:
          hosting: service opened connection=1 kind=Some(Terminal) service=1
          terminal: terminal stream ended ended=service closed explicit_close=false service=1
          hosting: service released service=1
    "#]]
    .assert_eq(&trace.story(&["terminal", "hosting"]));
    Ok(())
}

/// The host's terminal limits count a shell still starting: while one
/// starts its policy cannot change and its peer's second terminal is
/// refused before a shell starts, while another peer's opens. Closing one
/// leaves the other, and a policy without persistence refuses a persistent
/// terminal before a shell starts.
#[test]
fn terminal_limits_count_shells_starting_and_keep_peers_independent() -> TestResult {
    /// A backend whose first startup waits for the test's release.
    struct Gated {
        calls: AtomicUsize,
        entered: SyncSender<()>,
        release: Mutex<Receiver<()>>,
        shell: Arc<dyn HostTerminalFactory>,
    }
    impl HostTerminalFactory for Gated {
        fn open(
            &self,
            size: TerminalSize,
            startup: BackendStartup,
        ) -> Result<TerminalBackend, Error> {
            if self.calls.fetch_add(1, Ordering::SeqCst) == 0 {
                self.entered.send(()).unwrap();
                startup.receive(&self.release.lock().unwrap())?;
            }
            self.shell.open(size, startup)
        }
    }

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
    let (entered, startup_entered) = sync_channel(1);
    let (release, released) = channel();
    let factory = Arc::new(Gated {
        calls: AtomicUsize::new(0),
        entered,
        release: Mutex::new(released),
        shell: shell(&host.root()),
    });
    host.core.set_terminal_factory(Some(factory.clone()))?;
    let to_a = a.connect_services(host)?;
    let to_b = b.connect_services(host)?;
    let attach = |peer: &Engine, connection: &SessionInfo| -> Result<ServiceInfo, Error> {
        peer.core.attach_controller_service(
            connection.id,
            connection.generation,
            ServiceKind::Terminal,
        )
    };
    let first = attach(a, &to_a)?;
    let (ta, tb) = std::thread::scope(|scope| -> Result<_, Error> {
        let opening = scope.spawn(|| {
            a.core
                .open_terminal_service(first.id, first.generation, SIZE)
        });
        startup_entered
            .recv_timeout(Duration::from_secs(3))
            .expect("the first shell to start");
        assert!(
            matches!(host.core.set_terminal_policy(policy), Err(Error::Busy)),
            "the policy does not change while a shell starts"
        );
        let duplicate = attach(a, &to_a)?;
        assert!(
            matches!(
                a.core
                    .open_terminal_service(duplicate.id, duplicate.generation, SIZE),
                Err(Error::Busy)
            ),
            "a peer's shell still starting counts against its limit"
        );
        assert_eq!(
            factory.calls.load(Ordering::SeqCst),
            1,
            "refusal must precede native startup"
        );
        let sibling = attach(&b, &to_b)?;
        let tb = b
            .core
            .open_terminal_service(sibling.id, sibling.generation, SIZE)?;
        assert_eq!(
            factory.calls.load(Ordering::SeqCst),
            2,
            "another peer's terminal opens under its own limit"
        );
        release.send(()).expect("the first shell to wait");
        Ok((opening.join().expect("the opening thread")?, tb))
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
    tb.close();
    host_ended_shells(host);
    let denied = TerminalPolicy {
        allow_persistence: false,
        ..policy
    };
    host.core.set_terminal_policy(denied)?;
    let owner = attach(a, &to_a)?;
    assert!(
        matches!(
            a.core.open_terminal_service_with_options(
                owner.id,
                owner.generation,
                TerminalOptions {
                    size: SIZE,
                    persistent: true,
                    resume_id: None,
                }
            ),
            Err(Error::Unauthorized)
        ),
        "a policy without persistence refuses a persistent terminal"
    );
    assert_eq!(
        factory.calls.load(Ordering::SeqCst),
        2,
        "the refusal precedes native startup"
    );
    trace.wait_for_records("terminal stream ended", 4);
    trace.wait_for_records("terminal pipe ended", 2);
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
    "#]]
    .assert_eq(&trace.tell(&["terminal"]).to_string());
    Ok(())
}
