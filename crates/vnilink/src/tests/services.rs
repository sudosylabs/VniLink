use super::support::{pair_for_control, start_direct};
use crate::{
    Core, CoreEvent, Error, Grants, ServiceInfo, ServiceKind, ServicePhase, SessionInfo,
    SessionPhase,
};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::{Duration, Instant};

fn wait(mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(8);
    while !condition() {
        assert!(Instant::now() < deadline, "service condition timed out");
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn active(core: &Core, id: u64) -> SessionInfo {
    wait(|| core.controller_session(id).unwrap().phase == SessionPhase::Active);
    core.controller_session(id).unwrap()
}

fn desktop(core: &Core, session: &SessionInfo) -> ServiceInfo {
    core.attach_controller_service(session.id, session.generation, ServiceKind::Desktop)
        .unwrap()
}

#[test]
fn sibling_services_share_transport_but_not_cancellation() {
    let controller = start_direct();
    let host = start_direct();
    pair_for_control(&host, &controller);
    let session = active(
        &controller,
        controller.begin_connect(&host.endpoint_id()).unwrap().id,
    );
    let events = controller.subscribe().unwrap();
    let screen = desktop(&controller, &session);
    let files = controller
        .attach_controller_service(session.id, session.generation, ServiceKind::Files)
        .unwrap();
    let mut screen_work = controller
        .service_lease(screen.id, screen.generation)
        .unwrap();
    let files_work = controller
        .service_lease(files.id, files.generation)
        .unwrap();
    assert_eq!(
        controller
            .service_connection(files.id, files.generation)
            .unwrap(),
        session
    );
    assert!(matches!(
        controller.attach_controller_service(session.id, session.generation, ServiceKind::Desktop),
        Err(Error::Busy)
    ));
    assert!(matches!(
        controller.forget_controller_service(screen.id),
        Err(Error::Busy)
    ));

    // A backend awaiting I/O drops its owned resources when its service closes.
    struct Resource(Arc<AtomicBool>);
    impl Drop for Resource {
        fn drop(&mut self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }
    let dropped = Arc::new(AtomicBool::new(false));
    let resource = Resource(dropped.clone());
    let (ready_tx, ready_rx) = std::sync::mpsc::channel();
    let worker = std::thread::spawn(move || {
        tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap()
            .block_on(screen_work.run(async move {
                let _resource = resource;
                ready_tx.send(()).unwrap();
                std::future::pending::<Result<(), Error>>().await
            }))
    });
    ready_rx.recv_timeout(Duration::from_secs(2)).unwrap();
    controller
        .close_controller_service(screen.id, screen.generation)
        .unwrap();
    assert!(matches!(worker.join().unwrap(), Err(Error::NoSession)));
    assert!(dropped.load(Ordering::SeqCst));
    assert!(!files_work.is_cancelled());
    assert_eq!(controller.controller_session(session.id).unwrap(), session);
    assert_eq!(
        host.snapshot().unwrap().incoming.unwrap().phase,
        SessionPhase::Active
    );
    assert!(matches!(
        controller.service_connection(screen.id, screen.generation),
        Err(Error::NoSession)
    ));
    controller
        .close_controller_service(screen.id, screen.generation)
        .unwrap();
    let replacement = desktop(&controller, &session);
    assert_ne!(replacement.id, screen.id);
    controller.forget_controller_service(screen.id).unwrap();
    assert!(matches!(
        controller.controller_service(screen.id),
        Err(Error::NoSession)
    ));

    let mut changes = Vec::new();
    while let Some(event) = events.try_recv() {
        if let CoreEvent::ServiceChanged { service } = event {
            changes.push(service);
        }
    }
    assert_eq!(
        changes,
        vec![
            screen.clone(),
            files.clone(),
            ServiceInfo {
                phase: ServicePhase::Closed,
                ..screen
            },
            replacement
        ]
    );
    controller.disconnect_session(session.id).unwrap();
    assert!(
        files_work.is_cancelled(),
        "disconnect must cancel immediately, without polling"
    );
}

#[test]
fn reconnect_never_revives_old_leases_or_closed_services() {
    let controller = start_direct();
    let a = start_direct();
    let b = start_direct();
    pair_for_control(&a, &controller);
    pair_for_control(&b, &controller);
    let sa = active(
        &controller,
        controller.begin_connect(&a.endpoint_id()).unwrap().id,
    );
    let sb = active(
        &controller,
        controller.begin_connect(&b.endpoint_id()).unwrap().id,
    );
    let screen = desktop(&controller, &sa);
    let other = desktop(&controller, &sb);
    let files = controller
        .attach_controller_service(sa.id, sa.generation, ServiceKind::Files)
        .unwrap();
    let old = controller
        .service_lease(screen.id, screen.generation)
        .unwrap();
    let untouched = controller
        .service_lease(other.id, other.generation)
        .unwrap();
    controller
        .close_controller_service(files.id, files.generation)
        .unwrap();
    controller.drop_controller_link(sa.id).unwrap();
    wait(|| old.is_cancelled());
    wait(|| {
        let next = controller.controller_session(sa.id).unwrap();
        next.phase == SessionPhase::Active && next.generation > sa.generation
    });
    let next = controller.controller_service(screen.id).unwrap();
    assert_eq!(next.id, screen.id);
    assert_eq!(next.phase, ServicePhase::Active);
    assert!(next.generation > screen.generation);
    assert!(old.is_cancelled());
    assert!(!untouched.is_cancelled());
    assert_eq!(controller.controller_service(other.id).unwrap(), other);
    assert_eq!(
        controller.controller_service(files.id).unwrap().phase,
        ServicePhase::Closed
    );
    assert!(matches!(
        controller.service_lease(screen.id, screen.generation),
        Err(Error::StaleSession)
    ));
    assert!(matches!(
        controller.close_controller_service(screen.id, screen.generation),
        Err(Error::StaleSession)
    ));
    let fresh = controller.service_lease(next.id, next.generation).unwrap();
    assert!(!fresh.is_cancelled());
    controller.unpair(&a.endpoint_id()).unwrap();
    assert!(fresh.is_cancelled());
    assert!(!untouched.is_cancelled());
    controller.shutdown();
    assert!(untouched.is_cancelled());
}

#[test]
fn pending_owner_can_close_without_cancelling_parent_approval() {
    let controller = start_direct();
    let host = start_direct();
    pair_for_control(&host, &controller);
    host.authorize(&controller.endpoint_id(), Grants::default())
        .unwrap();
    host.export_pairing_file(Duration::from_secs(60), true)
        .unwrap();
    let pending = controller.begin_connect(&host.endpoint_id()).unwrap();
    wait(|| host.control_request().unwrap().is_some());
    let service = desktop(&controller, &pending);
    assert_eq!(service.phase, ServicePhase::Waiting);
    let files = controller
        .attach_controller_service(pending.id, pending.generation, ServiceKind::Files)
        .unwrap();
    assert!(matches!(
        controller.service_lease(service.id, service.generation),
        Err(Error::NoSession)
    ));
    controller
        .close_controller_service(service.id, service.generation)
        .unwrap();
    assert!(host.control_request().unwrap().is_some());
    host.confirm_control_request(&controller.control_check())
        .unwrap();
    let session = active(&controller, pending.id);
    assert_eq!(
        controller.controller_service(service.id).unwrap().phase,
        ServicePhase::Closed
    );
    let waiting = desktop(&controller, &session);
    assert_eq!(waiting.phase, ServicePhase::Active);
    let files = controller.controller_service(files.id).unwrap();
    assert_eq!(files.phase, ServicePhase::Active);
    assert_eq!(files.generation, session.generation);
    assert!(!controller
        .service_lease(files.id, files.generation)
        .unwrap()
        .is_cancelled());
}

#[test]
fn rejected_admission_fails_attached_services() {
    let controller = start_direct();
    let host = start_direct();
    pair_for_control(&host, &controller);
    host.authorize(&controller.endpoint_id(), Grants::default())
        .unwrap();
    host.export_pairing_file(Duration::from_secs(60), true)
        .unwrap();
    let pending = controller.begin_connect(&host.endpoint_id()).unwrap();
    wait(|| host.control_request().unwrap().is_some());
    let service = desktop(&controller, &pending);
    host.reject_control_request().unwrap();
    wait(|| {
        matches!(
            controller.controller_session(pending.id).unwrap().phase,
            SessionPhase::Failed(_)
        )
    });
    assert!(matches!(
        controller.controller_service(service.id).unwrap().phase,
        ServicePhase::Failed(_)
    ));
    assert!(matches!(
        controller.service_lease(service.id, service.generation),
        Err(Error::NoSession)
    ));
    controller.forget_controller_session(pending.id).unwrap();
    controller.forget_controller_service(service.id).unwrap();
    assert!(controller.controller_services().unwrap().is_empty());
}

#[test]
fn unsupported_kinds_and_record_limits_do_not_change_connection() {
    let controller = start_direct();
    let host = start_direct();
    pair_for_control(&host, &controller);
    host.authorize(
        &controller.endpoint_id(),
        Grants {
            transfer: true,
            ..Grants::default()
        },
    )
    .unwrap();
    let session = active(
        &controller,
        controller
            .begin_connect_files(&host.endpoint_id())
            .unwrap()
            .id,
    );
    let support = controller.controller_service_support(session.id).unwrap();
    assert!(!support.desktop && support.files && !support.terminal && !support.camera);
    for kind in [
        ServiceKind::Desktop,
        ServiceKind::Terminal,
        ServiceKind::Camera,
    ] {
        assert!(matches!(
            controller.attach_controller_service(session.id, session.generation, kind),
            Err(Error::Unsupported(_))
        ));
    }
    assert!(controller.controller_services().unwrap().is_empty());
    for _ in 0..crate::MAX_SERVICE_RECORDS {
        let service = controller
            .attach_controller_service(session.id, session.generation, ServiceKind::Files)
            .unwrap();
        controller
            .close_controller_service(service.id, service.generation)
            .unwrap();
    }
    assert!(matches!(
        controller.attach_controller_service(session.id, session.generation, ServiceKind::Files),
        Err(Error::Busy)
    ));
    controller.forget_controller_service(1).unwrap();
    let service = controller
        .attach_controller_service(session.id, session.generation, ServiceKind::Files)
        .unwrap();
    assert!(service.id > crate::MAX_SERVICE_RECORDS as u64);
    assert_eq!(controller.controller_session(session.id).unwrap(), session);
}

#[test]
fn remote_files_pipe_routes_real_operations_and_closes_independently() {
    struct Scratch(std::path::PathBuf);
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    let scratch = Scratch(std::env::temp_dir().join(format!(
            "vnilink-service-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        )));
    std::fs::create_dir(&scratch.0).unwrap();
    let controller = start_direct();
    let host = start_direct();
    host.set_file_roots(vec![scratch.0.clone()]).unwrap();
    pair_for_control(&host, &controller);
    let session = active(
        &controller,
        controller.begin_connect(&host.endpoint_id()).unwrap().id,
    );
    let screen = desktop(&controller, &session);
    let screen_work = controller
        .service_lease(screen.id, screen.generation)
        .unwrap();
    let owner = controller
        .attach_controller_service(session.id, session.generation, ServiceKind::Files)
        .unwrap();
    let support = controller
        .remote_service_support(session.id, session.generation)
        .unwrap();
    assert!(support.files && support.desktop && !support.terminal && !support.camera);
    assert!(matches!(
        controller.open_files_service(owner.id, owner.generation),
        Err(Error::TransferDenied)
    ));
    assert!(host.hosted_services().unwrap().is_empty());
    host.authorize(
        &controller.endpoint_id(),
        Grants {
            control: true,
            transfer: true,
            clipboard: false,
        },
    )
    .unwrap();
    let files = controller
        .open_files_service(owner.id, owner.generation)
        .unwrap();
    assert!(matches!(
        controller.open_files_service(owner.id, owner.generation),
        Err(Error::Busy)
    ));
    let root = host.file_roots().unwrap()[0].0;
    files.mkdir(root, "", "original").unwrap();
    let listing = files.list(root, "", 0, 20).unwrap();
    assert_eq!(
        listing
            .entries
            .iter()
            .map(|e| e.name.as_str())
            .collect::<Vec<_>>(),
        vec!["original"]
    );
    files
        .rename(
            root,
            "",
            "original",
            Some(listing.entries[0].token),
            "renamed",
        )
        .unwrap();
    assert!(scratch.0.join("renamed").is_dir());
    assert!(!scratch.0.join("original").exists());
    // A retained service pipe must not consume/block legacy filesystem streams.
    let legacy = controller
        .fs_list(session.id, session.generation, root, "", 0, 20)
        .unwrap();
    assert_eq!(
        legacy
            .entries
            .iter()
            .map(|e| e.name.as_str())
            .collect::<Vec<_>>(),
        vec!["renamed"]
    );
    #[cfg(unix)]
    {
        assert_eq!(legacy.entries[0].token.ino, listing.entries[0].token.ino);
    }
    let hosted = host.hosted_services().unwrap();
    assert_eq!(hosted.len(), 1);
    host.close_hosted_service(hosted[0].id, hosted[0].generation)
        .unwrap();
    wait(|| files.is_cancelled());
    wait(|| host.hosted_services().unwrap().is_empty());
    assert!(!screen_work.is_cancelled());
    assert_eq!(controller.controller_session(session.id).unwrap(), session);
    assert!(matches!(files.list(root, "", 0, 20), Err(Error::NoSession)));
    let next = controller
        .attach_controller_service(session.id, session.generation, ServiceKind::Files)
        .unwrap();
    let reopened = controller
        .open_files_service(next.id, next.generation)
        .unwrap();
    reopened.close();
    wait(|| reopened.is_cancelled() && host.hosted_services().unwrap().is_empty());
    assert!(!screen_work.is_cancelled());
    let next = controller
        .attach_controller_service(session.id, session.generation, ServiceKind::Files)
        .unwrap();
    let revoked = controller
        .open_files_service(next.id, next.generation)
        .unwrap();
    host.authorize_service(&controller.endpoint_id(), ServiceKind::Terminal, true)
        .unwrap();
    host.authorize_service(&controller.endpoint_id(), ServiceKind::Camera, true)
        .unwrap();
    host.authorize_service(&controller.endpoint_id(), ServiceKind::Camera, false)
        .unwrap();
    assert!(!revoked.is_cancelled());
    assert_eq!(revoked.list(root, "", 0, 20).unwrap().entries.len(), 1);
    host.authorize_service(&controller.endpoint_id(), ServiceKind::Files, false)
        .unwrap();
    let permissions = host.service_permissions(&controller.endpoint_id()).unwrap();
    assert!(permissions.desktop && permissions.terminal);
    assert!(!permissions.files && !permissions.camera);
    wait(|| revoked.is_cancelled() && host.hosted_services().unwrap().is_empty());
    assert_eq!(controller.controller_session(session.id).unwrap(), session);
    assert!(!screen_work.is_cancelled());
}

#[test]
fn remote_service_negotiation_rejects_incompatible_or_stale_requests() {
    let controller = start_direct();
    let host = start_direct();
    pair_for_control(&host, &controller);
    let session = active(
        &controller,
        controller.begin_connect(&host.endpoint_id()).unwrap().id,
    );
    let incoming = host.snapshot().unwrap().incoming.unwrap();
    let header = |version: u8, generation: u64, kind: u8| {
        let mut bytes = vec![8, version, 1];
        bytes.extend_from_slice(&incoming.id.to_be_bytes());
        bytes.extend_from_slice(&generation.to_be_bytes());
        bytes.extend_from_slice(&123u64.to_be_bytes());
        bytes.push(kind);
        bytes
    };
    for (bytes, expected) in [
        (header(2, incoming.generation, 2), 5),
        (header(1, incoming.generation + 1, 2), 3),
        (header(1, incoming.generation, 3), 6),
        (header(1, incoming.generation, 4), 6),
        (header(1, incoming.generation, 255), 1),
        (header(1, incoming.generation, 2), 2),
    ] {
        assert_eq!(
            controller
                .test_service_frame(session.id, session.generation, bytes)
                .unwrap(),
            vec![1, expected, 0]
        );
        assert!(host.hosted_services().unwrap().is_empty());
        assert_eq!(controller.controller_session(session.id).unwrap(), session);
    }
    // Permission and backend availability are distinct. An explicit grant
    // changes Denied to Unsupported without creating a shell/camera backend.
    host.authorize_service(&controller.endpoint_id(), ServiceKind::Terminal, true)
        .unwrap();
    assert_eq!(
        controller
            .test_service_frame(
                session.id,
                session.generation,
                header(1, incoming.generation, 3)
            )
            .unwrap(),
        vec![1, 1, 0]
    );
    assert_eq!(
        controller
            .test_service_frame(
                session.id,
                session.generation,
                header(1, incoming.generation, 4)
            )
            .unwrap(),
        vec![1, 6, 0]
    );
    assert_eq!(
        controller
            .test_service_frame(
                session.id,
                session.generation,
                header(1, incoming.generation, 2)
            )
            .unwrap(),
        vec![1, 2, 0]
    );
    host.authorize_service(&controller.endpoint_id(), ServiceKind::Camera, true)
        .unwrap();
    assert_eq!(
        controller
            .test_service_frame(
                session.id,
                session.generation,
                header(1, incoming.generation, 4)
            )
            .unwrap(),
        vec![1, 1, 0]
    );
    assert!(host.hosted_services().unwrap().is_empty());
    assert!(controller
        .test_service_frame(session.id, session.generation, vec![8, 1])
        .is_err());
    assert!(
        controller
            .remote_service_support(session.id, session.generation)
            .unwrap()
            .files
    );
}

#[test]
fn remote_files_pipe_reconnect_preserves_owner_and_fences_old_backend() {
    let controller = start_direct();
    let host = start_direct();
    pair_for_control(&host, &controller);
    host.authorize(
        &controller.endpoint_id(),
        Grants {
            control: true,
            transfer: true,
            clipboard: false,
        },
    )
    .unwrap();
    let session = active(
        &controller,
        controller.begin_connect(&host.endpoint_id()).unwrap().id,
    );
    let owner = controller
        .attach_controller_service(session.id, session.generation, ServiceKind::Files)
        .unwrap();
    let old = controller
        .open_files_service(owner.id, owner.generation)
        .unwrap();
    controller.drop_controller_link(session.id).unwrap();
    wait(|| old.is_cancelled());
    wait(|| {
        let next = controller.controller_session(session.id).unwrap();
        next.phase == SessionPhase::Active && next.generation > session.generation
    });
    let next = controller.controller_service(owner.id).unwrap();
    assert_eq!(next.phase, ServicePhase::Active);
    assert!(next.generation > owner.generation);
    assert!(matches!(
        controller.open_files_service(owner.id, owner.generation),
        Err(Error::StaleSession)
    ));
    let fresh = controller
        .open_files_service(next.id, next.generation)
        .unwrap();
    old.close();
    drop(old);
    assert!(!fresh.is_cancelled());
    fresh.close();
    wait(|| host.hosted_services().unwrap().is_empty());
    assert_eq!(
        controller.controller_session(session.id).unwrap().phase,
        SessionPhase::Active
    );
}

#[test]
fn neutral_connection_admits_without_feature_grants_and_gates_real_files() {
    struct Scratch(std::path::PathBuf);
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    let scratch = Scratch(std::env::temp_dir().join(format!(
            "vnilink-neutral-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        )));
    std::fs::create_dir(&scratch.0).unwrap();
    let host = start_direct();
    let controller = start_direct();
    host.set_file_roots(vec![scratch.0.clone()]).unwrap();
    pair_for_control(&host, &controller);
    host.authorize(&controller.endpoint_id(), Grants::default())
        .unwrap();
    host.set_h264_support(false, false).unwrap();
    controller.set_h264_support(false, false).unwrap();
    let session = active(
        &controller,
        controller
            .begin_connect_services(&host.endpoint_id())
            .unwrap()
            .id,
    );
    let incoming = host.snapshot().unwrap().incoming.unwrap();
    assert_eq!((session.monitor_width, session.monitor_height), (0, 0));
    assert_eq!((incoming.monitor_width, incoming.monitor_height), (0, 0));
    assert!(session.quality.is_none() && incoming.quality.is_none());
    assert!(matches!(
        controller.submit_input(
            session.id,
            session.generation,
            crate::InputEvent::Pointer { x: 1, y: 1 }
        ),
        Err(Error::Unsupported(_))
    ));
    assert!(matches!(
        controller.subscribe_session_video(session.id, session.generation),
        Err(Error::Unsupported(_))
    ));
    let desktop = controller
        .attach_controller_service(session.id, session.generation, ServiceKind::Desktop)
        .unwrap();
    assert!(matches!(
        controller.open_desktop_service(desktop.id, desktop.generation),
        Err(Error::Unsupported(_))
    ));
    let owner = controller
        .attach_controller_service(session.id, session.generation, ServiceKind::Files)
        .unwrap();
    assert!(
        controller
            .remote_service_support(session.id, session.generation)
            .unwrap()
            .files
    );
    assert!(matches!(
        controller.open_files_service(owner.id, owner.generation),
        Err(Error::TransferDenied)
    ));
    assert!(host.hosted_services().unwrap().is_empty());
    host.authorize_service(&controller.endpoint_id(), ServiceKind::Files, true)
        .unwrap();
    let pipe = controller
        .open_files_service(owner.id, owner.generation)
        .unwrap();
    pipe.mkdir(0, "", "owned").unwrap();
    assert_eq!(pipe.list(0, "", 0, 20).unwrap().entries[0].name, "owned");
    // Even with Transfer, legacy mutations cannot bypass service admission.
    assert!(controller
        .fs_mkdir(session.id, session.generation, 0, "", "legacy")
        .is_err());
    assert!(!scratch.0.join("legacy").exists());
    let old_lease = controller
        .service_lease(owner.id, owner.generation)
        .unwrap();
    controller.drop_controller_link(session.id).unwrap();
    wait(|| {
        controller.controller_session(session.id).unwrap().phase == SessionPhase::Active
            && controller
                .controller_session(session.id)
                .unwrap()
                .generation
                > session.generation
    });
    assert!(old_lease.is_cancelled());
    let fresh = controller.controller_service(owner.id).unwrap();
    let next = controller
        .open_files_service(fresh.id, fresh.generation)
        .unwrap();
    pipe.close();
    assert_eq!(next.list(0, "", 0, 20).unwrap().entries[0].name, "owned");
    assert!(controller
        .controller_session(session.id)
        .unwrap()
        .quality
        .is_none());
    host.authorize_service(&controller.endpoint_id(), ServiceKind::Files, false)
        .unwrap();
    wait(|| next.is_cancelled());
    assert_eq!(
        controller.controller_session(session.id).unwrap().phase,
        SessionPhase::Active
    );
}

#[test]
fn neutral_connection_does_not_ask_an_unpaired_host_for_control_approval() {
    let host = start_direct();
    let controller = start_direct();
    host.start_hosting().unwrap();
    // Only the controller knows the host; the host has not accepted its identity.
    controller
        .accept_invitation(&host.create_invitation(Duration::from_secs(60)).unwrap())
        .unwrap();
    host.export_pairing_file(Duration::from_secs(60), true)
        .unwrap();
    let events = host.subscribe().unwrap();
    let pending = controller
        .begin_connect_services(&host.endpoint_id())
        .unwrap();
    wait(|| {
        matches!(
            controller.controller_session(pending.id).unwrap().phase,
            SessionPhase::Failed(_)
        )
    });
    assert!(host.snapshot().unwrap().incoming.is_none());
    while let Some(event) = events.try_recv() {
        assert!(!matches!(event, CoreEvent::ControlRequested { .. }));
    }
}

#[test]
fn adopted_desktop_close_releases_input_and_preserves_files_for_every_close_path() {
    struct Scratch(std::path::PathBuf);
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    for close in 0..5 {
        let host = start_direct();
        let controller = start_direct();
        let root = Scratch(std::env::temp_dir().join(format!(
            "vnilink-desktop-files-{}-{}",
            std::process::id(),
            close
        )));
        std::fs::create_dir_all(&root.0).unwrap();
        host.set_file_roots(vec![root.0.clone()]).unwrap();
        let applied = Arc::new(std::sync::Mutex::new(Vec::new()));
        host.set_desktop(Box::new(super::support::RecordingDesktop {
            events: applied.clone(),
        }))
        .unwrap();
        pair_for_control(&host, &controller);
        host.authorize_service(&controller.endpoint_id(), ServiceKind::Files, true)
            .unwrap();
        let session = controller.connect(&host.endpoint_id()).unwrap();
        let incoming = host.snapshot().unwrap().incoming.unwrap();
        let desktop = desktop(&controller, &session);
        let handle = controller
            .open_desktop_service(desktop.id, desktop.generation)
            .unwrap();
        assert!(matches!(
            controller.open_desktop_service(desktop.id, desktop.generation),
            Err(Error::Busy)
        ));
        let files = controller
            .attach_controller_service(session.id, session.generation, ServiceKind::Files)
            .unwrap();
        let pipe = controller
            .open_files_service(files.id, files.generation)
            .unwrap();
        let subscription = controller
            .subscribe_session_video(session.id, session.generation)
            .unwrap();
        assert!(host
            .host_desktop_active(incoming.id, incoming.generation)
            .unwrap());
        host.publish_video(
            incoming.id,
            crate::EncodedFrame::h264(true, 7, vec![7]).unwrap(),
        )
        .unwrap();
        assert_eq!(
            subscription
                .recv_timeout(Duration::from_secs(5))
                .unwrap()
                .payload,
            vec![7]
        );
        let key = crate::InputEvent::key_down(crate::input::usage::A);
        let button = crate::InputEvent::button_down(crate::input::PointerButton::Left);
        controller
            .submit_input(session.id, session.generation, key)
            .unwrap();
        controller
            .submit_input(session.id, session.generation, button)
            .unwrap();
        wait(|| applied.lock().unwrap().len() == 2);
        let remote = host
            .hosted_services()
            .unwrap()
            .into_iter()
            .find(|s| s.kind == ServiceKind::Desktop)
            .unwrap();
        match close {
            0 => handle.close(),
            1 => controller
                .close_controller_service(desktop.id, desktop.generation)
                .unwrap(),
            2 => host
                .close_hosted_service(remote.id, remote.generation)
                .unwrap(),
            3 => host
                .authorize_service(&controller.endpoint_id(), ServiceKind::Desktop, false)
                .unwrap(),
            _ => drop(handle),
        }
        wait(|| controller.controller_service(desktop.id).unwrap().phase == ServicePhase::Closed);
        wait(|| applied.lock().unwrap().len() == 4);
        assert_eq!(
            *applied.lock().unwrap(),
            vec![
                key,
                button,
                crate::InputEvent::button_up(crate::input::PointerButton::Left),
                crate::InputEvent::key_up(crate::input::usage::A)
            ]
        );
        assert!(matches!(
            controller.submit_input(session.id, session.generation, key),
            Err(Error::Unsupported(_))
        ));
        assert!(matches!(
            host.publish_video(
                incoming.id,
                crate::EncodedFrame::h264(true, 8, vec![8]).unwrap()
            ),
            Err(Error::NoSession)
        ));
        assert!(subscription.try_recv().is_none());
        assert!(controller
            .controller_session(session.id)
            .unwrap()
            .quality
            .is_none());
        assert!(host.snapshot().unwrap().incoming.unwrap().quality.is_none());
        assert!(!host
            .host_desktop_active(incoming.id, incoming.generation)
            .unwrap());
        assert_eq!(
            controller.controller_session(session.id).unwrap().phase,
            SessionPhase::Active
        );
        assert_eq!(
            host.snapshot().unwrap().incoming.unwrap().phase,
            SessionPhase::Active
        );
        pipe.mkdir(0, "", "after-desktop-close").unwrap();
        assert_eq!(
            pipe.list(0, "", 0, 20).unwrap().entries[0].name,
            "after-desktop-close"
        );
        assert!(!pipe.is_cancelled());
        assert_eq!(
            controller.controller_service(files.id).unwrap().phase,
            ServicePhase::Active
        );
        assert!(
            !controller
                .remote_service_support(session.id, session.generation)
                .unwrap()
                .desktop
        );
        // Redial after Desktop close remains neutral rather than resuming capture.
        controller.drop_controller_link(session.id).unwrap();
        wait(|| {
            controller.controller_session(session.id).unwrap().phase == SessionPhase::Active
                && controller
                    .controller_session(session.id)
                    .unwrap()
                    .generation
                    > session.generation
        });
        let fresh = controller.controller_session(session.id).unwrap();
        assert!(fresh.quality.is_none());
        assert_eq!((fresh.monitor_width, fresh.monitor_height), (0, 0));
        assert_eq!(
            controller.controller_service(desktop.id).unwrap().phase,
            ServicePhase::Closed
        );
        let files = controller.controller_service(files.id).unwrap();
        let fresh_pipe = controller
            .open_files_service(files.id, files.generation)
            .unwrap();
        pipe.close();
        assert!(!fresh_pipe.is_cancelled());
        fresh_pipe.mkdir(0, "", "after-reconnect").unwrap();
    }
}

#[test]
fn adopted_desktop_reconnect_fences_old_handle_without_retiring_replacement() {
    let host = start_direct();
    let controller = start_direct();
    pair_for_control(&host, &controller);
    let session = controller.connect(&host.endpoint_id()).unwrap();
    let owner = desktop(&controller, &session);
    let old = controller
        .open_desktop_service(owner.id, owner.generation)
        .unwrap();
    controller.drop_controller_link(session.id).unwrap();
    wait(|| {
        controller
            .controller_session(session.id)
            .unwrap()
            .generation
            > session.generation
            && controller.controller_session(session.id).unwrap().phase == SessionPhase::Active
    });
    let fresh = controller.controller_service(owner.id).unwrap();
    assert_eq!(fresh.phase, ServicePhase::Active);
    let replacement = controller
        .open_desktop_service(fresh.id, fresh.generation)
        .unwrap();
    old.close();
    assert!(!replacement.is_cancelled());
    assert!(controller
        .controller_session(session.id)
        .unwrap()
        .quality
        .is_some());
    let incoming = host.snapshot().unwrap().incoming.unwrap();
    let video = controller
        .subscribe_session_video(session.id, fresh.generation)
        .unwrap();
    host.publish_video(
        incoming.id,
        crate::EncodedFrame::h264(true, 9, vec![9]).unwrap(),
    )
    .unwrap();
    assert_eq!(
        video.recv_timeout(Duration::from_secs(5)).unwrap().payload,
        vec![9]
    );
    assert!(matches!(
        controller.close_controller_service(owner.id, owner.generation),
        Err(Error::StaleSession)
    ));
    replacement.close();
    assert_eq!(
        controller.controller_session(session.id).unwrap().phase,
        SessionPhase::Active
    );
}
