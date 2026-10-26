//! Files beside a Desktop on one connection: real operations through its
//! pipe, and the host closing or withdrawing it, which ends Files alone and
//! never revives an old owner.

use expect_test::expect;
use std::collections::BTreeSet;

use vnilink::{CloseCause, Error, Grants, ServiceKind, ServicePhase, SessionPhase};
use vnilink_testkit::desktop::MonitorCapture;
use vnilink_testkit::{traced, wait_for, Pair, TestResult, DEFAULT_WAIT};

use super::support::{hosted_closes, phase, whole_disk};

/// A Files pipe beside a Desktop: refused until the host grants Transfer,
/// opened once, carrying real operations without taking the connection's
/// own file calls; the host closing it, its owner closing it, and the host
/// withdrawing Files each end it alone, while changing other services'
/// sharing leaves it open.
#[test]
fn a_files_pipe_carries_real_operations_and_closes_without_its_siblings() -> TestResult {
    let trace = traced();
    let pair = Pair::for_desktop();
    let (host, controller) = (&pair.host.core, &pair.controller.core);
    let (root, folder) = whole_disk(&pair.host)?;
    let session = pair.connect_control();
    let screen = pair.attach(&session, ServiceKind::Desktop);
    let screen_work = controller.service_lease(screen.id, screen.generation)?;
    let owner = pair.attach(&session, ServiceKind::Files);
    let support = controller.remote_service_support(session.id, session.generation)?;
    assert!(
        support.files && support.desktop && !support.terminal && !support.camera,
        "the host offers Desktop and Files: {support:?}"
    );
    assert!(
        matches!(
            controller.open_files_service(owner.id, owner.generation),
            Err(Error::TransferDenied)
        ),
        "Files needs Transfer"
    );
    assert!(
        pair.host.hosted(None).is_empty(),
        "a refused pipe is not served"
    );
    host.set_desktop_sharing(Grants {
        view: true,
        control: true,
        transfer: true,
        clipboard: false,
    })?;
    let files = controller.open_files_service(owner.id, owner.generation)?;
    assert!(
        matches!(
            controller.open_files_service(owner.id, owner.generation),
            Err(Error::Busy)
        ),
        "a Files owner opens one pipe"
    );
    assert_eq!(
        files.places()?,
        host.places()?,
        "the pipe shows the host's places"
    );
    files.mkdir(root, &folder, "original")?;
    let listing = files.list(root, &folder, 0, 20)?;
    assert_eq!(
        listing
            .entries
            .iter()
            .map(|entry| entry.name.as_str())
            .collect::<Vec<_>>(),
        vec!["original"],
        "the pipe lists what it made"
    );
    files.rename(
        root,
        &folder,
        "original",
        Some(listing.entries[0].token),
        "renamed",
    )?;
    assert!(
        pair.host.root().join("renamed").is_dir(),
        "the rename reached the host's disk"
    );
    assert!(
        !pair.host.root().join("original").exists(),
        "the old name is gone"
    );
    // A retained service pipe must not take or block the connection's own
    // file streams.
    let own = controller.fs_list(session.id, session.generation, root, &folder, 0, 20)?;
    assert_eq!(
        own.entries
            .iter()
            .map(|entry| entry.name.as_str())
            .collect::<Vec<_>>(),
        vec!["renamed"],
        "the connection's own listing works beside the pipe"
    );
    #[cfg(unix)]
    assert_eq!(
        own.entries[0].token.ino, listing.entries[0].token.ino,
        "both name the same object"
    );
    let hosted = pair.host.hosted(None);
    assert_eq!(hosted.len(), 1, "the host serves the pipe alone");
    host.close_hosted_service(hosted[0].id, hosted[0].generation)?;
    wait_for("the host's close to end the pipe", DEFAULT_WAIT, || {
        files.is_cancelled().then_some(())
    });
    pair.host.wait_released(None);
    assert!(
        !screen_work.is_cancelled(),
        "the host closing Files leaves the Desktop"
    );
    assert_eq!(
        controller.controller_session(session.id)?,
        session,
        "the host closing Files leaves the connection"
    );
    assert!(
        matches!(files.list(root, &folder, 0, 20), Err(Error::NoSession)),
        "a closed pipe refuses work"
    );
    let next = pair.attach(&session, ServiceKind::Files);
    let reopened = controller.open_files_service(next.id, next.generation)?;
    reopened.close();
    wait_for("the owner's close to end the pipe", DEFAULT_WAIT, || {
        (reopened.is_cancelled() && pair.host.hosted(None).is_empty()).then_some(())
    });
    assert!(
        !screen_work.is_cancelled(),
        "the owner closing Files leaves the Desktop"
    );
    let next = pair.attach(&session, ServiceKind::Files);
    let revoked = controller.open_files_service(next.id, next.generation)?;
    host.set_service_sharing(ServiceKind::Terminal, true)?;
    host.set_service_sharing(ServiceKind::Camera, true)?;
    host.set_service_sharing(ServiceKind::Camera, false)?;
    assert!(
        !revoked.is_cancelled(),
        "changing other services' sharing leaves Files"
    );
    assert_eq!(
        revoked.list(root, &folder, 0, 20)?.entries.len(),
        1,
        "the pipe still works"
    );
    host.set_service_sharing(ServiceKind::Files, false)?;
    let permissions = host.service_permissions(&pair.controller.endpoint())?;
    assert!(
        permissions.desktop && permissions.terminal,
        "withdrawing Files leaves the other grants: {permissions:?}"
    );
    assert!(
        !permissions.files && !permissions.camera,
        "Files and Camera are withdrawn: {permissions:?}"
    );
    wait_for("withdrawing Files to end the pipe", DEFAULT_WAIT, || {
        (revoked.is_cancelled() && pair.host.hosted(None).is_empty()).then_some(())
    });
    trace.wait_for_records("service released", 3);
    assert_eq!(
        controller.controller_session(session.id)?,
        session,
        "withdrawing Files leaves the connection"
    );
    assert!(
        !screen_work.is_cancelled(),
        "withdrawing Files leaves the Desktop"
    );
    expect![[r#"
        controller:
          service: service phase from=active generation=1 kind=Files service=2 to=closed: PeerClose
          service: service phase from=active generation=1 kind=Files service=3 to=closed: OwnerClose
          service: service phase from=active generation=1 kind=Files service=4 to=closed: PeerClose
        host:
          hosting: service refused connection=1 kind=Files status=not permitted
          hosting: service opened connection=1 kind=Some(Files) service=1
          hosting: service refused connection=1 kind=Files status=busy
          service: service phase from=active generation=1 kind=Files service=1 to=closed: OwnerClose
          hosting: service released service=1
          hosting: service opened connection=1 kind=Some(Files) service=2
          hosting: service closed kind=Files phase=closed: PeerClose service=2
          hosting: service released service=2
          hosting: service opened connection=1 kind=Some(Files) service=3
          service: service phase from=active generation=1 kind=Files service=3 to=closed: Revoked
          hosting: service released service=3
    "#]]
    .assert_eq(&trace.story(&["service", "hosting"]));
    Ok(())
}

/// Withdrawing Files closes it for every viewer as revoked while their
/// Desktops and connections stay; sharing Files again revives no old pipe
/// or owner, and a new owner opens.
#[test]
fn withdrawing_files_closes_it_for_every_viewer_and_sharing_again_revives_nothing() -> TestResult {
    let trace = traced();
    let pair = Pair::for_desktop();
    let (host, a) = (&pair.host, &pair.controller);
    let b = pair.add_viewer("viewer");
    whole_disk(host)?;
    host.core
        .set_capture(Box::new(MonitorCapture((1920, 1080))))?;
    host.core.set_service_sharing(ServiceKind::Files, true)?;
    let mut connections = Vec::new();
    for viewer in [a, &b] {
        let parent = viewer.connect_services(host)?;
        let desktop = viewer.open_desktop(&parent)?;
        let (owner, files) = viewer.open_files(&parent)?;
        connections.push((viewer, parent, owner, files, desktop));
    }
    let events = host.core.subscribe()?;
    host.core.set_service_sharing(ServiceKind::Files, false)?;
    let revoked: BTreeSet<String> = hosted_closes(&events, ServiceKind::Files, CloseCause::Revoked)
        .into_iter()
        .map(|service| service.peer_id)
        .collect();
    assert_eq!(
        revoked,
        [a.endpoint(), b.endpoint()].into_iter().collect(),
        "withdrawing Files revokes it for every viewer as it returns"
    );
    for (viewer, parent, owner, files, desktop) in &connections {
        wait_for("the viewer's pipe to end", DEFAULT_WAIT, || {
            files.is_cancelled().then_some(())
        });
        assert_eq!(
            phase(viewer, owner),
            ServicePhase::Closed(CloseCause::PeerClose),
            "the viewer's Files closes as the peer's close"
        );
        assert_eq!(
            viewer.core.controller_session(parent.id)?.phase,
            SessionPhase::Active,
            "the viewer's connection stays"
        );
        assert!(desktop.desktop_info().is_ok(), "the viewer's Desktop stays");
    }
    trace.wait_for_records("service released", 2);
    host.core.set_service_sharing(ServiceKind::Files, true)?;
    let mut replacements = Vec::new();
    for (viewer, parent, owner, files, _) in &connections {
        assert!(
            files.is_cancelled(),
            "sharing again never revives an old pipe"
        );
        assert!(
            matches!(
                viewer.core.open_files_service(owner.id, owner.generation),
                Err(Error::NoSession)
            ),
            "sharing again never reopens an old owner"
        );
        let replacement = viewer.core.attach_controller_service(
            parent.id,
            parent.generation,
            ServiceKind::Files,
        )?;
        replacements.push(
            viewer
                .core
                .open_files_service(replacement.id, replacement.generation)
                .map_err(|error| {
                    format!("a new owner opens once Files is shared again: {error}")
                })?,
        );
    }
    // The two viewers' pipes end and open side by side.
    expect![[r#"
        controller:
          service: service phase from=active generation=1 kind=Files service=2 to=closed: PeerClose
        host:
          hosting: service opened connection=1 kind=Some(Files) service=2
          hosting: service opened connection=2 kind=Some(Files) service=4
          service: service phase from=active generation=1 kind=Files service=2 to=closed: Revoked
          service: service phase from=active generation=2 kind=Files service=4 to=closed: Revoked
          hosting: service released service=2
          hosting: service released service=4
          hosting: service opened connection=1 kind=Some(Files) service=5
          hosting: service opened connection=2 kind=Some(Files) service=6
        viewer:
          service: service phase from=active generation=2 kind=Files service=2 to=closed: PeerClose
    "#]]
    .assert_eq(&trace.concurrent_story(&["service", "hosting"]));
    Ok(())
}
