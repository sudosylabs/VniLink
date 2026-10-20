use super::filesystem_location::FilesystemLocation;
use super::support::{pair_for_control, start_direct, RecordingDesktop};
use crate::{
    Core, CursorState, EncodedFrame, Error, Grants, HostClipboard, InputEvent, KeyboardMode,
    LocalKey, ServiceKind, SessionInfo, SessionPhase, SessionRole,
};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

fn wait(ready: impl FnMut() -> bool) {
    wait_up_to(Duration::from_secs(8), ready);
}
fn wait_up_to(limit: Duration, mut ready: impl FnMut() -> bool) {
    let until = Instant::now() + limit;
    while !ready() {
        assert!(Instant::now() < until, "multi-session condition timed out");
        std::thread::sleep(Duration::from_millis(10));
    }
}
struct Capture;
impl crate::HostCapture for Capture {
    fn monitor(&self) -> (u16, u16) {
        (1920, 1080)
    }
    fn separate_cursor(&self) -> bool {
        true
    }
}
fn active(core: &Core, id: u64) -> SessionInfo {
    wait(|| core.controller_session(id).unwrap().phase == SessionPhase::Active);
    core.controller_session(id).unwrap()
}
struct Clipboard(Arc<Mutex<Vec<String>>>);
impl HostClipboard for Clipboard {
    fn set_text(&mut self, text: &str) -> Result<(), Error> {
        self.0.lock().unwrap().push(text.into());
        Ok(())
    }
}

#[test]
fn two_hosts_with_identical_wire_ids_have_independent_channels_and_cleanup() {
    let controller = start_direct();
    let a = start_direct();
    let b = start_direct();
    controller.set_cursor_receiver(true).unwrap();
    let keys_a = Arc::new(Mutex::new(Vec::new()));
    let keys_b = Arc::new(Mutex::new(Vec::new()));
    let clips_a = Arc::new(Mutex::new(Vec::new()));
    let clips_b = Arc::new(Mutex::new(Vec::new()));
    for (host, keys, clips) in [(&a, &keys_a, &clips_a), (&b, &keys_b, &clips_b)] {
        host.set_desktop(Box::new(RecordingDesktop {
            events: keys.clone(),
        }))
        .unwrap();
        host.set_clipboard(Box::new(Clipboard(clips.clone())))
            .unwrap();
        host.set_capture(Box::new(Capture)).unwrap();
        pair_for_control(host, &controller);
        host.set_desktop_sharing(Grants {
            view: true,
            control: true,
            clipboard: true,
            transfer: false,
        })
        .unwrap();
    }
    let events = controller.subscribe().unwrap();
    let pending_a = controller.begin_connect(&a.endpoint_id()).unwrap();
    let pending_b = controller.begin_connect(&b.endpoint_id()).unwrap();
    assert_ne!(pending_a.id, pending_b.id);
    let sa = active(&controller, pending_a.id);
    let mut sb = active(&controller, pending_b.id);
    let ha = a.snapshot().unwrap().incoming.unwrap();
    let hb = b.snapshot().unwrap().incoming.unwrap();
    assert_eq!((ha.id, ha.generation), (hb.id, hb.generation));
    assert_eq!(controller.controller_sessions().unwrap().len(), 2);
    assert!(controller.snapshot().unwrap().outgoing.is_none());
    assert!(matches!(controller.disconnect(), Err(Error::Busy)));
    assert!(matches!(
        controller.begin_connect(&a.endpoint_id()),
        Err(Error::Busy)
    ));
    let va = controller
        .subscribe_session_video(sa.id, sa.generation)
        .unwrap();
    let vb = controller
        .subscribe_session_video(sb.id, sb.generation)
        .unwrap();
    assert!(matches!(
        controller.subscribe_session_video(sa.id, sa.generation),
        Err(Error::Busy)
    ));
    a.publish_video(ha.id, EncodedFrame::h264(true, 11, vec![1, 1]).unwrap())
        .unwrap();
    b.publish_video(hb.id, EncodedFrame::h264(true, 22, vec![2, 2]).unwrap())
        .unwrap();
    assert_eq!(
        va.recv_timeout(Duration::from_secs(3)).unwrap().payload,
        vec![1, 1]
    );
    assert_eq!(
        vb.recv_timeout(Duration::from_secs(3)).unwrap().payload,
        vec![2, 2]
    );
    for (host, hs, pos) in [(&a, &ha, (11, 12)), (&b, &hb, (21, 22))] {
        host.publish_cursor(
            hs.id,
            hs.generation,
            CursorState {
                position: pos,
                visible: true,
                image: None,
            },
        )
        .unwrap();
    }
    wait(|| {
        controller.cursor(sa.id, sa.generation).unwrap().is_some()
            && controller.cursor(sb.id, sb.generation).unwrap().is_some()
    });
    assert_eq!(
        controller
            .cursor(sa.id, sa.generation)
            .unwrap()
            .unwrap()
            .position,
        (11, 12)
    );
    assert_eq!(
        controller
            .cursor(sb.id, sb.generation)
            .unwrap()
            .unwrap()
            .position,
        (21, 22)
    );
    let quality = crate::Quality::new(1500, 30, 1280, 720).unwrap();
    controller
        .update_session_quality(sb.id, SessionRole::Controller, quality)
        .unwrap();
    wait(|| b.snapshot().unwrap().incoming.unwrap().quality == Some(quality));
    assert_eq!(
        controller.controller_session(sa.id).unwrap().quality,
        sa.quality
    );
    sb = controller.controller_session(sb.id).unwrap();
    controller
        .set_session_keyboard_mode(sa.id, sa.generation, KeyboardMode::Map)
        .unwrap();
    assert_eq!(
        controller
            .session_keyboard_mode(sb.id, sb.generation)
            .unwrap(),
        KeyboardMode::Character
    );
    controller
        .set_session_swap_control_and_command(sa.id, sa.generation, true)
        .unwrap();
    assert!(!controller
        .session_swap_control_and_command(sb.id, sb.generation)
        .unwrap());
    for session in [&sa, &sb] {
        controller
            .set_session_keyboard_mode(session.id, session.generation, KeyboardMode::Map)
            .unwrap();
        controller
            .submit_local_key(session.id, session.generation, LocalKey::new(4, true, "a"))
            .unwrap();
    }
    wait(|| !keys_a.lock().unwrap().is_empty() && !keys_b.lock().unwrap().is_empty());
    assert_eq!(*keys_a.lock().unwrap(), vec![InputEvent::key_down(4)]);
    assert_eq!(*keys_b.lock().unwrap(), vec![InputEvent::key_down(4)]);
    controller
        .release_session_input(sa.id, sa.generation)
        .unwrap();
    wait(|| keys_a.lock().unwrap().len() == 2);
    assert_eq!(keys_a.lock().unwrap()[1], InputEvent::key_up(4));
    assert_eq!(keys_b.lock().unwrap().len(), 1);
    controller
        .submit_clipboard(sa.id, sa.generation, "to-a")
        .unwrap();
    controller
        .submit_clipboard(sb.id, sb.generation, "to-b")
        .unwrap();
    assert_eq!(*clips_a.lock().unwrap(), vec!["to-a"]);
    assert_eq!(*clips_b.lock().unwrap(), vec!["to-b"]);
    a.publish_clipboard(ha.id, "from-a").unwrap();
    b.publish_clipboard(hb.id, "from-b").unwrap();
    wait(|| {
        controller
            .session_clipboard_text(sa.id, sa.generation)
            .unwrap()
            .is_some()
            && controller
                .session_clipboard_text(sb.id, sb.generation)
                .unwrap()
                .is_some()
    });
    assert_eq!(
        controller
            .session_clipboard_text(sa.id, sa.generation)
            .unwrap()
            .as_deref(),
        Some("from-a")
    );
    assert_eq!(
        controller
            .session_clipboard_text(sb.id, sb.generation)
            .unwrap()
            .as_deref(),
        Some("from-b")
    );
    assert!(matches!(controller.clipboard_text(), Err(Error::Busy)));
    assert!(!controller
        .controller_session_paths(sa.id)
        .unwrap()
        .is_empty());
    assert!(!controller
        .controller_session_paths(sb.id)
        .unwrap()
        .is_empty());
    controller.disconnect_session(sa.id).unwrap();
    wait(|| controller.controller_session(sa.id).unwrap().phase == SessionPhase::Closed);
    assert_eq!(controller.controller_session(sb.id).unwrap(), sb);
    assert_eq!(
        controller
            .session_clipboard_text(sb.id, sb.generation)
            .unwrap()
            .as_deref(),
        Some("from-b")
    );
    b.publish_video(hb.id, EncodedFrame::h264(true, 33, vec![3]).unwrap())
        .unwrap();
    assert_eq!(
        vb.recv_timeout(Duration::from_secs(3)).unwrap().payload,
        vec![3]
    );
    controller.forget_controller_session(sa.id).unwrap();
    assert!(matches!(
        controller.controller_session(sa.id),
        Err(Error::NoSession)
    ));
    assert!(matches!(
        controller.forget_controller_session(sb.id),
        Err(Error::Busy)
    ));
    let recorded: Vec<_> = std::iter::from_fn(|| events.try_recv()).collect();
    assert!(recorded.iter().any(|e| matches!(e, crate::CoreEvent::SessionChanged { id, phase: SessionPhase::Closed, role: SessionRole::Controller, .. } if *id == sa.id)));
    assert!(!recorded.iter().any(|e| matches!(e, crate::CoreEvent::SessionChanged { id, phase: SessionPhase::Closed, role: SessionRole::Controller, .. } if *id == sb.id)));
    controller.shutdown();
    wait(|| b.snapshot().unwrap().incoming.unwrap().phase == SessionPhase::Closed);
    wait(|| keys_b.lock().unwrap().len() == 2);
    assert_eq!(keys_b.lock().unwrap()[1], InputEvent::key_up(4));
    a.shutdown();
    b.shutdown();
}

#[test]
fn a_failed_connection_has_its_own_handle_and_can_be_forgotten() {
    let controller = start_direct();
    let a = start_direct();
    let b = start_direct();
    pair_for_control(&a, &controller);
    pair_for_control(&b, &controller);
    let sa = active(
        &controller,
        controller.begin_connect(&a.endpoint_id()).unwrap().id,
    );
    b.stop_hosting().unwrap();
    let pending = controller.begin_connect(&b.endpoint_id()).unwrap();
    wait(|| {
        matches!(
            controller.controller_session(pending.id).unwrap().phase,
            SessionPhase::Failed(_)
        )
    });
    assert_eq!(controller.controller_session(sa.id).unwrap(), sa);
    controller.forget_controller_session(pending.id).unwrap();
    b.start_hosting().unwrap();
    let retry = controller.begin_connect(&b.endpoint_id()).unwrap();
    assert_ne!(retry.id, pending.id);
    active(&controller, retry.id);
    assert_eq!(controller.controller_session(sa.id).unwrap(), sa);
    controller.shutdown();
    a.shutdown();
    b.shutdown();
}

#[test]
fn cancellation_during_approval_and_reconnect_do_not_stop_the_other_session() {
    let controller = start_direct();
    let a = start_direct();
    let b = start_direct();
    pair_for_control(&a, &controller);
    pair_for_control(&b, &controller);
    let sa = active(
        &controller,
        controller.begin_connect(&a.endpoint_id()).unwrap().id,
    );
    b.unpair(&controller.endpoint_id()).unwrap();
    b.export_pairing_file(Duration::from_secs(60), true)
        .unwrap();
    let pending = controller.begin_connect(&b.endpoint_id()).unwrap();
    wait(|| b.control_request().unwrap().is_some());
    controller.disconnect_session(pending.id).unwrap();
    wait(|| controller.controller_session(pending.id).unwrap().phase == SessionPhase::Closed);
    assert_eq!(controller.controller_session(sa.id).unwrap(), sa);
    if b.control_request().unwrap().is_some() {
        b.reject_control_request().unwrap();
    }
    b.accept_invitation(
        &controller
            .create_invitation(Duration::from_secs(60))
            .unwrap(),
    )
    .unwrap();
    b.set_desktop_sharing(Grants {
        view: true,
        control: true,
        ..Grants::default()
    })
    .unwrap();
    let sb = active(
        &controller,
        controller.begin_connect(&b.endpoint_id()).unwrap().id,
    );
    let old = controller
        .subscribe_session_video(sa.id, sa.generation)
        .unwrap();
    // Drop only A's link; the controller's local tab id survives redial.
    controller.drop_controller_link(sa.id).unwrap();
    wait(|| {
        controller.controller_session(sa.id).unwrap().phase == SessionPhase::Active
            && controller.controller_session(sa.id).unwrap().generation > sa.generation
    });
    let next = controller.controller_session(sa.id).unwrap();
    assert_eq!(next.id, sa.id);
    assert_eq!(controller.controller_session(sb.id).unwrap(), sb);
    assert!(matches!(
        controller.submit_text(sa.id, sa.generation, "stale"),
        Err(Error::StaleSession)
    ));
    let fresh = controller
        .subscribe_session_video(next.id, next.generation)
        .unwrap();
    let ha = a.snapshot().unwrap().incoming.unwrap();
    a.publish_video(ha.id, EncodedFrame::h264(true, 44, vec![4]).unwrap())
        .unwrap();
    assert_eq!(
        fresh.recv_timeout(Duration::from_secs(3)).unwrap().payload,
        vec![4]
    );
    assert!(old.recv_timeout(Duration::from_millis(50)).is_none());
    controller.unpair(&a.endpoint_id()).unwrap();
    wait(|| controller.controller_session(sa.id).unwrap().phase == SessionPhase::Closed);
    assert_eq!(controller.controller_session(sb.id).unwrap(), sb);
    controller.shutdown();
    a.shutdown();
    b.shutdown();
}

#[test]
fn file_jobs_with_identical_peer_ids_route_to_the_correct_connected_host() {
    let controller = start_direct();
    let a = start_direct();
    let b = start_direct();
    let dir = std::env::temp_dir().join(format!("vnilink-multi-jobs-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    for (core, name) in [(&controller, "controller"), (&a, "a"), (&b, "b")] {
        std::fs::create_dir_all(dir.join(name).join("root")).unwrap();
        core.enable_filesystem_access().unwrap();
        core.set_transfer_directory(dir.join(name).join("blobs"))
            .unwrap();
    }
    let controller_files = FilesystemLocation::new(
        controller.file_roots().unwrap(),
        &dir.join("controller").join("root"),
    );
    let a_files = FilesystemLocation::new(a.file_roots().unwrap(), &dir.join("a").join("root"));
    let b_files = FilesystemLocation::new(b.file_roots().unwrap(), &dir.join("b").join("root"));
    std::fs::write(dir.join("a/root/a.txt"), b"A's bytes").unwrap();
    std::fs::write(dir.join("b/root/b.txt"), b"B's bytes").unwrap();
    for host in [&a, &b] {
        pair_for_control(host, &controller);
        host.set_desktop_sharing(Grants {
            transfer: true,
            ..Grants::default()
        })
        .unwrap();
        controller
            .set_desktop_sharing(Grants {
                transfer: true,
                ..Grants::default()
            })
            .unwrap();
        active(
            &controller,
            controller
                .begin_connect_files(&host.endpoint_id())
                .unwrap()
                .id,
        );
    }
    let ja = controller
        .job_submit_download(
            &a.endpoint_id(),
            a_files.root,
            &a_files.path("a.txt"),
            controller_files.root,
            &controller_files.path(""),
        )
        .unwrap();
    let jb = controller
        .job_submit_download(
            &b.endpoint_id(),
            b_files.root,
            &b_files.path("b.txt"),
            controller_files.root,
            &controller_files.path(""),
        )
        .unwrap();
    assert_ne!(ja, jb);
    assert_eq!(a.job_list().unwrap()[0].id, b.job_list().unwrap()[0].id);
    controller.job_cancel(jb).unwrap();
    assert_eq!(b.job_list().unwrap()[0].state, crate::JobState::Cancelled);
    assert_eq!(a.job_list().unwrap()[0].state, crate::JobState::Serving);
    controller.job_resume(jb).unwrap();
    // Every byte arrives quickly; finishing both files can take a starved
    // Windows runner longer, so this waits on completion, not speed.
    let diagnostic = Instant::now();
    wait_up_to(Duration::from_secs(31), || {
        assert!(
            diagnostic.elapsed() < Duration::from_secs(30),
            "controller={:?} A={:?} B={:?}",
            controller.job_list(),
            a.job_list(),
            b.job_list()
        );
        controller.job_poll(ja).unwrap();
        controller.job_poll(jb).unwrap();
        a.job_poll(a.job_list().unwrap()[0].id).unwrap();
        b.job_poll(b.job_list().unwrap()[0].id).unwrap();
        controller.job_snapshot(ja).unwrap().state == crate::JobState::Complete
            && controller.job_snapshot(jb).unwrap().state == crate::JobState::Complete
    });
    assert_eq!(
        std::fs::read(dir.join("controller/root/a.txt")).unwrap(),
        b"A's bytes"
    );
    assert_eq!(
        std::fs::read(dir.join("controller/root/b.txt")).unwrap(),
        b"B's bytes"
    );
    std::fs::write(dir.join("a/root/closed.txt"), b"must stay paused").unwrap();
    std::fs::write(dir.join("b/root/still-open.txt"), b"still transfers").unwrap();
    let closed_job = controller
        .job_submit_download(
            &a.endpoint_id(),
            a_files.root,
            &a_files.path("closed.txt"),
            controller_files.root,
            &controller_files.path(""),
        )
        .unwrap();
    let open_job = controller
        .job_submit_download(
            &b.endpoint_id(),
            b_files.root,
            &b_files.path("still-open.txt"),
            controller_files.root,
            &controller_files.path(""),
        )
        .unwrap();
    let a_session = controller
        .controller_sessions()
        .unwrap()
        .into_iter()
        .find(|s| s.peer_id == a.endpoint_id())
        .unwrap();
    controller.disconnect_session(a_session.id).unwrap();
    wait(|| controller.job_snapshot(closed_job).unwrap().state == crate::JobState::Interrupted);
    wait(|| {
        controller.drive_jobs().unwrap();
        controller.job_snapshot(open_job).unwrap().state == crate::JobState::Complete
    });
    assert!(!dir.join("controller/root/closed.txt").exists());
    assert_eq!(
        std::fs::read(dir.join("controller/root/still-open.txt")).unwrap(),
        b"still transfers"
    );
    controller.shutdown();
    a.shutdown();
    b.shutdown();
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn a_fifth_outgoing_connection_is_busy_until_one_closes() {
    let controller = start_direct();
    let hosts: Vec<Core> = (0..5).map(|_| start_direct()).collect();
    for host in &hosts {
        pair_for_control(host, &controller);
    }
    let mut live = Vec::new();
    for host in hosts.iter().take(4) {
        live.push(active(
            &controller,
            controller.begin_connect(&host.endpoint_id()).unwrap().id,
        ));
    }
    assert_eq!(controller.controller_sessions().unwrap().len(), 4);
    assert!(matches!(
        controller.begin_connect(&hosts[4].endpoint_id()),
        Err(Error::Busy)
    ));
    // The refused peer left no session record behind.
    assert_eq!(controller.controller_sessions().unwrap().len(), 4);
    controller.disconnect_session(live[0].id).unwrap();
    wait(|| controller.controller_session(live[0].id).unwrap().phase == SessionPhase::Closed);
    let fifth = active(
        &controller,
        controller
            .begin_connect(&hosts[4].endpoint_id())
            .unwrap()
            .id,
    );
    assert_eq!(fifth.peer_id, hosts[4].endpoint_id());
    for survivor in &live[1..] {
        assert_eq!(
            controller.controller_session(survivor.id).unwrap().phase,
            SessionPhase::Active
        );
    }
    controller.shutdown();
    for host in &hosts {
        host.shutdown();
    }
}

#[test]
fn a_saturated_viewer_does_not_slow_another_viewer() {
    let host = start_direct();
    let slow = start_direct();
    let fast = start_direct();
    for viewer in [&slow, &fast] {
        pair_for_control(&host, viewer);
        host.set_desktop_sharing(Grants {
            view: true,
            ..Grants::default()
        })
        .unwrap();
    }
    let sa = active(&slow, slow.begin_connect(&host.endpoint_id()).unwrap().id);
    let sb = active(&fast, fast.begin_connect(&host.endpoint_id()).unwrap().id);
    // Only the fast viewer subscribes; the slow viewer never reads video.
    let vb = fast.subscribe_session_video(sb.id, sb.generation).unwrap();
    let hosted = host.hosted_sessions().unwrap();
    assert_eq!(hosted.len(), 2);
    let ha = hosted
        .iter()
        .find(|s| s.peer_id == slow.endpoint_id())
        .unwrap()
        .id;
    let hb = hosted
        .iter()
        .find(|s| s.peer_id == fast.endpoint_id())
        .unwrap()
        .id;
    // Flood the slow viewer's outbox past its 8-frame bound.
    host.publish_video(ha, EncodedFrame::h264(true, 1, vec![7]).unwrap())
        .unwrap();
    for stamp in 2..=41 {
        host.publish_video(ha, EncodedFrame::h264(false, stamp, vec![7]).unwrap())
            .unwrap();
    }
    // The fast viewer still gets a fresh keyframe promptly.
    host.publish_video(hb, EncodedFrame::h264(true, 777, vec![9]).unwrap())
        .unwrap();
    assert_eq!(
        vb.recv_timeout(Duration::from_secs(3))
            .unwrap()
            .timestamp_us,
        777
    );
    assert_eq!(
        slow.controller_session(sa.id).unwrap().phase,
        SessionPhase::Active
    );
    assert_eq!(
        fast.controller_session(sb.id).unwrap().phase,
        SessionPhase::Active
    );
    slow.shutdown();
    fast.shutdown();
    host.shutdown();
}

/// CP02 combined scenario: one host serves four distinct paired peers at
/// once — a fast legacy viewer, a slow legacy viewer that never reads, a
/// files-only peer running an upload job, and a services peer with two
/// independent shells. Mid-run the terminal peer is revoked and the slow
/// viewer reconnects; the fast viewer streams uninterrupted throughout, and
/// a global host stop tears every remaining session down.
#[cfg(unix)]
#[test]
fn four_peers_share_one_host_through_slow_revoke_and_reconnect() {
    use crate::job::JobState;
    use crate::terminal::{NativeTerminalFactory, TerminalEvent, TerminalSize};
    const SIZE: TerminalSize = TerminalSize {
        rows: 24,
        columns: 80,
    };

    let host = start_direct();
    let fast = start_direct();
    let slow = start_direct();
    let files = start_direct();
    let term = start_direct();
    host.set_desktop(Box::new(RecordingDesktop {
        events: Arc::new(Mutex::new(Vec::new())),
    }))
    .unwrap();
    host.set_capture(Box::new(Capture)).unwrap();
    host.set_terminal_factory(Some(Arc::new(
        NativeTerminalFactory::new("/bin/sh".into(), std::env::temp_dir()).unwrap(),
    )))
    .unwrap();

    // File roots for the jobs leg; small fixtures need no free-space margin.
    let base = std::env::temp_dir().join(format!(
        "vnilink-multi-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    for (core, name) in [(&host, "host"), (&files, "files")] {
        let root = base.join(name).join("root");
        std::fs::create_dir_all(&root).unwrap();
        core.set_transfer_directory(base.join(name).join("blobs"))
            .unwrap();
        core.set_file_roots(vec![root]).unwrap();
        core.set_file_resource_policy(crate::job::FileResourcePolicy {
            minimum_free_bytes: 0,
            ..Default::default()
        })
        .unwrap();
    }
    std::fs::write(base.join("files/root/note.txt"), b"combined-scenario").unwrap();

    // Viewers use session-model connections (View grant admits); the
    // services peers connect neutrally and authorize per feature.
    for peer in [&fast, &slow, &files, &term] {
        pair_for_control(&host, peer);
    }
    host.set_desktop_sharing(Grants {
        view: true,
        transfer: true,
        ..Grants::default()
    })
    .unwrap();
    files
        .set_desktop_sharing(Grants {
            transfer: true,
            ..Grants::default()
        })
        .unwrap();
    host.set_service_sharing(ServiceKind::Terminal, true)
        .unwrap();

    let sa = active(&fast, fast.begin_connect(&host.endpoint_id()).unwrap().id);
    let sb = active(&slow, slow.begin_connect(&host.endpoint_id()).unwrap().id);
    let sc = active(
        &files,
        files.begin_connect_files(&host.endpoint_id()).unwrap().id,
    );
    let sd = active(
        &term,
        term.begin_connect_services(&host.endpoint_id()).unwrap().id,
    );
    assert_eq!(host.hosted_sessions().unwrap().len(), 4);

    // Two viewers: only the fast one reads. The slow subscription stays
    // unread while its outbox floods past the 8-frame bound.
    let va = fast.subscribe_session_video(sa.id, sa.generation).unwrap();
    let _vb = slow.subscribe_session_video(sb.id, sb.generation).unwrap();
    let hosted_id = |peer: &Core| {
        host.hosted_sessions()
            .unwrap()
            .into_iter()
            .find(|s| s.peer_id == peer.endpoint_id())
            .unwrap()
            .id
    };
    let ha = hosted_id(&fast);
    let hb = hosted_id(&slow);
    host.publish_video(hb, EncodedFrame::h264(true, 1, vec![7]).unwrap())
        .unwrap();
    for stamp in 2..=41u64 {
        host.publish_video(hb, EncodedFrame::h264(false, stamp, vec![7]).unwrap())
            .unwrap();
    }

    // Two independent shells on the services peer: distinct sessions and
    // distinct host processes.
    let open_shell = || {
        let owner = term
            .attach_controller_service(sd.id, sd.generation, ServiceKind::Terminal)
            .unwrap();
        term.open_terminal_service(owner.id, owner.generation, SIZE)
            .unwrap()
    };
    let sh_first = open_shell();
    let sh_second = open_shell();
    assert_ne!(sh_first.session().id, sh_second.session().id);
    assert_ne!(
        sh_first.session().process_id,
        sh_second.session().process_id
    );

    // One files job, driven to completion while video floods.
    let job = files
        .job_submit_upload(&host.endpoint_id(), 0, "", 0, "arrived")
        .unwrap();
    let host_jobs = host.job_list().unwrap();
    assert_eq!(host_jobs.len(), 1);
    let host_job = host_jobs[0].id;
    let mut sender = files.job_snapshot(job).unwrap();
    let mut receiver = host.job_snapshot(host_job).unwrap();
    for _ in 0..200 {
        if !receiver.state.is_terminal() {
            receiver = host.job_poll(host_job).unwrap();
        }
        if !sender.state.is_terminal() {
            sender = files.job_poll(job).unwrap();
        }
        if sender.state.is_terminal() && receiver.state.is_terminal() {
            break;
        }
    }
    assert_eq!(sender.state, JobState::Complete);
    assert_eq!(receiver.state, JobState::Complete);
    assert_eq!(
        std::fs::read(base.join("host/root/arrived/note.txt")).unwrap(),
        b"combined-scenario"
    );

    // Both shells answer while the slow viewer floods.
    let drain = |shell: &crate::RemoteTerminalService, needle: &str| {
        let mut output = Vec::new();
        wait(|| {
            while let Some(event) = shell.poll() {
                match event {
                    TerminalEvent::Output(data) => output.extend(data),
                    TerminalEvent::Failed(error) => panic!("{error}"),
                    _ => {}
                }
            }
            String::from_utf8_lossy(&output).contains(needle)
        });
    };
    sh_first.input(b"printf 'FIRST_ALIVE\\n'\n").unwrap();
    drain(&sh_first, "FIRST_ALIVE");
    sh_second.input(b"printf 'SECOND_ALIVE\\n'\n").unwrap();
    drain(&sh_second, "SECOND_ALIVE");

    // Revoke the terminal peer mid-run: both shells close, the viewers and
    // the finished job lane continue.
    host.set_service_sharing(ServiceKind::Terminal, false)
        .unwrap();
    wait(|| sh_first.is_cancelled() && sh_second.is_cancelled());
    host.publish_video(ha, EncodedFrame::h264(true, 777, vec![9]).unwrap())
        .unwrap();
    assert_eq!(
        va.recv_timeout(Duration::from_secs(3))
            .unwrap()
            .timestamp_us,
        777
    );

    // Reconnect the slow viewer; the fast viewer never notices.
    slow.drop_controller_link(sb.id).unwrap();
    wait(|| {
        slow.controller_session(sb.id).unwrap().phase == SessionPhase::Active
            && slow.controller_session(sb.id).unwrap().generation > sb.generation
    });
    let sb2 = slow.controller_session(sb.id).unwrap();
    assert_eq!(sb2.id, sb.id);
    host.publish_video(ha, EncodedFrame::h264(true, 778, vec![9]).unwrap())
        .unwrap();
    assert_eq!(
        va.recv_timeout(Duration::from_secs(3))
            .unwrap()
            .timestamp_us,
        778
    );
    let vb2 = slow
        .subscribe_session_video(sb2.id, sb2.generation)
        .unwrap();
    let hb2 = hosted_id(&slow);
    host.publish_video(hb2, EncodedFrame::h264(true, 779, vec![8]).unwrap())
        .unwrap();
    assert_eq!(
        vb2.recv_timeout(Duration::from_secs(3))
            .unwrap()
            .timestamp_us,
        779
    );

    // Global stop tears everything down: no session stays Active once the
    // host is gone — redial arms as Reconnecting, never as usable — and
    // every record disconnects and releases from the controller side.
    host.shutdown();
    let legs = [
        (&fast, sa.id),
        (&slow, sb.id),
        (&files, sc.id),
        (&term, sd.id),
    ];
    for (core, id) in &legs {
        wait(|| {
            !matches!(
                core.controller_session(*id).unwrap().phase,
                SessionPhase::Active
            )
        });
    }
    for (core, id) in &legs {
        // Already-Closed legs report NoSession; anything else must stop.
        match core.disconnect_session(*id) {
            Ok(()) | Err(Error::NoSession) => {}
            Err(error) => panic!("disconnect after host stop failed: {error}"),
        }
        wait(|| match core.controller_session(*id) {
            Err(Error::NoSession) => true,
            Ok(session) => session.phase == SessionPhase::Closed,
            Err(_) => false,
        });
        let _ = core.forget_controller_session(*id);
        assert!(core.controller_sessions().unwrap().is_empty());
    }
    fast.shutdown();
    slow.shutdown();
    files.shutdown();
    term.shutdown();
    std::fs::remove_dir_all(&base).unwrap();
}

#[test]
fn review_stop_hosting_stops_every_peer() {
    let host = start_direct();
    let a = start_direct();
    let b = start_direct();
    pair_for_control(&host, &a);
    pair_for_control(&host, &b);
    for peer in [&a, &b] {
        let pending = peer.begin_connect_services(&host.endpoint_id()).unwrap();
        active(peer, pending.id);
    }
    wait(|| {
        host.hosted_sessions()
            .unwrap()
            .iter()
            .filter(|s| s.phase == SessionPhase::Active)
            .count()
            == 2
    });
    host.stop_hosting().unwrap();
    // Stopping must fence every peer immediately; transport may close later.
    assert_eq!(
        host.hosted_sessions()
            .unwrap()
            .iter()
            .filter(|s| s.phase == SessionPhase::Active)
            .count(),
        0,
        "hosting disabled while another peer remains active"
    );
}

#[test]
fn stop_hosting_cancels_live_owners_and_refuses_new_sessions() {
    let host = start_direct();
    let a = start_direct();
    let b = start_direct();
    host.set_capture(Box::new(Capture)).unwrap();
    pair_for_control(&host, &a);
    pair_for_control(&host, &b);
    host.set_desktop_sharing(Grants {
        view: true,
        ..Grants::default()
    })
    .unwrap();
    let mut owners = Vec::new();
    for peer in [&a, &b] {
        let pending = peer.begin_connect_services(&host.endpoint_id()).unwrap();
        active(peer, pending.id);
        let session = peer.controller_session(pending.id).unwrap();
        let owner = peer
            .attach_controller_service(session.id, session.generation, ServiceKind::Desktop)
            .unwrap();
        owners.push(
            peer.open_desktop_service(owner.id, owner.generation)
                .unwrap(),
        );
    }
    host.stop_hosting().unwrap();
    for owner in &owners {
        wait(|| owner.is_cancelled());
    }
    assert!(matches!(
        a.connect(&host.endpoint_id()),
        Err(Error::NotHosting)
    ));
    host.shutdown();
    a.shutdown();
    b.shutdown();
}
