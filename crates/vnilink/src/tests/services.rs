use super::filesystem_location::FilesystemLocation;
use super::support::{pair_for_control, start_direct, RecordingDesktop};
use crate::camera::{CameraBackend, CameraConfig, CameraDevice, CameraSource, HostCameraFactory};
use crate::terminal::TerminalSize;
use crate::{
    CloseCause, Core, CoreEvent, Error, Grants, ServiceInfo, ServiceKind, ServicePhase,
    SessionInfo, SessionPhase,
};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use std::time::{Duration, Instant};

fn wait(condition: impl FnMut() -> bool) {
    wait_named("service condition", condition);
}

fn wait_named(name: &str, mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(8);
    while !condition() {
        assert!(Instant::now() < deadline, "{name} timed out");
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
                phase: ServicePhase::Closed(CloseCause::OwnerClose),
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
        ServicePhase::Closed(CloseCause::OwnerClose)
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
    host.unpair(&controller.endpoint_id()).unwrap();
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
        ServicePhase::Closed(CloseCause::OwnerClose)
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
    host.unpair(&controller.endpoint_id()).unwrap();
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
    host.set_desktop_sharing(Grants {
        transfer: true,
        ..Grants::default()
    })
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
    host.enable_filesystem_access().unwrap();
    let location = FilesystemLocation::new(host.file_roots().unwrap(), &scratch.0);
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
    host.set_desktop_sharing(Grants {
        view: true,
        control: true,
        transfer: true,
        clipboard: false,
    })
    .unwrap();
    let files = controller
        .open_files_service(owner.id, owner.generation)
        .unwrap();
    assert!(matches!(
        controller.open_files_service(owner.id, owner.generation),
        Err(Error::Busy)
    ));
    files
        .mkdir(location.root, &location.path(""), "original")
        .unwrap();
    let listing = files
        .list(location.root, &location.path(""), 0, 20)
        .unwrap();
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
            location.root,
            &location.path(""),
            "original",
            Some(listing.entries[0].token),
            "renamed",
        )
        .unwrap();
    assert!(scratch.0.join("renamed").is_dir());
    assert!(!scratch.0.join("original").exists());
    // A retained service pipe must not consume/block legacy filesystem streams.
    let legacy = controller
        .fs_list(
            session.id,
            session.generation,
            location.root,
            &location.path(""),
            0,
            20,
        )
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
    assert!(matches!(
        files.list(location.root, &location.path(""), 0, 20),
        Err(Error::NoSession)
    ));
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
    host.set_service_sharing(ServiceKind::Terminal, true)
        .unwrap();
    host.set_service_sharing(ServiceKind::Camera, true).unwrap();
    host.set_service_sharing(ServiceKind::Camera, false)
        .unwrap();
    assert!(!revoked.is_cancelled());
    assert_eq!(
        revoked
            .list(location.root, &location.path(""), 0, 20)
            .unwrap()
            .entries
            .len(),
        1
    );
    host.set_service_sharing(ServiceKind::Files, false).unwrap();
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
    host.set_service_sharing(ServiceKind::Terminal, true)
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
    host.set_service_sharing(ServiceKind::Camera, true).unwrap();
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
    host.set_desktop_sharing(Grants {
        view: true,
        control: true,
        transfer: true,
        clipboard: false,
    })
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
    host.enable_filesystem_access().unwrap();
    let location = FilesystemLocation::new(host.file_roots().unwrap(), &scratch.0);
    pair_for_control(&host, &controller);
    host.set_desktop_sharing(Grants::default()).unwrap();
    host.set_video_codecs(crate::VideoCodecCaps::default())
        .unwrap();
    controller
        .set_video_codecs(crate::VideoCodecCaps::default())
        .unwrap();
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
            crate::InputEvent::pointer(1, 1, 1, 1)
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
    host.set_service_sharing(ServiceKind::Files, true).unwrap();
    let pipe = controller
        .open_files_service(owner.id, owner.generation)
        .unwrap();
    pipe.mkdir(location.root, &location.path(""), "owned")
        .unwrap();
    assert_eq!(
        pipe.list(location.root, &location.path(""), 0, 20)
            .unwrap()
            .entries[0]
            .name,
        "owned"
    );
    // Even with Transfer, legacy mutations cannot bypass service admission.
    assert!(controller
        .fs_mkdir(
            session.id,
            session.generation,
            location.root,
            &location.path(""),
            "legacy"
        )
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
    assert_eq!(
        next.list(location.root, &location.path(""), 0, 20)
            .unwrap()
            .entries[0]
            .name,
        "owned"
    );
    assert!(controller
        .controller_session(session.id)
        .unwrap()
        .quality
        .is_none());
    host.set_service_sharing(ServiceKind::Files, false).unwrap();
    wait(|| next.is_cancelled());
    assert_eq!(
        controller.controller_session(session.id).unwrap().phase,
        SessionPhase::Active
    );
}

#[test]
fn neutral_pairing_needs_host_consent_and_does_not_enable_features() {
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
    wait(|| host.control_request().unwrap().is_some());
    host.confirm_control_request(&controller.control_check())
        .unwrap();
    active(&controller, pending.id);
    assert_eq!(host.snapshot().unwrap().devices.len(), 1);
    assert_eq!(
        host.sharing_policy().unwrap(),
        crate::SharingPolicy::default()
    );
    assert!(events.try_recv().is_some());
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
        host.enable_filesystem_access().unwrap();
        let location = FilesystemLocation::new(host.file_roots().unwrap(), &root.0);
        let applied = Arc::new(std::sync::Mutex::new(Vec::new()));
        host.set_desktop(Box::new(super::support::RecordingDesktop {
            events: applied.clone(),
        }))
        .unwrap();
        pair_for_control(&host, &controller);
        host.set_service_sharing(ServiceKind::Files, true).unwrap();
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
                .set_service_sharing(ServiceKind::Desktop, false)
                .unwrap(),
            _ => drop(handle),
        }
        let expected = match close {
            2 | 3 => ServicePhase::Closed(CloseCause::PeerClose),
            _ => ServicePhase::Closed(CloseCause::OwnerClose),
        };
        wait(|| controller.controller_service(desktop.id).unwrap().phase == expected);
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
        pipe.mkdir(location.root, &location.path(""), "after-desktop-close")
            .unwrap();
        assert_eq!(
            pipe.list(location.root, &location.path(""), 0, 20)
                .unwrap()
                .entries[0]
                .name,
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
        // The redial preserves the original close cause: first close wins.
        assert_eq!(
            controller.controller_service(desktop.id).unwrap().phase,
            expected
        );
        let files = controller.controller_service(files.id).unwrap();
        let fresh_pipe = controller
            .open_files_service(files.id, files.generation)
            .unwrap();
        pipe.close();
        assert!(!fresh_pipe.is_cancelled());
        fresh_pipe
            .mkdir(location.root, &location.path(""), "after-reconnect")
            .unwrap();
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

struct TestCapture;
impl crate::HostCapture for TestCapture {
    fn monitor(&self) -> (u16, u16) {
        (1920, 1080)
    }
}

struct ScratchDir(std::path::PathBuf);
impl Drop for ScratchDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn scratch_dir(name: &str) -> ScratchDir {
    let dir = std::env::temp_dir().join(format!(
        "vnilink-services-{}-{}-{}",
        name,
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    ScratchDir(dir)
}

#[derive(Default)]
struct TestCameraFactory {
    sources: Mutex<Vec<Arc<CameraSource>>>,
}

impl HostCameraFactory for TestCameraFactory {
    fn devices(&self) -> Result<Vec<CameraDevice>, Error> {
        Ok(vec![CameraDevice {
            id: "approved-camera".into(),
            name: "Test camera".into(),
        }])
    }

    fn open(
        &self,
        requested: CameraConfig,
        startup: crate::BackendStartup,
    ) -> Result<CameraBackend, Error> {
        startup.check()?;
        let source = CameraSource::new();
        self.sources.lock().unwrap().push(source.clone());
        CameraBackend::new(requested, source)
    }
}

fn test_camera_config() -> CameraConfig {
    CameraConfig {
        device_id: "approved-camera".into(),
        width: 640,
        height: 480,
        fps: 30,
    }
}

const TEST_TERMINAL_SIZE: TerminalSize = TerminalSize {
    rows: 24,
    columns: 80,
};

#[cfg(unix)]
fn test_terminal_factory() -> Arc<crate::terminal::NativeTerminalFactory> {
    Arc::new(
        crate::terminal::NativeTerminalFactory::new("/bin/sh".into(), std::env::temp_dir())
            .unwrap(),
    )
}

fn connect_services(controller: &Core, host: &Core) -> SessionInfo {
    let pending = controller
        .begin_connect_services(&host.endpoint_id())
        .unwrap();
    wait(|| controller.controller_session(pending.id).unwrap().phase == SessionPhase::Active);
    controller.controller_session(pending.id).unwrap()
}

#[test]
#[cfg(unix)]
fn all_services_share_one_parent_and_close_independently() {
    let controller = start_direct();
    let host = start_direct();
    let _root = scratch_dir("all");
    host.enable_filesystem_access().unwrap();
    host.set_desktop(Box::new(RecordingDesktop {
        events: Arc::new(Mutex::new(Vec::new())),
    }))
    .unwrap();
    host.set_capture(Box::new(TestCapture)).unwrap();
    host.set_terminal_factory(Some(test_terminal_factory()))
        .unwrap();
    let camera_factory = Arc::new(TestCameraFactory::default());
    host.set_camera_factory(Some(camera_factory.clone()))
        .unwrap();
    pair_for_control(&host, &controller);
    for kind in [
        ServiceKind::Desktop,
        ServiceKind::Files,
        ServiceKind::Terminal,
        ServiceKind::Camera,
    ] {
        host.set_service_sharing(kind, true).unwrap();
    }

    let parent = connect_services(&controller, &host);

    let desktop_owner = controller
        .attach_controller_service(parent.id, parent.generation, ServiceKind::Desktop)
        .unwrap();
    let desktop = controller
        .open_desktop_service(desktop_owner.id, desktop_owner.generation)
        .unwrap();

    let files_owner = controller
        .attach_controller_service(parent.id, parent.generation, ServiceKind::Files)
        .unwrap();
    let files = controller
        .open_files_service(files_owner.id, files_owner.generation)
        .unwrap();

    let terminal_owner = controller
        .attach_controller_service(parent.id, parent.generation, ServiceKind::Terminal)
        .unwrap();
    let terminal = controller
        .open_terminal_service(
            terminal_owner.id,
            terminal_owner.generation,
            TEST_TERMINAL_SIZE,
        )
        .unwrap();

    let camera_owner = controller
        .attach_controller_service(parent.id, parent.generation, ServiceKind::Camera)
        .unwrap();
    let camera = controller
        .open_camera_service(
            camera_owner.id,
            camera_owner.generation,
            test_camera_config(),
        )
        .unwrap();

    // All four owners share one parent; duplicate opens are Busy.
    assert_eq!(controller.controller_services().unwrap().len(), 4);
    assert_eq!(host.hosted_services().unwrap().len(), 4);
    assert!(matches!(
        controller.open_desktop_service(desktop_owner.id, desktop_owner.generation),
        Err(Error::Busy)
    ));
    assert!(matches!(
        controller.open_files_service(files_owner.id, files_owner.generation),
        Err(Error::Busy)
    ));
    assert!(matches!(
        controller.open_terminal_service(
            terminal_owner.id,
            terminal_owner.generation,
            TEST_TERMINAL_SIZE
        ),
        Err(Error::Busy)
    ));
    assert!(matches!(
        controller.open_camera_service(
            camera_owner.id,
            camera_owner.generation,
            test_camera_config()
        ),
        Err(Error::Busy)
    ));

    // Close Desktop first: parent and siblings survive, backend tears down.
    desktop.close();
    wait(|| {
        host.hosted_services()
            .unwrap()
            .iter()
            .all(|s| s.kind != ServiceKind::Desktop)
    });
    assert_eq!(host.hosted_services().unwrap().len(), 3);
    assert_eq!(
        controller
            .controller_service(desktop_owner.id)
            .unwrap()
            .phase,
        ServicePhase::Closed(CloseCause::OwnerClose)
    );
    assert_eq!(
        controller.controller_session(parent.id).unwrap().phase,
        SessionPhase::Active
    );
    assert!(!files.is_cancelled());
    assert!(!terminal.is_cancelled());
    assert!(!camera.is_cancelled());
    // A closed Desktop handle cannot act on a replacement.
    assert!(desktop.subscribe_video().is_err());

    // Close Files: terminal and camera leases survive.
    files.close();
    wait(|| {
        host.hosted_services()
            .unwrap()
            .iter()
            .all(|s| s.kind != ServiceKind::Files)
    });
    assert_eq!(host.hosted_services().unwrap().len(), 2);
    assert!(!terminal.is_cancelled());
    assert!(!camera.is_cancelled());
    assert_eq!(
        controller.controller_session(parent.id).unwrap().phase,
        SessionPhase::Active
    );

    // Close Terminal: camera survives, shell is reaped.
    terminal.close();
    wait(|| {
        host.hosted_services()
            .unwrap()
            .iter()
            .all(|s| s.kind != ServiceKind::Terminal)
    });
    assert_eq!(host.hosted_services().unwrap().len(), 1);
    assert!(!camera.is_cancelled());

    // Close Camera: parent stays Active with no hosted backends.
    camera.close();
    wait(|| host.hosted_services().unwrap().is_empty());
    assert_eq!(
        controller.controller_session(parent.id).unwrap().phase,
        SessionPhase::Active
    );
    assert_eq!(controller.controller_services().unwrap().len(), 4);
    for service in controller.controller_services().unwrap() {
        assert_eq!(service.phase, ServicePhase::Closed(CloseCause::OwnerClose));
    }
}

#[test]
#[cfg(unix)]
fn files_and_terminal_start_without_capture_or_desktop_decoder() {
    let controller = start_direct();
    let host = start_direct();
    let root = scratch_dir("nocapture");
    host.enable_filesystem_access().unwrap();
    let location = FilesystemLocation::new(host.file_roots().unwrap(), &root.0);
    host.set_terminal_factory(Some(test_terminal_factory()))
        .unwrap();
    // No capture, no desktop adapter, no H.264 encoder on the host.
    host.set_video_codecs(crate::VideoCodecCaps::default())
        .unwrap();
    pair_for_control(&host, &controller);
    host.set_service_sharing(ServiceKind::Files, true).unwrap();
    host.set_service_sharing(ServiceKind::Terminal, true)
        .unwrap();
    host.set_service_sharing(ServiceKind::Camera, true).unwrap();
    let camera_factory = Arc::new(TestCameraFactory::default());
    host.set_camera_factory(Some(camera_factory)).unwrap();

    let parent = connect_services(&controller, &host);
    assert_eq!((parent.monitor_width, parent.monitor_height), (0, 0));
    assert!(parent.quality.is_none());

    // Files and Terminal open without any display/camera resource.
    let files_owner = controller
        .attach_controller_service(parent.id, parent.generation, ServiceKind::Files)
        .unwrap();
    let files = controller
        .open_files_service(files_owner.id, files_owner.generation)
        .unwrap();
    files
        .mkdir(location.root, &location.path(""), "no-capture")
        .unwrap();
    assert!(root.0.join("no-capture").is_dir());

    let terminal_owner = controller
        .attach_controller_service(parent.id, parent.generation, ServiceKind::Terminal)
        .unwrap();
    let terminal = controller
        .open_terminal_service(
            terminal_owner.id,
            terminal_owner.generation,
            TEST_TERMINAL_SIZE,
        )
        .unwrap();
    assert!(!terminal.is_cancelled());

    // Camera opens without any display resource either.
    let camera_owner = controller
        .attach_controller_service(parent.id, parent.generation, ServiceKind::Camera)
        .unwrap();
    let camera = controller
        .open_camera_service(
            camera_owner.id,
            camera_owner.generation,
            test_camera_config(),
        )
        .unwrap();
    assert!(!camera.is_cancelled());

    // Desktop still requires a real backend: no capture means Unsupported,
    // not a silent Files/Terminal failure.
    let desktop_owner = controller
        .attach_controller_service(parent.id, parent.generation, ServiceKind::Desktop)
        .unwrap();
    assert!(matches!(
        controller.open_desktop_service(desktop_owner.id, desktop_owner.generation),
        Err(Error::Unsupported(_))
    ));
    assert!(host
        .hosted_services()
        .unwrap()
        .iter()
        .all(|s| s.kind != ServiceKind::Desktop));
    assert_eq!(
        controller.controller_session(parent.id).unwrap().phase,
        SessionPhase::Active
    );
    assert!(!files.is_cancelled());
}

#[test]
fn concurrent_neutral_connects_reuse_parent_without_replacing_service_owners() {
    let host = start_direct();
    let controller = Arc::new(start_direct());
    pair_for_control(&host, &controller);
    let barrier = Arc::new(std::sync::Barrier::new(8));
    let workers: Vec<_> = (0..8)
        .map(|_| {
            let controller = controller.clone();
            let barrier = barrier.clone();
            let peer = host.endpoint_id();
            std::thread::spawn(move || {
                barrier.wait();
                controller.begin_connect_services(&peer).unwrap().id
            })
        })
        .collect();
    let ids: Vec<_> = workers.into_iter().map(|w| w.join().unwrap()).collect();
    assert!(ids.iter().all(|id| *id == ids[0]));
    assert_eq!(controller.controller_sessions().unwrap().len(), 1);
    let session = active(&controller, ids[0]);
    let files = controller
        .attach_controller_service(session.id, session.generation, ServiceKind::Files)
        .unwrap();
    let lease = controller
        .service_lease(files.id, files.generation)
        .unwrap();
    let reused = controller
        .begin_connect_services(&host.endpoint_id())
        .unwrap();
    assert_eq!(reused, session);
    assert!(!lease.is_cancelled());
    assert_eq!(controller.controller_service(files.id).unwrap(), files);
    assert_eq!(host.hosted_sessions().unwrap().len(), 1);
}

#[test]
fn every_close_names_its_cause_on_the_controller_side() {
    let controller = start_direct();
    let host_a = start_direct();
    let host_b = start_direct();
    // Redial leg: the record follows the parent across the redial.
    pair_for_control(&host_a, &controller);
    let pending = controller.begin_connect(&host_a.endpoint_id()).unwrap();
    let session = active(&controller, pending.id);
    // The record adopts the new epoch at establish, so a later parent
    // close names ParentLoss. Replaced covers only the pre-adoption race
    // window, which no deterministic sequence hits.
    let follower = controller
        .attach_controller_service(session.id, session.generation, ServiceKind::Files)
        .unwrap();
    controller.drop_controller_link(session.id).unwrap();
    wait(|| {
        let next = controller.controller_session(session.id).unwrap();
        next.phase == SessionPhase::Active && next.generation > session.generation
    });
    controller.disconnect_session(session.id).unwrap();
    wait(|| controller.controller_session(session.id).unwrap().phase == SessionPhase::Closed);
    let followed = controller.controller_service(follower.id).unwrap();
    assert!(followed.generation > session.generation);
    assert_eq!(followed.phase, ServicePhase::Closed(CloseCause::ParentLoss));
    // Parent-loss leg: the record shares the closing parent's epoch.
    pair_for_control(&host_b, &controller);
    let pending = controller.begin_connect(&host_b.endpoint_id()).unwrap();
    let session = active(&controller, pending.id);
    let orphaned = desktop(&controller, &session);
    controller.disconnect_session(session.id).unwrap();
    wait(|| controller.controller_session(session.id).unwrap().phase == SessionPhase::Closed);
    assert_eq!(
        controller.controller_service(orphaned.id).unwrap().phase,
        ServicePhase::Closed(CloseCause::ParentLoss)
    );
    // Shutdown names itself on the event stream; reads are gone after it.
    let live = start_direct();
    let last_host = start_direct();
    pair_for_control(&last_host, &live);
    let pending = live.begin_connect(&last_host.endpoint_id()).unwrap();
    let session = active(&live, pending.id);
    let owned = live
        .attach_controller_service(session.id, session.generation, ServiceKind::Files)
        .unwrap();
    let events = live.subscribe().unwrap();
    live.shutdown();
    let mut saw_shutdown = false;
    while let Some(event) = events.try_recv() {
        if let CoreEvent::ServiceChanged { service } = event {
            if service.id == owned.id {
                assert_eq!(service.phase, ServicePhase::Closed(CloseCause::Shutdown));
                saw_shutdown = true;
            }
        }
    }
    assert!(saw_shutdown);
    controller.shutdown();
    host_a.shutdown();
    host_b.shutdown();
    last_host.shutdown();
}

#[test]
fn revoke_and_idle_expiry_name_their_causes() {
    let controller = start_direct();
    let host = start_direct();
    let root = scratch_dir("cause-files");
    std::fs::create_dir_all(&root.0).unwrap();
    host.enable_filesystem_access().unwrap();
    host.set_desktop(Box::new(RecordingDesktop {
        events: Arc::new(Mutex::new(Vec::new())),
    }))
    .unwrap();
    pair_for_control(&host, &controller);
    host.set_service_sharing(ServiceKind::Files, true).unwrap();
    let parent = connect_services(&controller, &host);
    let files_owner = controller
        .attach_controller_service(parent.id, parent.generation, ServiceKind::Files)
        .unwrap();
    let _pipe = controller
        .open_files_service(files_owner.id, files_owner.generation)
        .unwrap();
    wait_named("hosted files active", || {
        host.hosted_services()
            .unwrap()
            .iter()
            .any(|s| s.kind == ServiceKind::Files && s.phase == ServicePhase::Active)
    });
    // The revoke event proves the cause: the serve task's guard may remove
    // the record before any poll observes the finished phase.
    let host_events = host.subscribe().unwrap();
    host.set_service_sharing(ServiceKind::Files, false).unwrap();
    let mut saw_revoked = false;
    let deadline = Instant::now() + Duration::from_secs(8);
    while !saw_revoked && Instant::now() < deadline {
        while let Some(event) = host_events.try_recv() {
            if let CoreEvent::HostedServiceChanged { service } = event {
                if service.kind == ServiceKind::Files
                    && service.phase == ServicePhase::Closed(CloseCause::Revoked)
                {
                    saw_revoked = true;
                }
            }
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(saw_revoked, "host never published the Files revoke close");
    wait(|| {
        host.hosted_services()
            .unwrap()
            .iter()
            .all(|s| s.kind != ServiceKind::Files || s.phase.finished())
    });

    // An idle desktop with no input expires while the parent stays Active.
    let desktop_owner = controller
        .attach_controller_service(parent.id, parent.generation, ServiceKind::Desktop)
        .unwrap();
    let _desktop = controller
        .open_desktop_service(desktop_owner.id, desktop_owner.generation)
        .unwrap();
    controller
        .set_power_policy(crate::power::PowerPolicy {
            outgoing_idle_secs: 1,
            ..Default::default()
        })
        .unwrap();
    wait_named("desktop idle expiry", || {
        controller
            .controller_service(desktop_owner.id)
            .unwrap()
            .phase
            .finished()
    });
    assert_eq!(
        controller
            .controller_service(desktop_owner.id)
            .unwrap()
            .phase,
        ServicePhase::Closed(CloseCause::IdleExpired)
    );
    assert_eq!(
        controller.controller_session(parent.id).unwrap().phase,
        SessionPhase::Active
    );
    controller.shutdown();
    host.shutdown();
}

#[test]
#[cfg(unix)]
fn removing_a_backend_device_closes_its_hosted_services() {
    let controller = start_direct();
    let host = start_direct();
    host.set_terminal_factory(Some(test_terminal_factory()))
        .unwrap();
    pair_for_control(&host, &controller);
    host.set_service_sharing(ServiceKind::Terminal, true)
        .unwrap();
    let parent = connect_services(&controller, &host);
    let owner = controller
        .attach_controller_service(parent.id, parent.generation, ServiceKind::Terminal)
        .unwrap();
    let _terminal = controller
        .open_terminal_service(owner.id, owner.generation, TEST_TERMINAL_SIZE)
        .unwrap();
    wait(|| {
        host.hosted_services()
            .unwrap()
            .iter()
            .any(|s| s.kind == ServiceKind::Terminal && s.phase == ServicePhase::Active)
    });
    // The event proves the cause: the serve task's guard may remove the
    // record before any read observes the finished phase.
    let host_events = host.subscribe().unwrap();
    host.set_terminal_factory(None).unwrap();
    let mut saw_device_loss = false;
    let deadline = Instant::now() + Duration::from_secs(8);
    while !saw_device_loss && Instant::now() < deadline {
        while let Some(event) = host_events.try_recv() {
            if let CoreEvent::HostedServiceChanged { service } = event {
                if service.kind == ServiceKind::Terminal
                    && service.phase == ServicePhase::Closed(CloseCause::DeviceLoss)
                {
                    saw_device_loss = true;
                }
            }
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(
        saw_device_loss,
        "host never published the Terminal device-loss close"
    );
    controller.shutdown();
    host.shutdown();
}

#[test]
fn stalled_native_camera_open_cancels_cleanly_and_adopts_nothing_late() {
    use std::sync::atomic::AtomicUsize;
    struct Stall {
        entered: std::sync::mpsc::Sender<()>,
        release: Mutex<std::sync::mpsc::Receiver<()>>,
        late: AtomicUsize,
    }
    impl HostCameraFactory for Stall {
        fn devices(&self) -> Result<Vec<CameraDevice>, Error> {
            Ok(vec![CameraDevice {
                id: "approved-camera".into(),
                name: "Stall".into(),
            }])
        }
        fn open(
            &self,
            requested: CameraConfig,
            _startup: crate::BackendStartup,
        ) -> Result<CameraBackend, Error> {
            // Hostile backend: never polls `startup`, so only the engine's
            // own select branches can settle the open.
            self.entered.send(()).unwrap();
            self.release.lock().unwrap().recv().unwrap();
            self.late.fetch_add(1, Ordering::SeqCst);
            CameraBackend::new(requested, CameraSource::new())
        }
    }
    let controller = start_direct();
    let host = start_direct();
    let (entered, entering) = std::sync::mpsc::channel();
    let (release, released) = std::sync::mpsc::channel();
    let stall = Arc::new(Stall {
        entered,
        release: Mutex::new(released),
        late: AtomicUsize::new(0),
    });
    host.set_camera_factory(Some(stall.clone())).unwrap();
    pair_for_control(&host, &controller);
    host.set_service_sharing(ServiceKind::Camera, true).unwrap();
    let parent = connect_services(&controller, &host);
    let owner = controller
        .attach_controller_service(parent.id, parent.generation, ServiceKind::Camera)
        .unwrap();
    let (elapsed, outcome) = std::thread::scope(|scope| {
        let opener = scope.spawn(|| {
            controller.open_camera_service(owner.id, owner.generation, test_camera_config())
        });
        entering.recv_timeout(Duration::from_secs(5)).unwrap();
        let started = Instant::now();
        controller
            .close_controller_service(owner.id, owner.generation)
            .unwrap();
        (started.elapsed(), opener.join().unwrap())
    });
    // Cancellation settles the open; the 5s exchange timeout never fires.
    assert!(elapsed < Duration::from_secs(4));
    assert!(outcome.is_err());
    assert_eq!(
        controller.controller_service(owner.id).unwrap().phase,
        ServicePhase::Closed(CloseCause::OwnerClose)
    );
    // The late backend completes into a dropped spawn: counted, never
    // admitted, and the host record stays finished without an Active leak.
    release.send(()).unwrap();
    wait(|| stall.late.load(Ordering::SeqCst) == 1);
    wait(|| {
        host.hosted_services()
            .unwrap()
            .iter()
            .filter(|s| s.kind == ServiceKind::Camera)
            .all(|s| s.phase.finished())
    });
    assert!(host
        .hosted_services()
        .unwrap()
        .iter()
        .filter(|s| s.kind == ServiceKind::Camera)
        .all(|s| s.phase != ServicePhase::Active));
    controller.shutdown();
    host.shutdown();
}

#[test]
#[cfg(unix)]
fn files_only_and_terminal_only_connections_never_request_capture() {
    let controller = start_direct();
    let host = start_direct();
    // Capture and camera backends exist, so an empty target list proves the
    // core never asked — not that no backend was registered.
    host.set_capture(Box::new(TestCapture)).unwrap();
    let cameras = Arc::new(TestCameraFactory::default());
    host.set_camera_factory(Some(cameras.clone())).unwrap();
    host.set_terminal_factory(Some(test_terminal_factory()))
        .unwrap();
    pair_for_control(&host, &controller);
    host.set_desktop_sharing(Grants {
        transfer: true,
        ..Grants::default()
    })
    .unwrap();

    // Files-only leg: the session carries jobs, never video.
    let files = active(
        &controller,
        controller
            .begin_connect_files(&host.endpoint_id())
            .unwrap()
            .id,
    );
    assert!(host.video_targets().unwrap().is_empty());
    assert!(matches!(
        controller.subscribe_session_video(files.id, files.generation),
        Err(Error::Unsupported(_))
    ));
    assert!(host
        .hosted_services()
        .unwrap()
        .iter()
        .all(|service| service.kind != ServiceKind::Desktop));
    controller.disconnect_session(files.id).unwrap();
    wait(|| controller.controller_session(files.id).unwrap().phase == SessionPhase::Closed);

    // Terminal-only leg: a live shell with output, still no capture target.
    host.set_service_sharing(ServiceKind::Terminal, true)
        .unwrap();
    let parent = connect_services(&controller, &host);
    let owner = controller
        .attach_controller_service(parent.id, parent.generation, ServiceKind::Terminal)
        .unwrap();
    let terminal = controller
        .open_terminal_service(owner.id, owner.generation, TEST_TERMINAL_SIZE)
        .unwrap();
    terminal.input(b"echo capture-never-requested\n").unwrap();
    let mut transcript = Vec::new();
    wait(|| {
        while let Some(event) = terminal.poll() {
            if let crate::terminal::TerminalEvent::Output(bytes) = event {
                transcript.extend_from_slice(&bytes);
            }
        }
        transcript
            .windows(b"capture-never-requested".len())
            .any(|window| window == b"capture-never-requested")
    });
    assert!(host.video_targets().unwrap().is_empty());
    assert!(host
        .hosted_services()
        .unwrap()
        .iter()
        .filter(|service| service.kind == ServiceKind::Desktop)
        .all(|service| service.phase != ServicePhase::Active));
    assert!(cameras.sources.lock().unwrap().is_empty());
    terminal.close();
    controller.shutdown();
    host.shutdown();
}

#[test]
fn global_file_revocation_closes_both_peers_without_stopping_desktops_or_reopening_old_owners() {
    let host = start_direct();
    let a = start_direct();
    let b = start_direct();
    host.enable_filesystem_access().unwrap();
    host.set_capture(Box::new(TestCapture)).unwrap();
    for peer in [&a, &b] {
        pair_for_control(&host, peer);
    }
    host.set_service_sharing(ServiceKind::Files, true).unwrap();
    let mut connections = Vec::new();
    for peer in [&a, &b] {
        let parent = connect_services(peer, &host);
        let screen = peer
            .attach_controller_service(parent.id, parent.generation, ServiceKind::Desktop)
            .unwrap();
        let desktop = peer
            .open_desktop_service(screen.id, screen.generation)
            .unwrap();
        let owner = peer
            .attach_controller_service(parent.id, parent.generation, ServiceKind::Files)
            .unwrap();
        let files = peer.open_files_service(owner.id, owner.generation).unwrap();
        connections.push((peer, parent, owner, files, desktop));
    }
    let events = host.subscribe().unwrap();
    host.set_service_sharing(ServiceKind::Files, false).unwrap();
    let mut revoked = std::collections::BTreeSet::new();
    while let Some(event) = events.try_recv() {
        if let CoreEvent::HostedServiceChanged { service } = event {
            if service.kind == ServiceKind::Files
                && service.phase == ServicePhase::Closed(CloseCause::Revoked)
            {
                revoked.insert(service.peer_id);
            }
        }
    }
    assert_eq!(
        revoked,
        [a.endpoint_id(), b.endpoint_id()].into_iter().collect()
    );
    for (peer, parent, owner, files, desktop) in &connections {
        wait(|| files.is_cancelled());
        assert_eq!(
            peer.controller_service(owner.id).unwrap().phase,
            ServicePhase::Closed(CloseCause::PeerClose)
        );
        assert!(files.is_cancelled());
        assert_eq!(
            peer.controller_session(parent.id).unwrap().phase,
            SessionPhase::Active
        );
        assert!(desktop.desktop_info().is_ok());
    }
    host.set_service_sharing(ServiceKind::Files, true).unwrap();
    for (peer, parent, owner, files, _) in &connections {
        assert!(
            files.is_cancelled(),
            "enabling must never revive an old file handle"
        );
        assert!(matches!(
            peer.open_files_service(owner.id, owner.generation),
            Err(Error::NoSession)
        ));
        let replacement = peer
            .attach_controller_service(parent.id, parent.generation, ServiceKind::Files)
            .unwrap();
        assert!(peer
            .open_files_service(replacement.id, replacement.generation)
            .is_ok());
    }
}
