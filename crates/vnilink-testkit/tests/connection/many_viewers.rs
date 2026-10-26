//! One host, several peers at once: each has its own session, a viewer
//! that never reads slows no other, and what ends one peer's session leaves
//! the others'. Which desktop holds control is the desktop suite's
//! (`desktop/control.rs`).

use expect_test::expect;
use std::time::Duration;

use vnilink::{Error, Grants, Quality, ServiceKind, SessionPhase, SessionRole};
use vnilink_testkit::desktop::{delta, keyframe, InputLog, MonitorCapture};
use vnilink_testkit::{traced, wait_for, Engine, TestResult, DEFAULT_WAIT};

/// The monitor of the desktops these scenarios install.
const MONITOR: (u16, u16) = (1920, 1080);

/// Two viewers and a files peer are hosted at once with their own records,
/// only the viewers consuming the capture, and the files peer works
/// alongside. A fifth peer is busy, and revoking a peer ends only its
/// session.
#[test]
fn concurrent_peers_operate_independently_within_capacity() -> TestResult {
    let trace = traced();
    let host = Engine::start("host");
    let viewers = [Engine::start("viewer-a"), Engine::start("viewer-b")];
    let worker = Engine::start("worker");
    let extra = [Engine::start("extra-a"), Engine::start("extra-b")];
    for peer in viewers.iter().chain([&worker]).chain(extra.iter()) {
        host.pair_for_control(peer)?;
    }
    InputLog::install(&host, MONITOR)?;
    host.core.set_service_sharing(ServiceKind::Files, true)?;
    host.core.set_desktop_sharing(Grants {
        view: true,
        control: true,
        clipboard: false,
        transfer: true,
    })?;

    viewers[0].core.connect(&host.endpoint())?;
    let vb = viewers[1].core.connect(&host.endpoint())?;
    let files_session = worker.core.connect_files(&host.endpoint())?;
    let hosted = host.core.hosted_sessions()?;
    let mut peers: Vec<String> = hosted.iter().map(|s| s.peer_id.clone()).collect();
    peers.sort();
    let mut expected = vec![
        viewers[0].endpoint(),
        viewers[1].endpoint(),
        worker.endpoint(),
    ];
    expected.sort();
    assert_eq!(peers, expected, "each peer has its own hosted session");
    let targets = host.core.video_targets()?;
    assert_eq!(
        targets.len(),
        2,
        "a files session is not a capture consumer"
    );
    assert!(
        targets.iter().all(|target| !target.service),
        "the viewers' sessions are the capture's targets"
    );
    let target_ids: Vec<u64> = targets.iter().map(|target| target.id).collect();
    for session in &hosted {
        assert_eq!(
            target_ids.contains(&session.id),
            session.peer_id != worker.endpoint(),
            "only the viewers' sessions consume the capture"
        );
    }

    worker.core.fs_mkdir(
        files_session.id,
        files_session.generation,
        host.root_id(),
        "",
        "from-worker",
    )?;
    assert!(
        host.root().join("from-worker").is_dir(),
        "the files peer works alongside the viewers"
    );

    let vd = extra[0].core.connect(&host.endpoint())?;
    assert_eq!(vd.phase, SessionPhase::Active, "a fourth peer is admitted");
    assert_eq!(
        host.core.hosted_sessions()?.len(),
        4,
        "four peers are hosted"
    );
    assert!(
        matches!(extra[1].core.connect(&host.endpoint()), Err(Error::Busy)),
        "a fifth peer is busy"
    );
    assert_eq!(
        host.core.hosted_sessions()?.len(),
        4,
        "the busy peer leaves the live sessions"
    );
    extra[0].core.disconnect()?;
    host.wait_hosted(3);

    viewers[0].core.disconnect()?;
    host.wait_hosted(2);

    host.core.revoke(&worker.endpoint())?;
    host.wait_hosted(1);
    assert_eq!(
        viewers[1].core.controller_session(vb.id)?.phase,
        SessionPhase::Active,
        "revoking one peer leaves another's session"
    );
    assert_eq!(
        host.core.hosted_sessions()?[0].peer_id,
        viewers[1].endpoint(),
        "only the revoked peer's session ended"
    );
    expect![""].assert_eq(&trace.story(&["input"]));
    Ok(())
}

/// With each engine hosting the other, a quality change for one role
/// changes only that direction's session.
#[test]
fn quality_updates_select_the_role_with_both_directions_active() -> TestResult {
    let _trace = traced();
    let a = Engine::start("a");
    let b = Engine::start("b");
    a.pair_for_control(&b)?;
    b.pair_for_control(&a)?;
    let b_out = b.core.connect(&a.endpoint())?;
    let a_out = a.core.connect(&b.endpoint())?;
    let a_host_id = a.core.snapshot()?.incoming.ok_or("a hosts b")?.id;
    assert_ne!(
        a_host_id, a_out.id,
        "an engine's handles are unique across roles"
    );
    assert_ne!(
        b.core.snapshot()?.incoming.ok_or("b hosts a")?.id,
        b_out.id,
        "an engine's handles are unique across roles"
    );

    let controller_quality = Quality::new(1_500, 30, 1280, 720)?;
    a.core
        .update_session_quality(a_out.id, SessionRole::Controller, controller_quality)?;
    let snapshot = a.core.snapshot()?;
    assert_eq!(
        snapshot.outgoing.ok_or("a controls b")?.quality,
        Some(controller_quality),
        "the controller role's quality changes the outgoing session"
    );
    assert_ne!(
        snapshot.incoming.ok_or("a hosts b")?.quality,
        Some(controller_quality),
        "the hosted session keeps the negotiated limits"
    );

    let host_quality = Quality::new(800, 15, 640, 480)?;
    a.core
        .update_session_quality(a_host_id, SessionRole::Host, host_quality)?;
    let snapshot = a.core.snapshot()?;
    assert_eq!(
        snapshot.incoming.ok_or("a hosts b")?.quality,
        Some(host_quality),
        "the host role's quality changes the hosted session"
    );
    assert_eq!(
        snapshot.outgoing.ok_or("a controls b")?.quality,
        Some(controller_quality),
        "the outgoing session keeps its quality"
    );
    Ok(())
}

/// A viewer that never reads its video, flooded past its outbox, does not
/// delay a fresh keyframe to another viewer, and neither session ends.
#[test]
fn a_saturated_viewer_does_not_slow_another_viewer() -> TestResult {
    let _trace = traced();
    let host = Engine::start("host");
    let slow = Engine::start("slow");
    let fast = Engine::start("fast");
    for viewer in [&slow, &fast] {
        host.pair_for_control(viewer)?;
        host.core.set_desktop_sharing(Grants {
            view: true,
            ..Grants::default()
        })?;
    }
    let sa = slow.connect_control(&host)?;
    let sb = fast.connect_control(&host)?;
    // Only the fast viewer subscribes; the slow viewer never reads video.
    let vb = fast.core.subscribe_session_video(sb.id, sb.generation)?;
    assert_eq!(
        host.core.hosted_sessions()?.len(),
        2,
        "both viewers are hosted"
    );
    let ha = host.hosted_id(&slow);
    let hb = host.hosted_id(&fast);
    // Past the slow viewer's outbox of eight frames.
    host.core.publish_video(ha, keyframe(1, vec![7]))?;
    for stamp in 2..=41 {
        host.core.publish_video(ha, delta(stamp, vec![7]))?;
    }
    host.core.publish_video(hb, keyframe(777, vec![9]))?;
    let fresh = vb
        .recv_timeout(Duration::from_secs(3))
        .ok_or("the fast viewer's keyframe within 3 s")?;
    assert_eq!(
        fresh.timestamp_us, 777,
        "the fast viewer gets its fresh keyframe promptly"
    );
    assert_eq!(
        slow.core.controller_session(sa.id)?.phase,
        SessionPhase::Active,
        "the flooded viewer stays connected"
    );
    assert_eq!(
        fast.core.controller_session(sb.id)?.phase,
        SessionPhase::Active,
        "the fast viewer stays connected"
    );
    Ok(())
}

/// One host serves four peers at once: a viewer that reads, a viewer that
/// never reads, a files peer whose upload completes, and a services peer
/// with two shells. Withdrawing the terminal closes both shells and the
/// slow viewer reconnecting goes unnoticed by the fast one; the host
/// stopping then leaves no peer's session active, and each is closed and
/// forgotten from its side.
#[cfg(unix)]
#[test]
fn four_peers_share_one_host_through_slow_revoke_and_reconnect() -> TestResult {
    use vnilink::terminal::TerminalEvent;
    use vnilink::JobState;
    use vnilink_testkit::terminal::{shell, SIZE};

    let _trace = traced();
    let host = Engine::start("host");
    let fast = Engine::start("fast");
    let slow = Engine::start("slow");
    let files = Engine::start("files");
    let term = Engine::start("terminal");
    InputLog::install(&host, MONITOR)?;
    host.core.set_capture(Box::new(
        MonitorCapture::new((1920, 1080)).separate_cursor(true),
    ))?;
    host.core
        .set_terminal_factory(Some(shell(&std::env::temp_dir())))?;
    std::fs::write(files.root().join("note.txt"), b"combined-scenario")?;

    // The viewers open sessions under the view grant; the services peers
    // connect neutrally and are authorized per feature.
    for peer in [&fast, &slow, &files, &term] {
        host.pair_for_control(peer)?;
    }
    host.core.set_desktop_sharing(Grants {
        view: true,
        transfer: true,
        ..Grants::default()
    })?;
    files.core.set_desktop_sharing(Grants {
        transfer: true,
        ..Grants::default()
    })?;
    host.core.set_service_sharing(ServiceKind::Terminal, true)?;

    let sa = fast.connect_control(&host)?;
    let sb = slow.connect_control(&host)?;
    let sc = files.active(files.core.begin_connect_files(&host.endpoint())?.id);
    let sd = term.connect_services(&host)?;
    assert_eq!(
        host.core.hosted_sessions()?.len(),
        4,
        "four peers are hosted"
    );

    // Only the fast viewer reads; the slow one's outbox floods past eight
    // frames.
    let va = fast.core.subscribe_session_video(sa.id, sa.generation)?;
    let _vb = slow.core.subscribe_session_video(sb.id, sb.generation)?;
    let ha = host.hosted_id(&fast);
    let hb = host.hosted_id(&slow);
    host.core.publish_video(hb, keyframe(1, vec![7]))?;
    for stamp in 2..=41u64 {
        host.core.publish_video(hb, delta(stamp, vec![7]))?;
    }

    let open_shell = || -> Result<vnilink::RemoteTerminalService, Error> {
        let owner =
            term.core
                .attach_controller_service(sd.id, sd.generation, ServiceKind::Terminal)?;
        term.core
            .open_terminal_service(owner.id, owner.generation, SIZE)
    };
    let first = open_shell()?;
    let second = open_shell()?;
    assert_ne!(
        first.session().id,
        second.session().id,
        "two shells are two sessions"
    );
    assert_ne!(
        first.session().process_id,
        second.session().process_id,
        "two shells are two host processes"
    );

    // The upload, driven to completion within 200 polls while video floods.
    let job = files.core.job_submit_upload(
        &host.endpoint(),
        files.root_id(),
        "",
        host.root_id(),
        "arrived",
    )?;
    let host_jobs = host.core.job_list()?;
    assert_eq!(host_jobs.len(), 1, "the host has the upload");
    let host_job = host_jobs[0].id;
    let mut sender = files.core.job_snapshot(job)?;
    let mut receiver = host.core.job_snapshot(host_job)?;
    for _ in 0..200 {
        if !receiver.state.is_terminal() {
            receiver = host.core.job_poll(host_job)?;
        }
        if !sender.state.is_terminal() {
            sender = files.core.job_poll(job)?;
        }
        if sender.state.is_terminal() && receiver.state.is_terminal() {
            break;
        }
    }
    assert_eq!(
        (sender.state, receiver.state),
        (JobState::Complete, JobState::Complete),
        "the upload completes within 200 polls"
    );
    assert_eq!(
        std::fs::read(host.root().join("arrived/note.txt"))?,
        b"combined-scenario",
        "the uploaded file lands whole on the host"
    );

    let answers = |shell: &vnilink::RemoteTerminalService, needle: &str| {
        let mut output = Vec::new();
        wait_for(
            &format!("the shell to print {needle}"),
            DEFAULT_WAIT,
            || {
                while let Some(event) = shell.poll() {
                    match event {
                        TerminalEvent::Output(data) => output.extend(data),
                        TerminalEvent::Failed(error) => panic!("{error}"),
                        _ => {}
                    }
                }
                String::from_utf8_lossy(&output)
                    .contains(needle)
                    .then_some(())
            },
        );
    };
    first.input(b"printf 'FIRST_ALIVE\\n'\n")?;
    answers(&first, "FIRST_ALIVE");
    second.input(b"printf 'SECOND_ALIVE\\n'\n")?;
    answers(&second, "SECOND_ALIVE");

    host.core
        .set_service_sharing(ServiceKind::Terminal, false)?;
    wait_for("both shells to close", DEFAULT_WAIT, || {
        (first.is_cancelled() && second.is_cancelled()).then_some(())
    });
    host.core.publish_video(ha, keyframe(777, vec![9]))?;
    assert_eq!(
        va.recv_timeout(Duration::from_secs(3))
            .ok_or("the fast viewer's keyframe within 3 s")?
            .timestamp_us,
        777,
        "withdrawing the terminal leaves the fast viewer streaming"
    );

    slow.core.drop_controller_link(sb.id)?;
    let sb2 = wait_for("the slow viewer to reconnect", DEFAULT_WAIT, || {
        slow.core
            .controller_session(sb.id)
            .ok()
            .filter(|s| s.phase == SessionPhase::Active && s.generation > sb.generation)
    });
    assert_eq!(sb2.id, sb.id, "a reconnect keeps the session's handle");
    host.core.publish_video(ha, keyframe(778, vec![9]))?;
    assert_eq!(
        va.recv_timeout(Duration::from_secs(3))
            .ok_or("the fast viewer's keyframe within 3 s")?
            .timestamp_us,
        778,
        "the slow viewer reconnecting leaves the fast viewer streaming"
    );
    let vb2 = slow.core.subscribe_session_video(sb2.id, sb2.generation)?;
    host.core
        .publish_video(host.hosted_id(&slow), keyframe(779, vec![8]))?;
    assert_eq!(
        vb2.recv_timeout(Duration::from_secs(3))
            .ok_or("the reconnected viewer's keyframe within 3 s")?
            .timestamp_us,
        779,
        "the reconnected viewer streams"
    );

    // Once the host is gone no session stays active: a redial arms as
    // reconnecting, never as usable, and each side closes and forgets it.
    host.core.shutdown();
    let legs = [
        (&fast, sa.id),
        (&slow, sb.id),
        (&files, sc.id),
        (&term, sd.id),
    ];
    for (peer, id) in legs {
        wait_for("the peer's session to leave active", DEFAULT_WAIT, || {
            (peer.core.controller_session(id).ok()?.phase != SessionPhase::Active).then_some(())
        });
    }
    for (peer, id) in legs {
        match peer.core.disconnect_session(id) {
            Ok(()) | Err(Error::NoSession) => {}
            Err(error) => panic!("disconnect after host stop failed: {error}"),
        }
        wait_for("the peer's session to close", DEFAULT_WAIT, || {
            match peer.core.controller_session(id) {
                Err(Error::NoSession) => Some(()),
                Ok(session) => (session.phase == SessionPhase::Closed).then_some(()),
                Err(_) => None,
            }
        });
        let _ = peer.core.forget_controller_session(id);
        assert!(
            peer.core.controller_sessions()?.is_empty(),
            "a closed session is forgotten"
        );
    }
    Ok(())
}
