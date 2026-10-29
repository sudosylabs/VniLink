//! One controller, several hosts at once: each connection has its own
//! handle and its Desktop its own channels even when both hosts number
//! theirs alike, and one failing, cancelled, reconnecting or closing leaves
//! the others.

use expect_test::expect;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use vnilink::{CoreEvent, Error, Grants, JobState, ServiceKind, SessionPhase, SessionRole};
use vnilink_testkit::desktop::{
    keyframe, next_picture, InputLog, MonitorCapture, RecordingClipboard,
};
use vnilink_testkit::files::source;
use vnilink_testkit::{traced, wait_for, Engine, Files, TestResult, DEFAULT_WAIT};

use super::support::{cursor_at, INVITATION};

/// The monitor of the desktops these scenarios install.
const MONITOR: (u16, u16) = (1920, 1080);

/// Two hosts number their connections and Desktops alike, yet the
/// controller keeps two handles whose Desktops have their own video,
/// cursor and clipboard (each desktop's own keyboard settings are
/// `desktop/control.rs`'s). Closing one leaves the other as it was, a live
/// connection cannot be forgotten, and the controller shutting down ends
/// the other.
#[test]
fn two_hosts_with_identical_wire_ids_have_independent_channels_and_cleanup() -> TestResult {
    let _trace = traced();
    let controller = Engine::start("controller");
    let a = Engine::start("host-a");
    let b = Engine::start("host-b");
    controller.core.set_cursor_receiver(true)?;
    let mut clips = Vec::new();
    for host in [&a, &b] {
        InputLog::install(host, MONITOR)?;
        let clip = Arc::new(Mutex::new(Vec::new()));
        host.core.set_clipboard(Box::new(RecordingClipboard {
            texts: Arc::clone(&clip),
        }))?;
        clips.push(clip);
        host.core.set_capture(Box::new(
            MonitorCapture::new((1920, 1080)).separate_cursor(true),
        ))?;
        host.pair_for_control(&controller)?;
        host.core.set_desktop_sharing(Grants {
            view: true,
            control: true,
            clipboard: true,
            transfer: false,
        })?;
    }
    let clipped = |index: usize| -> Vec<String> {
        clips[index]
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .clone()
    };
    let events = controller.core.subscribe()?;
    let pending_a = controller.core.begin_connect_services(&a.endpoint())?;
    let pending_b = controller.core.begin_connect_services(&b.endpoint())?;
    assert_ne!(pending_a.id, pending_b.id, "each host has its own handle");
    let sa = controller.active(pending_a.id);
    let sb = controller.active(pending_b.id);
    let ha = a.core.snapshot()?.incoming.ok_or("host a's session")?;
    let hb = b.core.snapshot()?.incoming.ok_or("host b's session")?;
    assert_eq!(
        (ha.id, ha.generation),
        (hb.id, hb.generation),
        "both hosts number their sessions alike"
    );
    assert_eq!(
        controller.core.controller_sessions()?.len(),
        2,
        "the controller holds a connection to each host"
    );
    assert!(
        controller.core.snapshot()?.outgoing.is_none(),
        "two connections are no single outgoing session"
    );
    assert!(
        matches!(controller.core.disconnect(), Err(Error::Busy)),
        "a disconnect that does not name a connection is ambiguous"
    );
    assert_eq!(
        controller.core.begin_connect_services(&a.endpoint())?.id,
        sa.id,
        "connecting to the same host again reuses its connection"
    );

    let da = controller.open_desktop(&sa)?;
    let db = controller.open_desktop(&sb)?;
    let ra = a.serving(ServiceKind::Desktop, &controller);
    let rb = b.serving(ServiceKind::Desktop, &controller);
    assert_eq!(
        (ra.id, ra.generation),
        (rb.id, rb.generation),
        "both hosts number their Desktops alike"
    );
    let va = da.subscribe_video()?;
    let vb = db.subscribe_video()?;
    assert!(
        matches!(da.subscribe_video(), Err(Error::Busy)),
        "a Desktop's video has one subscriber"
    );
    a.core
        .publish_desktop_video(ra.id, ra.generation, keyframe(11, vec![1, 1]))?;
    b.core
        .publish_desktop_video(rb.id, rb.generation, keyframe(22, vec![2, 2]))?;
    assert_eq!(
        next_picture(&va, "host a's picture").payload,
        [1, 1],
        "host a's video reaches its own Desktop"
    );
    assert_eq!(
        next_picture(&vb, "host b's picture").payload,
        [2, 2],
        "host b's video reaches its own Desktop"
    );

    a.core
        .publish_desktop_cursor(ra.id, ra.generation, cursor_at((11, 12)))?;
    b.core
        .publish_desktop_cursor(rb.id, rb.generation, cursor_at((21, 22)))?;
    let cursor = |desktop: &vnilink::RemoteDesktopService| desktop.cursor().ok().flatten();
    wait_for("both cursors to arrive", DEFAULT_WAIT, || {
        (cursor(&da).is_some() && cursor(&db).is_some()).then_some(())
    });
    assert_eq!(
        (
            cursor(&da).ok_or("a cursor")?.position,
            cursor(&db).ok_or("a cursor")?.position
        ),
        ((11, 12), (21, 22)),
        "each host's cursor reaches its own Desktop"
    );

    da.submit_clipboard("to-a")?;
    db.submit_clipboard("to-b")?;
    assert_eq!(
        (clipped(0), clipped(1)),
        (vec!["to-a".to_owned()], vec!["to-b".to_owned()]),
        "each Desktop's clipboard reaches its own host"
    );
    a.core
        .publish_desktop_clipboard(ra.id, ra.generation, "from-a")?;
    b.core
        .publish_desktop_clipboard(rb.id, rb.generation, "from-b")?;
    let clipboard =
        |desktop: &vnilink::RemoteDesktopService| desktop.clipboard_text().ok().flatten();
    wait_for("both hosts' clipboards to arrive", DEFAULT_WAIT, || {
        (clipboard(&da).is_some() && clipboard(&db).is_some()).then_some(())
    });
    assert_eq!(
        (clipboard(&da), clipboard(&db)),
        (Some("from-a".to_owned()), Some("from-b".to_owned())),
        "each host's clipboard reaches its own Desktop"
    );
    for session in [&sa, &sb] {
        assert!(
            !controller
                .core
                .controller_session_paths(session.id)?
                .is_empty(),
            "each connection reports its own paths"
        );
    }

    controller.core.disconnect_session(sa.id)?;
    controller.wait_phase(sa.id, SessionPhase::Closed);
    assert_eq!(
        controller.core.controller_session(sb.id)?,
        sb,
        "closing one connection leaves the other as it was"
    );
    assert_eq!(
        clipboard(&db).as_deref(),
        Some("from-b"),
        "closing one connection leaves the other's clipboard"
    );
    b.core
        .publish_desktop_video(rb.id, rb.generation, keyframe(33, vec![3]))?;
    assert_eq!(
        next_picture(&vb, "host b's next picture").payload,
        [3],
        "closing one connection leaves the other's video"
    );
    controller.core.forget_controller_session(sa.id)?;
    assert!(
        matches!(
            controller.core.controller_session(sa.id),
            Err(Error::NoSession)
        ),
        "a forgotten connection is gone"
    );
    assert!(
        matches!(
            controller.core.forget_controller_session(sb.id),
            Err(Error::Busy)
        ),
        "a live connection cannot be forgotten"
    );
    let published: Vec<_> = std::iter::from_fn(|| events.try_recv()).collect();
    let closed = |wanted: u64| {
        published.iter().any(|event| {
            matches!(
                event,
                CoreEvent::SessionChanged {
                    id,
                    phase: SessionPhase::Closed,
                    role: SessionRole::Controller,
                    ..
                } if *id == wanted
            )
        })
    };
    assert!(closed(sa.id), "the closed connection is published closed");
    assert!(
        !closed(sb.id),
        "the live connection is not published closed"
    );

    drop((va, vb, da, db));
    controller.core.shutdown();
    wait_for("host b's session to close", DEFAULT_WAIT, || {
        (b.core.snapshot().ok()?.incoming?.phase == SessionPhase::Closed).then_some(())
    });
    Ok(())
}

/// A connection that fails has its own handle: the live session is
/// untouched, the failed one can be forgotten, and a retry gets a new
/// handle.
#[test]
fn a_failed_connection_has_its_own_handle_and_can_be_forgotten() -> TestResult {
    let trace = traced();
    let controller = Engine::start("controller");
    let a = Engine::start("host-a");
    let b = Engine::start("host-b");
    a.pair_for_control(&controller)?;
    b.pair_for_control(&controller)?;
    let sa = controller.connect_services(&a)?;
    b.core.stop_hosting()?;
    let pending = controller.core.begin_connect_services(&b.endpoint())?;
    wait_for("the connection to host b to fail", DEFAULT_WAIT, || {
        matches!(
            controller.core.controller_session(pending.id).ok()?.phase,
            SessionPhase::Failed(_)
        )
        .then_some(())
    });
    assert_eq!(
        controller.core.controller_session(sa.id)?,
        sa,
        "a failed connection leaves the live session"
    );
    controller.core.forget_controller_session(pending.id)?;
    b.core.start_hosting()?;
    let retry = controller.core.begin_connect_services(&b.endpoint())?;
    assert_ne!(retry.id, pending.id, "a retry gets a new handle");
    controller.active(retry.id);
    assert_eq!(
        controller.core.controller_session(sa.id)?,
        sa,
        "the retry leaves the live session"
    );
    vnilink_testkit::assert_story(
        expect![[r#"
        controller:
          session: session phase from=connecting generation=1 role=Controller session=1 to=active
          session: session failed failure=not hosting session=2
          session: session phase from=connecting generation=0 role=Controller session=2 to=failed
          session: session phase from=connecting generation=2 role=Controller session=3 to=active
    "#]],
        || trace.engine_story("controller", &["session"]),
    );
    Ok(())
}

/// Cancelling a connection while its host asks its owner, and one
/// connection reconnecting, leave the other connection as it was; the
/// reconnected connection keeps its handle under a new generation, the old
/// Desktop handle is fenced, and the host's video reaches only the Desktop
/// opened under the new generation.
#[test]
fn cancellation_during_approval_and_reconnect_do_not_stop_the_other_session() -> TestResult {
    let _trace = traced();
    let controller = Engine::start("controller");
    let a = Engine::start("host-a");
    let b = Engine::start("host-b");
    InputLog::install(&a, MONITOR)?;
    a.pair_for_control(&controller)?;
    b.pair_for_control(&controller)?;
    let sa = controller.connect_services(&a)?;
    b.core.unpair(&controller.endpoint())?;
    b.core.export_pairing_file(INVITATION, true)?;
    let pending = controller.core.begin_connect_services(&b.endpoint())?;
    wait_for("host b to ask its owner", DEFAULT_WAIT, || {
        b.core.control_request().ok().flatten()
    });
    controller.core.disconnect_session(pending.id)?;
    controller.wait_phase(pending.id, SessionPhase::Closed);
    assert_eq!(
        controller.core.controller_session(sa.id)?,
        sa,
        "cancelling during approval leaves the other connection"
    );
    if b.core.control_request()?.is_some() {
        b.core.reject_control_request()?;
    }
    b.core
        .accept_invitation(&controller.core.create_invitation(INVITATION)?)?;
    b.core.set_desktop_sharing(Grants {
        view: true,
        control: true,
        ..Grants::default()
    })?;
    let sb = controller.connect_services(&b)?;
    let owner =
        controller
            .core
            .attach_controller_service(sa.id, sa.generation, ServiceKind::Desktop)?;
    let old_desktop = controller
        .core
        .open_desktop_service(owner.id, owner.generation)?;
    let old = old_desktop.subscribe_video()?;
    // Only host a's link drops; the controller's handle survives the redial.
    controller.core.drop_controller_link(sa.id)?;
    let next = wait_for("host a's connection to reconnect", DEFAULT_WAIT, || {
        controller
            .core
            .controller_session(sa.id)
            .ok()
            .filter(|s| s.phase == SessionPhase::Active && s.generation > sa.generation)
    });
    assert_eq!(next.id, sa.id, "a reconnect keeps the connection's handle");
    assert_eq!(
        controller.core.controller_session(sb.id)?,
        sb,
        "one connection reconnecting leaves the other"
    );
    assert!(
        matches!(
            controller
                .core
                .attach_controller_service(sa.id, sa.generation, ServiceKind::Files),
            Err(Error::StaleSession)
        ),
        "the old generation takes no command"
    );
    assert!(
        old_desktop.is_cancelled(),
        "the old generation's Desktop is fenced"
    );
    let owner = controller.core.controller_service(owner.id)?;
    let fresh_desktop = controller
        .core
        .open_desktop_service(owner.id, owner.generation)?;
    let fresh = fresh_desktop.subscribe_video()?;
    let remote = wait_for("host a to serve the new Desktop", DEFAULT_WAIT, || {
        a.hosted(Some(ServiceKind::Desktop))
            .into_iter()
            .find(|service| service.generation == next.generation)
    });
    a.core
        .publish_desktop_video(remote.id, remote.generation, keyframe(44, vec![4]))?;
    assert_eq!(
        fresh
            .recv_timeout(Duration::from_secs(3))
            .ok_or("the new generation's picture")?
            .payload,
        [4],
        "the new generation's subscriber gets the picture"
    );
    assert!(
        old.recv_timeout(Duration::from_millis(50)).is_none(),
        "the old generation's subscriber gets nothing"
    );
    controller.core.unpair(&a.endpoint())?;
    controller.wait_phase(sa.id, SessionPhase::Closed);
    assert_eq!(
        controller.core.controller_session(sb.id)?,
        sb,
        "unpairing one host leaves the other's connection"
    );
    Ok(())
}

/// Two hosts number their jobs alike, yet each download reaches the host it
/// names: cancelling one leaves the other serving, both complete, and
/// closing one host's session interrupts only its job.
#[test]
fn file_jobs_with_identical_peer_ids_route_to_the_correct_connected_host() -> TestResult {
    let _trace = traced();
    let controller = Engine::start("controller");
    let a = Engine::start("host-a");
    let b = Engine::start("host-b");
    std::fs::write(a.folder().join("a.txt"), b"A's bytes")?;
    std::fs::write(b.folder().join("b.txt"), b"B's bytes")?;
    let transfer = Grants {
        transfer: true,
        ..Grants::default()
    };
    let mut tabs = Vec::new();
    for host in [&a, &b] {
        host.pair_for_control(&controller)?;
        host.core.set_desktop_sharing(transfer)?;
        controller.core.set_desktop_sharing(transfer)?;
        tabs.push(controller.files_on(host)?);
    }
    let download = |files: &Files, host: &Engine, name: &str| {
        let (folder, names) = source(&host.location().path(name));
        controller.core.files_service_download(
            files.service.id,
            files.service.generation,
            host.location().root,
            &folder,
            &names,
            controller.location().root,
            &controller.location().path(""),
        )
    };
    let ja = download(&tabs[0], &a, "a.txt")?;
    let jb = download(&tabs[1], &b, "b.txt")?;
    assert_ne!(ja, jb, "the controller's jobs have their own ids");
    assert_eq!(
        a.core.job_list()?[0].id,
        b.core.job_list()?[0].id,
        "both hosts number their jobs alike"
    );
    controller.core.job_cancel(jb)?;
    assert_eq!(
        b.core.job_list()?[0].state,
        JobState::Cancelled,
        "cancelling reaches the job's own host"
    );
    assert_eq!(
        a.core.job_list()?[0].state,
        JobState::Serving,
        "cancelling leaves the other host's job"
    );
    controller.core.job_resume(jb)?;
    // Every byte arrives quickly; a starved runner can take longer to finish
    // both files, so this bounds completion, not speed.
    let started = Instant::now();
    wait_for(
        "both downloads to complete",
        Duration::from_secs(31),
        || {
            assert!(
                started.elapsed() < Duration::from_secs(30),
                "controller={:?} A={:?} B={:?}",
                controller.core.job_list(),
                a.core.job_list(),
                b.core.job_list()
            );
            let poll = |core: &vnilink::Core, job: u64| core.job_poll(job).expect("a poll");
            poll(&controller.core, ja);
            poll(&controller.core, jb);
            poll(&a.core, a.core.job_list().expect("host a's jobs")[0].id);
            poll(&b.core, b.core.job_list().expect("host b's jobs")[0].id);
            let state = |job| controller.core.job_snapshot(job).expect("a job").state;
            (state(ja) == JobState::Complete && state(jb) == JobState::Complete).then_some(())
        },
    );
    assert_eq!(
        std::fs::read(controller.folder().join("a.txt"))?,
        b"A's bytes",
        "host a's file arrives from host a"
    );
    assert_eq!(
        std::fs::read(controller.folder().join("b.txt"))?,
        b"B's bytes",
        "host b's file arrives from host b"
    );

    std::fs::write(a.folder().join("closed.txt"), b"must stay paused")?;
    std::fs::write(b.folder().join("still-open.txt"), b"still transfers")?;
    let closed_job = download(&tabs[0], &a, "closed.txt")?;
    let open_job = download(&tabs[1], &b, "still-open.txt")?;
    controller.core.disconnect_session(tabs[0].connection.id)?;
    wait_for("host a's job to be interrupted", DEFAULT_WAIT, || {
        (controller
            .core
            .job_snapshot(closed_job)
            .expect("a job")
            .state
            == JobState::Interrupted)
            .then_some(())
    });
    wait_for("host b's job to complete", DEFAULT_WAIT, || {
        controller.core.drive_jobs().expect("the jobs driven");
        (controller.core.job_snapshot(open_job).expect("a job").state == JobState::Complete)
            .then_some(())
    });
    assert!(
        !controller.folder().join("closed.txt").exists(),
        "the closed session's job lands nothing"
    );
    assert_eq!(
        std::fs::read(controller.folder().join("still-open.txt"))?,
        b"still transfers",
        "the other host's job completes"
    );
    Ok(())
}
