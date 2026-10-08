use super::support::{pair_for_control, start_direct, RecordingDesktop};
use crate::{
    Core, CursorState, EncodedFrame, Error, Grants, HostClipboard, InputEvent, KeyboardMode,
    LocalKey, SessionInfo, SessionPhase, SessionRole,
};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

fn wait(mut ready: impl FnMut() -> bool) {
    let until = Instant::now() + Duration::from_secs(8);
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
        host.authorize(
            &controller.endpoint_id(),
            Grants {
                control: true,
                clipboard: true,
                transfer: false,
            },
        )
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
            .submit_local_key(
                session.id,
                session.generation,
                LocalKey {
                    usage: 4,
                    text: "a",
                    down: true,
                },
            )
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
    b.authorize(&controller.endpoint_id(), Grants::default())
        .unwrap();
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
    b.authorize(
        &controller.endpoint_id(),
        Grants {
            control: true,
            ..Grants::default()
        },
    )
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
        core.set_file_roots(vec![dir.join(name).join("root")])
            .unwrap();
        core.set_blob_directory(dir.join(name).join("blobs"))
            .unwrap();
    }
    std::fs::write(dir.join("a/root/a.txt"), b"A's bytes").unwrap();
    std::fs::write(dir.join("b/root/b.txt"), b"B's bytes").unwrap();
    for host in [&a, &b] {
        pair_for_control(host, &controller);
        host.authorize(
            &controller.endpoint_id(),
            Grants {
                transfer: true,
                ..Grants::default()
            },
        )
        .unwrap();
        controller
            .authorize(
                &host.endpoint_id(),
                Grants {
                    transfer: true,
                    ..Grants::default()
                },
            )
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
        .job_submit_download(&a.endpoint_id(), 0, "a.txt", 0, "")
        .unwrap();
    let jb = controller
        .job_submit_download(&b.endpoint_id(), 0, "b.txt", 0, "")
        .unwrap();
    assert_ne!(ja, jb);
    assert_eq!(a.job_list().unwrap()[0].id, b.job_list().unwrap()[0].id);
    controller.job_cancel(jb).unwrap();
    assert_eq!(b.job_list().unwrap()[0].state, crate::JobState::Cancelled);
    assert_eq!(a.job_list().unwrap()[0].state, crate::JobState::Serving);
    controller.job_resume(jb).unwrap();
    let diagnostic = Instant::now();
    wait(|| {
        assert!(
            diagnostic.elapsed() < Duration::from_secs(7),
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
        .job_submit_download(&a.endpoint_id(), 0, "closed.txt", 0, "")
        .unwrap();
    let open_job = controller
        .job_submit_download(&b.endpoint_id(), 0, "still-open.txt", 0, "")
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
