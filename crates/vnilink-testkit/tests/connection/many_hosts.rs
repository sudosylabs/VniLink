//! One controller, several hosts at once: each session has its own handle
//! and channels even when both hosts number theirs alike, one failing,
//! cancelled, reconnecting or closing leaves the others, and the controller
//! holds at most four.

use expect_test::expect;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use vnilink::{
    CoreEvent, Error, Grants, InputEvent, JobState, KeyboardMode, LocalKey, Quality, SessionPhase,
    SessionRole,
};
use vnilink_testkit::desktop::{keyframe, next_picture, InputLog, RecordingClipboard};
use vnilink_testkit::{traced, wait_for, Engine, TestResult, DEFAULT_WAIT};

use super::support::{active, cursor_at, pair, wait_phase, CursorCapture, INVITATION};

/// The monitor of the desktops these scenarios install.
const MONITOR: (u16, u16) = (1920, 1080);

/// Two hosts number their sessions alike, yet the controller keeps two
/// handles with their own video, cursor, quality, keyboard settings, input
/// and clipboard. Closing one leaves the other as it was, a live session
/// cannot be forgotten, and the controller shutting down releases what the
/// other host held.
#[test]
fn two_hosts_with_identical_wire_ids_have_independent_channels_and_cleanup() -> TestResult {
    let _trace = traced();
    let controller = Engine::start("controller");
    let a = Engine::start("host-a");
    let b = Engine::start("host-b");
    controller.core.set_cursor_receiver(true)?;
    let mut keys = Vec::new();
    let mut clips = Vec::new();
    for host in [&a, &b] {
        keys.push(InputLog::install(host, MONITOR)?);
        let clip = Arc::new(Mutex::new(Vec::new()));
        host.core.set_clipboard(Box::new(RecordingClipboard {
            texts: Arc::clone(&clip),
        }))?;
        clips.push(clip);
        host.core.set_capture(Box::new(CursorCapture))?;
        pair(host, &controller)?;
        host.core.set_desktop_sharing(Grants {
            view: true,
            control: true,
            clipboard: true,
            transfer: false,
        })?;
    }
    let (keys_a, keys_b) = (&keys[0], &keys[1]);
    let clipped = |index: usize| -> Vec<String> {
        clips[index]
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .clone()
    };
    let events = controller.core.subscribe()?;
    let pending_a = controller.core.begin_connect(&a.endpoint())?;
    let pending_b = controller.core.begin_connect(&b.endpoint())?;
    assert_ne!(pending_a.id, pending_b.id, "each host has its own handle");
    let sa = active(&controller, pending_a.id);
    let mut sb = active(&controller, pending_b.id);
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
        "the controller holds a session to each host"
    );
    assert!(
        controller.core.snapshot()?.outgoing.is_none(),
        "two sessions are no single outgoing session"
    );
    assert!(
        matches!(controller.core.disconnect(), Err(Error::Busy)),
        "a disconnect that does not name a session is ambiguous"
    );
    assert!(
        matches!(
            controller.core.begin_connect(&a.endpoint()),
            Err(Error::Busy)
        ),
        "a second session to the same host is busy"
    );

    let va = controller
        .core
        .subscribe_session_video(sa.id, sa.generation)?;
    let vb = controller
        .core
        .subscribe_session_video(sb.id, sb.generation)?;
    assert!(
        matches!(
            controller
                .core
                .subscribe_session_video(sa.id, sa.generation),
            Err(Error::Busy)
        ),
        "a session's video has one subscriber"
    );
    a.core.publish_video(ha.id, keyframe(11, vec![1, 1]))?;
    b.core.publish_video(hb.id, keyframe(22, vec![2, 2]))?;
    assert_eq!(
        next_picture(&va, "host a's picture").payload,
        [1, 1],
        "host a's video reaches its own session"
    );
    assert_eq!(
        next_picture(&vb, "host b's picture").payload,
        [2, 2],
        "host b's video reaches its own session"
    );

    a.core
        .publish_cursor(ha.id, ha.generation, cursor_at((11, 12)))?;
    b.core
        .publish_cursor(hb.id, hb.generation, cursor_at((21, 22)))?;
    let cursor = |session: &vnilink::SessionInfo| {
        controller
            .core
            .cursor(session.id, session.generation)
            .ok()
            .flatten()
    };
    wait_for("both cursors to arrive", DEFAULT_WAIT, || {
        (cursor(&sa).is_some() && cursor(&sb).is_some()).then_some(())
    });
    assert_eq!(
        (
            cursor(&sa).ok_or("a cursor")?.position,
            cursor(&sb).ok_or("a cursor")?.position
        ),
        ((11, 12), (21, 22)),
        "each host's cursor reaches its own session"
    );

    let quality = Quality::new(1500, 30, 1280, 720)?;
    controller
        .core
        .update_session_quality(sb.id, SessionRole::Controller, quality)?;
    wait_for("host b to apply the quality", DEFAULT_WAIT, || {
        (b.core.snapshot().ok()?.incoming?.quality == Some(quality)).then_some(())
    });
    assert_eq!(
        controller.core.controller_session(sa.id)?.quality,
        sa.quality,
        "one session's quality leaves the other's"
    );
    sb = controller.core.controller_session(sb.id)?;
    controller
        .core
        .set_session_keyboard_mode(sa.id, sa.generation, KeyboardMode::Map)?;
    assert_eq!(
        controller
            .core
            .session_keyboard_mode(sb.id, sb.generation)?,
        KeyboardMode::Character,
        "one session's keyboard mode leaves the other's"
    );
    controller
        .core
        .set_session_swap_control_and_command(sa.id, sa.generation, true)?;
    assert!(
        !controller
            .core
            .session_swap_control_and_command(sb.id, sb.generation)?,
        "one session's shortcut swap leaves the other's"
    );

    for session in [&sa, &sb] {
        controller.core.set_session_keyboard_mode(
            session.id,
            session.generation,
            KeyboardMode::Map,
        )?;
        controller.core.submit_local_key(
            session.id,
            session.generation,
            LocalKey::new(4, true, "a"),
        )?;
    }
    assert_eq!(
        keys_a.wait_for(1),
        [InputEvent::key_down(4)],
        "a session's key reaches its own host"
    );
    assert_eq!(
        keys_b.wait_for(1),
        [InputEvent::key_down(4)],
        "a session's key reaches its own host"
    );
    controller
        .core
        .release_session_input(sa.id, sa.generation)?;
    assert_eq!(
        keys_a.exactly(2)[1],
        InputEvent::key_up(4),
        "releasing one session lifts its host's key"
    );
    assert_eq!(
        keys_b.events().len(),
        1,
        "releasing one session leaves the other host's key held"
    );

    controller
        .core
        .submit_clipboard(sa.id, sa.generation, "to-a")?;
    controller
        .core
        .submit_clipboard(sb.id, sb.generation, "to-b")?;
    assert_eq!(
        (clipped(0), clipped(1)),
        (vec!["to-a".to_owned()], vec!["to-b".to_owned()]),
        "each session's clipboard reaches its own host"
    );
    a.core.publish_clipboard(ha.id, "from-a")?;
    b.core.publish_clipboard(hb.id, "from-b")?;
    let clipboard = |session: &vnilink::SessionInfo| {
        controller
            .core
            .session_clipboard_text(session.id, session.generation)
            .ok()
            .flatten()
    };
    wait_for("both hosts' clipboards to arrive", DEFAULT_WAIT, || {
        (clipboard(&sa).is_some() && clipboard(&sb).is_some()).then_some(())
    });
    assert_eq!(
        (clipboard(&sa), clipboard(&sb)),
        (Some("from-a".to_owned()), Some("from-b".to_owned())),
        "each host's clipboard reaches its own session"
    );
    assert!(
        matches!(controller.core.clipboard_text(), Err(Error::Busy)),
        "a clipboard that does not name a session is ambiguous"
    );
    for session in [&sa, &sb] {
        assert!(
            !controller
                .core
                .controller_session_paths(session.id)?
                .is_empty(),
            "each session reports its own paths"
        );
    }

    controller.core.disconnect_session(sa.id)?;
    wait_phase(&controller, sa.id, SessionPhase::Closed);
    assert_eq!(
        controller.core.controller_session(sb.id)?,
        sb,
        "closing one session leaves the other as it was"
    );
    assert_eq!(
        clipboard(&sb).as_deref(),
        Some("from-b"),
        "closing one session leaves the other's clipboard"
    );
    b.core.publish_video(hb.id, keyframe(33, vec![3]))?;
    assert_eq!(
        next_picture(&vb, "host b's next picture").payload,
        [3],
        "closing one session leaves the other's video"
    );
    controller.core.forget_controller_session(sa.id)?;
    assert!(
        matches!(
            controller.core.controller_session(sa.id),
            Err(Error::NoSession)
        ),
        "a forgotten session is gone"
    );
    assert!(
        matches!(
            controller.core.forget_controller_session(sb.id),
            Err(Error::Busy)
        ),
        "a live session cannot be forgotten"
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
    assert!(closed(sa.id), "the closed session is published closed");
    assert!(!closed(sb.id), "the live session is not published closed");

    controller.core.shutdown();
    wait_for("host b's session to close", DEFAULT_WAIT, || {
        (b.core.snapshot().ok()?.incoming?.phase == SessionPhase::Closed).then_some(())
    });
    assert_eq!(
        keys_b.exactly(2)[1],
        InputEvent::key_up(4),
        "the controller shutting down lifts host b's held key"
    );
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
    pair(&a, &controller)?;
    pair(&b, &controller)?;
    let sa = controller.connect_control(&a)?;
    b.core.stop_hosting()?;
    let pending = controller.core.begin_connect(&b.endpoint())?;
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
    let retry = controller.core.begin_connect(&b.endpoint())?;
    assert_ne!(retry.id, pending.id, "a retry gets a new handle");
    active(&controller, retry.id);
    assert_eq!(
        controller.core.controller_session(sa.id)?,
        sa,
        "the retry leaves the live session"
    );
    expect![[r#"
        controller:
          session: session phase from=connecting generation=1 role=Controller session=1 to=active
          session: session failed failure=not hosting session=2
          session: session phase from=connecting generation=0 role=Controller session=2 to=failed
          session: session phase from=connecting generation=2 role=Controller session=3 to=active
    "#]]
    .assert_eq(&trace.engine_story("controller", &["session"]));
    Ok(())
}

/// Cancelling a connection while its host asks its owner, and one session
/// reconnecting, leave the other session as it was; the reconnected
/// session keeps its handle under a new generation whose video reaches
/// only its new subscriber.
#[test]
fn cancellation_during_approval_and_reconnect_do_not_stop_the_other_session() -> TestResult {
    let _trace = traced();
    let controller = Engine::start("controller");
    let a = Engine::start("host-a");
    let b = Engine::start("host-b");
    pair(&a, &controller)?;
    pair(&b, &controller)?;
    let sa = controller.connect_control(&a)?;
    b.core.unpair(&controller.endpoint())?;
    b.core.export_pairing_file(INVITATION, true)?;
    let pending = controller.core.begin_connect(&b.endpoint())?;
    wait_for("host b to ask its owner", DEFAULT_WAIT, || {
        b.core.control_request().ok().flatten()
    });
    controller.core.disconnect_session(pending.id)?;
    wait_phase(&controller, pending.id, SessionPhase::Closed);
    assert_eq!(
        controller.core.controller_session(sa.id)?,
        sa,
        "cancelling during approval leaves the other session"
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
    let sb = controller.connect_control(&b)?;
    let old = controller
        .core
        .subscribe_session_video(sa.id, sa.generation)?;
    // Only host a's link drops; the controller's handle survives the redial.
    controller.core.drop_controller_link(sa.id)?;
    let next = wait_for("host a's session to reconnect", DEFAULT_WAIT, || {
        controller
            .core
            .controller_session(sa.id)
            .ok()
            .filter(|s| s.phase == SessionPhase::Active && s.generation > sa.generation)
    });
    assert_eq!(next.id, sa.id, "a reconnect keeps the session's handle");
    assert_eq!(
        controller.core.controller_session(sb.id)?,
        sb,
        "one session reconnecting leaves the other"
    );
    assert!(
        matches!(
            controller.core.submit_text(sa.id, sa.generation, "stale"),
            Err(Error::StaleSession)
        ),
        "the old generation takes no input"
    );
    let fresh = controller
        .core
        .subscribe_session_video(next.id, next.generation)?;
    let ha = a.core.snapshot()?.incoming.ok_or("host a's session")?;
    a.core.publish_video(ha.id, keyframe(44, vec![4]))?;
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
    wait_phase(&controller, sa.id, SessionPhase::Closed);
    assert_eq!(
        controller.core.controller_session(sb.id)?,
        sb,
        "unpairing one host leaves the other's session"
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
    std::fs::write(a.root().join("a.txt"), b"A's bytes")?;
    std::fs::write(b.root().join("b.txt"), b"B's bytes")?;
    let transfer = Grants {
        transfer: true,
        ..Grants::default()
    };
    for host in [&a, &b] {
        pair(host, &controller)?;
        host.core.set_desktop_sharing(transfer)?;
        controller.core.set_desktop_sharing(transfer)?;
        active(
            &controller,
            controller.core.begin_connect_files(&host.endpoint())?.id,
        );
    }
    let download = |host: &Engine, name: &str| {
        controller.core.job_submit_download(
            &host.endpoint(),
            host.root_id(),
            name,
            controller.root_id(),
            "",
        )
    };
    let ja = download(&a, "a.txt")?;
    let jb = download(&b, "b.txt")?;
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
        std::fs::read(controller.root().join("a.txt"))?,
        b"A's bytes",
        "host a's file arrives from host a"
    );
    assert_eq!(
        std::fs::read(controller.root().join("b.txt"))?,
        b"B's bytes",
        "host b's file arrives from host b"
    );

    std::fs::write(a.root().join("closed.txt"), b"must stay paused")?;
    std::fs::write(b.root().join("still-open.txt"), b"still transfers")?;
    let closed_job = download(&a, "closed.txt")?;
    let open_job = download(&b, "still-open.txt")?;
    let a_session = controller
        .core
        .controller_sessions()?
        .into_iter()
        .find(|s| s.peer_id == a.endpoint())
        .ok_or("host a's session")?;
    controller.core.disconnect_session(a_session.id)?;
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
        !controller.root().join("closed.txt").exists(),
        "the closed session's job lands nothing"
    );
    assert_eq!(
        std::fs::read(controller.root().join("still-open.txt"))?,
        b"still transfers",
        "the other host's job completes"
    );
    Ok(())
}

/// A controller holds four sessions; a fifth is busy and leaves no record,
/// and once one closes the fifth host is reached while the rest stay
/// active.
#[test]
fn a_fifth_outgoing_connection_is_busy_until_one_closes() -> TestResult {
    let trace = traced();
    let controller = Engine::start("controller");
    let hosts: Vec<Engine> = (1..=5)
        .map(|n| Engine::start(&format!("host-{n}")))
        .collect();
    for host in &hosts {
        pair(host, &controller)?;
    }
    let mut live = Vec::new();
    for host in &hosts[..4] {
        live.push(controller.connect_control(host)?);
    }
    assert_eq!(
        controller.core.controller_sessions()?.len(),
        4,
        "a controller holds four sessions"
    );
    assert!(
        matches!(
            controller.core.begin_connect(&hosts[4].endpoint()),
            Err(Error::Busy)
        ),
        "a fifth outgoing session is busy"
    );
    assert_eq!(
        controller.core.controller_sessions()?.len(),
        4,
        "the refused host left no session record"
    );
    controller.core.disconnect_session(live[0].id)?;
    wait_phase(&controller, live[0].id, SessionPhase::Closed);
    let fifth = controller.connect_control(&hosts[4])?;
    assert_eq!(
        fifth.peer_id,
        hosts[4].endpoint(),
        "once one closes, the fifth host is reached"
    );
    for survivor in &live[1..] {
        assert_eq!(
            controller.core.controller_session(survivor.id)?.phase,
            SessionPhase::Active,
            "the other sessions stay active"
        );
    }
    expect![[r#"
        controller:
          session: session phase from=connecting generation=1 role=Controller session=1 to=active
          session: session phase from=connecting generation=1 role=Controller session=2 to=active
          session: session phase from=connecting generation=1 role=Controller session=3 to=active
          session: session phase from=connecting generation=1 role=Controller session=4 to=active
          session: session phase from=active generation=1 role=Controller session=1 to=stopping
          session: session phase from=stopping generation=1 role=Controller session=1 to=closed
          session: session phase from=connecting generation=1 role=Controller session=5 to=active
    "#]]
    .assert_eq(&trace.engine_story("controller", &["session"]));
    Ok(())
}
