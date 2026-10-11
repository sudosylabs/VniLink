use super::support::{pair_for_control, start_direct};
use crate::terminal::{HostTerminalFactory, NativeTerminalFactory, TerminalEvent, TerminalSize};
use crate::{Core, Error, ServiceKind, SessionPhase};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};
const SIZE: TerminalSize = TerminalSize {
    rows: 24,
    columns: 80,
};
fn wait(mut ready: impl FnMut() -> bool) {
    let end = Instant::now() + Duration::from_secs(8);
    while !ready() {
        assert!(Instant::now() < end, "terminal condition timed out");
        std::thread::sleep(Duration::from_millis(5));
    }
}
fn connect(controller: &Core, host: &Core) -> crate::SessionInfo {
    let pending = controller
        .begin_connect_services(&host.endpoint_id())
        .unwrap();
    wait(|| controller.controller_session(pending.id).unwrap().phase == SessionPhase::Active);
    controller.controller_session(pending.id).unwrap()
}
fn native() -> Arc<NativeTerminalFactory> {
    Arc::new(NativeTerminalFactory::new("/bin/sh".into(), std::env::temp_dir()).unwrap())
}
#[test]
fn native_terminal_round_trip_resize_exit_and_sibling_lifetime() {
    let controller = start_direct();
    let host = start_direct();
    pair_for_control(&host, &controller);
    host.set_terminal_factory(Some(native())).unwrap();
    let connection = connect(&controller, &host);
    assert!(
        controller
            .remote_service_support(connection.id, connection.generation)
            .unwrap()
            .terminal
    );
    let owner = controller
        .attach_controller_service(connection.id, connection.generation, ServiceKind::Terminal)
        .unwrap();
    assert!(matches!(
        controller.open_terminal_service(owner.id, owner.generation, SIZE),
        Err(Error::Unauthorized)
    ));
    // Failed admission only closes the attempted owner, never the parent.
    wait(|| {
        matches!(
            controller.controller_service(owner.id).unwrap().phase,
            crate::ServicePhase::Failed(_)
        )
    });
    host.authorize_service(&controller.endpoint_id(), ServiceKind::Terminal, true)
        .unwrap();
    let files = controller
        .attach_controller_service(connection.id, connection.generation, ServiceKind::Files)
        .unwrap();
    let files_lease = controller
        .service_lease(files.id, files.generation)
        .unwrap();
    let owner = controller
        .attach_controller_service(connection.id, connection.generation, ServiceKind::Terminal)
        .unwrap();
    let terminal = controller
        .open_terminal_service(owner.id, owner.generation, SIZE)
        .unwrap();
    assert!(matches!(
        controller.open_terminal_service(owner.id, owner.generation, SIZE),
        Err(Error::Busy)
    ));
    assert!(!terminal.is_cancelled());
    terminal
        .resize(TerminalSize {
            rows: 39,
            columns: 117,
        })
        .unwrap();
    terminal
        .input(b"printf 'VNI_READY\\n'; stty size; printf '\\316\\261\\n'; exit 7\n")
        .unwrap();
    let mut output = Vec::new();
    let mut exit = None;
    wait(|| {
        while let Some(event) = terminal.poll() {
            match event {
                TerminalEvent::Output(bytes) => output.extend(bytes),
                TerminalEvent::Exited(code) => exit = Some(code),
                TerminalEvent::Failed(message) => panic!("{message}"),
                TerminalEvent::OutputGap(bytes) => panic!("unexpected output gap {bytes}"),
            }
        }
        exit.is_some()
    });
    assert_eq!(exit, Some(7));
    let text = String::from_utf8(output).unwrap();
    assert!(text.contains("39 117"), "{text}");
    assert!(text.contains('α'), "{text}");
    wait(|| terminal.is_cancelled() && host.hosted_services().unwrap().is_empty());
    assert!(!files_lease.is_cancelled());
    assert_eq!(
        controller.controller_session(connection.id).unwrap().phase,
        SessionPhase::Active
    );
    assert!(matches!(terminal.input(b"x"), Err(Error::NoSession)));
}
#[test]
fn terminal_revoke_and_backend_disable_cancel_only_terminal() {
    let controller = start_direct();
    let host = start_direct();
    pair_for_control(&host, &controller);
    host.set_terminal_factory(Some(native())).unwrap();
    host.authorize_service(&controller.endpoint_id(), ServiceKind::Terminal, true)
        .unwrap();
    let connection = connect(&controller, &host);
    for disable in [false, true] {
        let owner = controller
            .attach_controller_service(connection.id, connection.generation, ServiceKind::Terminal)
            .unwrap();
        let terminal = controller
            .open_terminal_service(owner.id, owner.generation, SIZE)
            .unwrap();
        if disable {
            host.set_terminal_factory(None).unwrap();
        } else {
            host.authorize_service(&controller.endpoint_id(), ServiceKind::Terminal, false)
                .unwrap();
        }
        wait(|| terminal.is_cancelled() && host.hosted_services().unwrap().is_empty());
        assert_eq!(
            controller.controller_session(connection.id).unwrap().phase,
            SessionPhase::Active
        );
        host.authorize_service(&controller.endpoint_id(), ServiceKind::Terminal, true)
            .unwrap();
    }
    assert!(
        !controller
            .remote_service_support(connection.id, connection.generation)
            .unwrap()
            .terminal
    );
}
#[test]
fn terminal_validates_before_starting_and_close_reaps_shell() {
    assert!(SIZE.validate().is_ok());
    assert!(TerminalSize {
        rows: 0,
        columns: 80
    }
    .validate()
    .is_err());
    assert!(NativeTerminalFactory::new("sh".into(), std::env::temp_dir()).is_err());
    let mut backend = native()
        .open(SIZE, crate::BackendStartup::new(Duration::from_secs(5)))
        .unwrap();
    backend
        .commands
        .send(crate::terminal::TerminalCommand::Input(
            b"echo VNI_PID:$$; sleep 60\n".to_vec(),
        ))
        .unwrap();
    let mut bytes = Vec::new();
    let mut pid = None;
    wait(|| {
        while let Ok(event) = backend.output.try_recv() {
            if let TerminalEvent::Output(data) = event {
                bytes.extend(data);
            }
        }
        let text = String::from_utf8_lossy(&bytes);
        for line in text.lines() {
            if let Some(value) = line.trim().rsplit_once("VNI_PID:").map(|(_, value)| value) {
                pid = value.parse::<i32>().ok();
            }
        }
        pid.is_some()
    });
    drop(backend);
    wait(|| nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid.unwrap()), None).is_err());
}

#[test]
fn terminal_backpressure_does_not_prevent_close_or_parent_disconnect() {
    let controller = start_direct();
    let host = start_direct();
    pair_for_control(&host, &controller);
    host.set_terminal_factory(Some(native())).unwrap();
    host.authorize_service(&controller.endpoint_id(), ServiceKind::Terminal, true)
        .unwrap();
    let connection = connect(&controller, &host);
    for disconnect in [false, true] {
        let owner = controller
            .attach_controller_service(connection.id, connection.generation, ServiceKind::Terminal)
            .unwrap();
        let terminal = controller
            .open_terminal_service(owner.id, owner.generation, SIZE)
            .unwrap();
        assert!(terminal
            .input(&vec![0; crate::terminal::TERMINAL_CHUNK + 1])
            .is_err());
        assert!(terminal
            .resize(TerminalSize {
                rows: 1,
                columns: 0
            })
            .is_err());
        terminal.input(b"yes VNI_STREAM\n").unwrap();
        // Deliberately leave the bounded controller queue unread.
        std::thread::sleep(Duration::from_millis(100));
        if disconnect {
            controller.disconnect_session(connection.id).unwrap();
        } else {
            terminal.close();
        }
        wait(|| terminal.is_cancelled() && host.hosted_services().unwrap().is_empty());
        if !disconnect {
            assert_eq!(
                controller.controller_session(connection.id).unwrap().phase,
                SessionPhase::Active
            );
        }
    }
}

#[test]
fn terminal_backend_start_failure_leaves_parent_active() {
    struct Failed;
    impl HostTerminalFactory for Failed {
        fn open(
            &self,
            _: TerminalSize,
            _: crate::BackendStartup,
        ) -> Result<crate::terminal::TerminalBackend, Error> {
            Err(Error::Unsupported("test terminal startup"))
        }
    }
    let controller = start_direct();
    let host = start_direct();
    pair_for_control(&host, &controller);
    host.set_terminal_factory(Some(Arc::new(Failed))).unwrap();
    host.authorize_service(&controller.endpoint_id(), ServiceKind::Terminal, true)
        .unwrap();
    let connection = connect(&controller, &host);
    let owner = controller
        .attach_controller_service(connection.id, connection.generation, ServiceKind::Terminal)
        .unwrap();
    assert!(matches!(
        controller.open_terminal_service(owner.id, owner.generation, SIZE),
        Err(Error::Unsupported(_))
    ));
    wait(|| host.hosted_services().unwrap().is_empty());
    assert_eq!(
        controller.controller_session(connection.id).unwrap().phase,
        SessionPhase::Active
    );
}

#[test]
fn persistent_terminal_reconnect_restores_shell_identity_history_and_environment() {
    use crate::terminal::TerminalOptions;
    let controller = start_direct();
    let host = start_direct();
    pair_for_control(&host, &controller);
    host.set_terminal_factory(Some(native())).unwrap();
    host.authorize_service(&controller.endpoint_id(), ServiceKind::Terminal, true)
        .unwrap();
    let connection = connect(&controller, &host);
    let owner = controller
        .attach_controller_service(connection.id, connection.generation, ServiceKind::Terminal)
        .unwrap();
    let terminal = controller
        .open_terminal_service_with_options(
            owner.id,
            owner.generation,
            TerminalOptions {
                size: SIZE,
                persistent: true,
                resume_id: None,
            },
        )
        .unwrap();
    let metadata = terminal.session();
    assert!(metadata.process_id > 0);
    terminal
        .input(b"VNI_KEEP=kept; printf 'BEFORE_%s\\n' DETACH\n")
        .unwrap();
    let mut bytes = Vec::new();
    wait(|| {
        while let Some(TerminalEvent::Output(data)) = terminal.poll() {
            bytes.extend(data);
        }
        String::from_utf8_lossy(&bytes).contains("BEFORE_DETACH")
    });
    controller.disconnect_session(connection.id).unwrap();
    wait(|| {
        host.hosted_terminal_sessions()
            .unwrap()
            .iter()
            .any(|s| s.id == metadata.id && !s.attached)
    });
    let connection = connect(&controller, &host);
    let discovered = controller
        .remote_terminal_sessions(connection.id, connection.generation)
        .unwrap();
    assert_eq!(discovered.len(), 1);
    assert_eq!(discovered[0].id, metadata.id);
    assert!(!discovered[0].attached);
    let owner = controller
        .attach_controller_service(connection.id, connection.generation, ServiceKind::Terminal)
        .unwrap();
    let resumed = controller
        .open_terminal_service_with_options(
            owner.id,
            owner.generation,
            TerminalOptions {
                size: TerminalSize {
                    rows: 40,
                    columns: 120,
                },
                persistent: true,
                resume_id: Some(metadata.id.clone()),
            },
        )
        .unwrap();
    assert_eq!(resumed.session().id, metadata.id);
    assert_eq!(resumed.session().process_id, metadata.process_id);
    resumed
        .input(b"printf 'AFTER:%s\\n' \"$VNI_KEEP\"; stty size\n")
        .unwrap();
    let mut replay = Vec::new();
    wait(|| {
        while let Some(TerminalEvent::Output(data)) = resumed.poll() {
            replay.extend(data);
        }
        let text = String::from_utf8_lossy(&replay);
        text.contains("AFTER:kept") && text.contains("40 120")
    });
    assert!(String::from_utf8_lossy(&replay).contains("BEFORE_DETACH"));
    assert!(terminal.input(b"wrong\n").is_err());
    controller
        .close_remote_terminal_session(connection.id, connection.generation, &metadata.id)
        .unwrap();
    wait(|| resumed.is_cancelled());
    wait(|| host.hosted_terminal_sessions().unwrap().is_empty());
}

#[test]
fn simultaneous_terminal_shells_are_independent() {
    let controller = start_direct();
    let host = start_direct();
    pair_for_control(&host, &controller);
    host.set_terminal_factory(Some(native())).unwrap();
    host.authorize_service(&controller.endpoint_id(), ServiceKind::Terminal, true)
        .unwrap();
    let connection = connect(&controller, &host);
    let open = || {
        let owner = controller
            .attach_controller_service(connection.id, connection.generation, ServiceKind::Terminal)
            .unwrap();
        controller
            .open_terminal_service(owner.id, owner.generation, SIZE)
            .unwrap()
    };
    let first = open();
    let second = open();
    assert_ne!(first.session().id, second.session().id);
    assert_ne!(first.session().process_id, second.session().process_id);
    first.close();
    wait(|| first.is_cancelled());
    assert!(!second.is_cancelled());
    second.input(b"printf 'SECOND_ALIVE\\n'; exit 0\n").unwrap();
    let mut output = Vec::new();
    let mut exited = false;
    wait(|| {
        while let Some(event) = second.poll() {
            match event {
                TerminalEvent::Output(data) => output.extend(data),
                TerminalEvent::Exited(0) => exited = true,
                TerminalEvent::Failed(error) => panic!("{error}"),
                _ => {}
            }
        }
        exited
    });
    assert!(String::from_utf8_lossy(&output).contains("SECOND_ALIVE"));
}

#[test]
fn detached_terminal_is_peer_scoped_and_revoke_removes_retained_shell() {
    use crate::terminal::TerminalOptions;
    let owner = start_direct();
    let other = start_direct();
    let host = start_direct();
    pair_for_control(&host, &owner);
    pair_for_control(&host, &other);
    host.set_terminal_factory(Some(native())).unwrap();
    for peer in [&owner, &other] {
        host.authorize_service(&peer.endpoint_id(), ServiceKind::Terminal, true)
            .unwrap();
    }
    let connection = connect(&owner, &host);
    let service = owner
        .attach_controller_service(connection.id, connection.generation, ServiceKind::Terminal)
        .unwrap();
    let terminal = owner
        .open_terminal_service_with_options(
            service.id,
            service.generation,
            TerminalOptions {
                size: SIZE,
                persistent: true,
                resume_id: None,
            },
        )
        .unwrap();
    let retained = terminal.session();
    owner.disconnect_session(connection.id).unwrap();
    wait(|| {
        host.hosted_terminal_sessions()
            .unwrap()
            .iter()
            .any(|s| s.id == retained.id && !s.attached)
    });
    let foreign_connection = connect(&other, &host);
    assert!(other
        .remote_terminal_sessions(foreign_connection.id, foreign_connection.generation)
        .unwrap()
        .is_empty());
    assert!(other
        .close_remote_terminal_session(
            foreign_connection.id,
            foreign_connection.generation,
            &retained.id
        )
        .is_err());
    let foreign = other
        .attach_controller_service(
            foreign_connection.id,
            foreign_connection.generation,
            ServiceKind::Terminal,
        )
        .unwrap();
    assert!(matches!(
        other.open_terminal_service_with_options(
            foreign.id,
            foreign.generation,
            TerminalOptions {
                size: SIZE,
                persistent: true,
                resume_id: Some(retained.id.clone())
            }
        ),
        Err(Error::Unauthorized)
    ));
    assert_eq!(host.hosted_terminal_sessions().unwrap()[0].id, retained.id);
    host.authorize_service(&owner.endpoint_id(), ServiceKind::Terminal, false)
        .unwrap();
    wait(|| host.hosted_terminal_sessions().unwrap().is_empty());
    assert_eq!(
        other
            .controller_session(foreign_connection.id)
            .unwrap()
            .phase,
        SessionPhase::Active
    );
}

#[test]
fn persistent_terminal_close_bypasses_stalled_input() {
    use crate::terminal::TerminalOptions;
    let controller = start_direct();
    let host = start_direct();
    pair_for_control(&host, &controller);
    host.set_terminal_factory(Some(native())).unwrap();
    host.authorize_service(&controller.endpoint_id(), ServiceKind::Terminal, true)
        .unwrap();
    let connection = connect(&controller, &host);
    let owner = controller
        .attach_controller_service(connection.id, connection.generation, ServiceKind::Terminal)
        .unwrap();
    let terminal = controller
        .open_terminal_service_with_options(
            owner.id,
            owner.generation,
            TerminalOptions {
                size: SIZE,
                persistent: true,
                resume_id: None,
            },
        )
        .unwrap();
    terminal
        .input(b"stty -echo; printf 'SLEEP_%s\\n' STARTED; sleep 60\n")
        .unwrap();
    let mut output = Vec::new();
    wait(|| {
        while let Some(TerminalEvent::Output(bytes)) = terminal.poll() {
            output.extend(bytes);
        }
        String::from_utf8_lossy(&output).contains("SLEEP_STARTED")
    });
    for _ in 0..128 {
        let _ = terminal.input(&vec![b'x'; crate::terminal::TERMINAL_CHUNK]);
    }
    terminal.close();
    wait(|| host.hosted_terminal_sessions().unwrap().is_empty());
    assert_eq!(
        controller.controller_session(connection.id).unwrap().phase,
        SessionPhase::Active
    );
}

#[test]
fn delayed_startup_cancel_releases_backend() {
    use std::sync::Mutex;

    struct BlockingFactory {
        entered: std::sync::mpsc::SyncSender<()>,
        release: Mutex<std::sync::mpsc::Receiver<()>>,
        opened: std::sync::atomic::AtomicBool,
        finished: std::sync::mpsc::SyncSender<()>,
    }
    impl HostTerminalFactory for BlockingFactory {
        fn open(
            &self,
            size: TerminalSize,
            startup: crate::BackendStartup,
        ) -> Result<crate::terminal::TerminalBackend, Error> {
            self.opened.store(true, std::sync::atomic::Ordering::SeqCst);
            let _ = self.entered.try_send(());
            // Cancellation must reach the factory without a test release.
            let result = startup.receive(&self.release.lock().unwrap());
            self.finished.send(()).unwrap();
            result?;
            crate::terminal::NativeTerminalFactory::new("/bin/sh".into(), std::env::temp_dir())
                .unwrap()
                .open(size, startup)
        }
    }

    let controller = start_direct();
    let host = start_direct();
    pair_for_control(&host, &controller);
    host.authorize_service(&controller.endpoint_id(), ServiceKind::Terminal, true)
        .unwrap();
    let (entered_tx, entered_rx) = std::sync::mpsc::sync_channel(1);
    let (_release_tx, release_rx) = std::sync::mpsc::channel();
    let (finished, done) = std::sync::mpsc::sync_channel(1);
    let factory = Arc::new(BlockingFactory {
        entered: entered_tx,
        release: Mutex::new(release_rx),
        opened: std::sync::atomic::AtomicBool::new(false),
        finished,
    });
    host.set_terminal_factory(Some(factory.clone())).unwrap();
    let parent = connect(&controller, &host);
    let owner = controller
        .attach_controller_service(parent.id, parent.generation, ServiceKind::Terminal)
        .unwrap();

    let result = std::thread::scope(|s| {
        let handle = s.spawn(|| controller.open_terminal_service(owner.id, owner.generation, SIZE));
        entered_rx.recv_timeout(Duration::from_secs(8)).unwrap();
        assert!(factory.opened.load(std::sync::atomic::Ordering::SeqCst));
        host.authorize_service(&controller.endpoint_id(), ServiceKind::Terminal, false)
            .unwrap();
        done.recv_timeout(Duration::from_secs(1)).unwrap();
        handle.join().unwrap()
    });
    assert!(
        result.is_err(),
        "cancelled startup must never admit a terminal"
    );
    wait(|| {
        host.hosted_services().unwrap().is_empty()
            && host
                .hosted_terminal_sessions()
                .unwrap()
                .iter()
                .all(|s| !s.attached)
    });
    assert_eq!(
        controller.controller_session(parent.id).unwrap().phase,
        SessionPhase::Active
    );
}

#[test]
fn terminal_capacity_counts_pending_startup_and_keeps_peers_independent() {
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Mutex,
    };
    struct Factory {
        calls: AtomicUsize,
        entered: std::sync::mpsc::SyncSender<()>,
        release: Mutex<std::sync::mpsc::Receiver<()>>,
    }
    impl HostTerminalFactory for Factory {
        fn open(
            &self,
            size: TerminalSize,
            startup: crate::BackendStartup,
        ) -> Result<crate::terminal::TerminalBackend, Error> {
            if self.calls.fetch_add(1, Ordering::SeqCst) == 0 {
                self.entered.send(()).unwrap();
                startup.receive(&self.release.lock().unwrap())?;
            }
            native().open(size, startup)
        }
    }
    let host = start_direct();
    let a = start_direct();
    let b = start_direct();
    for peer in [&a, &b] {
        pair_for_control(&host, peer);
        host.authorize_service(&peer.endpoint_id(), ServiceKind::Terminal, true)
            .unwrap();
    }
    let policy = crate::terminal::TerminalPolicy {
        max_sessions: 2,
        max_sessions_per_peer: 1,
        ..Default::default()
    };
    host.set_terminal_policy(policy).unwrap();
    let (entered, opened) = std::sync::mpsc::sync_channel(1);
    let (release, released) = std::sync::mpsc::channel();
    let factory = Arc::new(Factory {
        calls: AtomicUsize::new(0),
        entered,
        release: Mutex::new(released),
    });
    host.set_terminal_factory(Some(factory.clone())).unwrap();
    let ca = connect(&a, &host);
    let cb = connect(&b, &host);
    let attach = |peer: &Core, parent: &crate::SessionInfo| {
        peer.attach_controller_service(parent.id, parent.generation, ServiceKind::Terminal)
            .unwrap()
    };
    let first = attach(&a, &ca);
    let (ta, tb) = std::thread::scope(|scope| {
        let opening = scope.spawn(|| a.open_terminal_service(first.id, first.generation, SIZE));
        opened.recv_timeout(Duration::from_secs(3)).unwrap();
        assert!(matches!(host.set_terminal_policy(policy), Err(Error::Busy)));
        let duplicate = attach(&a, &ca);
        assert!(matches!(
            a.open_terminal_service(duplicate.id, duplicate.generation, SIZE),
            Err(Error::Busy)
        ));
        assert_eq!(
            factory.calls.load(Ordering::SeqCst),
            1,
            "refusal must precede native startup"
        );
        let sibling = attach(&b, &cb);
        let tb = b
            .open_terminal_service(sibling.id, sibling.generation, SIZE)
            .unwrap();
        assert_eq!(factory.calls.load(Ordering::SeqCst), 2);
        release.send(()).unwrap();
        (opening.join().unwrap().unwrap(), tb)
    });
    assert_eq!(host.hosted_terminal_sessions().unwrap().len(), 2);
    ta.close();
    wait(|| host.hosted_terminal_sessions().unwrap().len() == 1);
    assert!(!tb.is_cancelled());
    tb.close();
    wait(|| host.hosted_terminal_sessions().unwrap().is_empty());
    let denied = crate::terminal::TerminalPolicy {
        allow_persistence: false,
        ..policy
    };
    host.set_terminal_policy(denied).unwrap();
    let owner = attach(&a, &ca);
    assert!(matches!(
        a.open_terminal_service_with_options(
            owner.id,
            owner.generation,
            crate::terminal::TerminalOptions {
                size: SIZE,
                persistent: true,
                resume_id: None,
            }
        ),
        Err(Error::Unauthorized)
    ));
    assert_eq!(factory.calls.load(Ordering::SeqCst), 2);
}
