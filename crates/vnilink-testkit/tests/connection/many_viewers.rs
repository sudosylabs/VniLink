//! One host, several peers at once: each has its own connection, a viewer
//! that never reads slows no other, and what ends one peer's connection
//! leaves the others'. Which desktop holds control is the desktop suite's
//! (`desktop/control.rs`).

use expect_test::expect;
use std::time::Duration;

use vnilink::{Error, Grants, RemoteDesktopService, ServiceInfo, ServiceKind, SessionPhase};
use vnilink_testkit::desktop::{delta, keyframe, InputLog};
use vnilink_testkit::{traced, wait_for, Engine, TestResult, DEFAULT_WAIT};

/// `viewer`'s Desktop on a services connection to `host`, its pictures, and
/// the host's side of it.
fn desktop_for(
    viewer: &Engine,
    host: &Engine,
) -> Result<
    (
        RemoteDesktopService,
        vnilink::VideoSubscription,
        ServiceInfo,
    ),
    Error,
> {
    let connection = viewer.connect_services(host)?;
    let desktop = viewer.open_desktop(&connection)?;
    let video = desktop.subscribe_video()?;
    let remote = host.serving(ServiceKind::Desktop, viewer);
    Ok((desktop, video, remote))
}

/// The monitor of the desktops these scenarios install.
const MONITOR: (u16, u16) = (1920, 1080);

/// Two viewers on services connections and a files peer are hosted at once
/// with their own records, only the viewers' Desktops consuming the
/// capture, and the files peer works alongside. Another peer joining and
/// leaving, a viewer leaving, and revoking a peer each end only that peer's
/// connection.
#[test]
fn concurrent_peers_operate_independently() -> TestResult {
    let trace = traced();
    let host = Engine::start("host");
    let viewers = [Engine::start("viewer-a"), Engine::start("viewer-b")];
    let worker = Engine::start("worker");
    let extra = Engine::start("extra");
    for peer in viewers.iter().chain([&worker, &extra]) {
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

    let (_da, _va, _) = desktop_for(&viewers[0], &host)?;
    let (_db, _vb, _) = desktop_for(&viewers[1], &host)?;
    let vb = viewers[1]
        .core
        .controller_sessions()?
        .into_iter()
        .next()
        .ok_or("viewer b's connection")?;
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
    let mut target_ids: Vec<u64> = host
        .core
        .video_targets()?
        .iter()
        .map(|target| target.id)
        .collect();
    target_ids.sort_unstable();
    let desktops = host.hosted(Some(ServiceKind::Desktop));
    let mut desktop_ids: Vec<u64> = desktops.iter().map(|service| service.id).collect();
    desktop_ids.sort_unstable();
    assert_eq!(
        target_ids, desktop_ids,
        "the viewers' Desktops, and no files session, consume the capture"
    );
    assert!(
        desktops
            .iter()
            .all(|service| service.peer_id != worker.endpoint()),
        "the files peer consumes no capture"
    );

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

    let joined = extra.connect_services(&host)?;
    assert_eq!(
        joined.phase,
        SessionPhase::Active,
        "a fourth peer is admitted"
    );
    host.wait_hosted(4);
    extra.core.disconnect()?;
    host.wait_hosted(3);

    viewers[0].core.disconnect()?;
    host.wait_hosted(2);

    host.core.revoke(&worker.endpoint())?;
    host.wait_hosted(1);
    assert_eq!(
        viewers[1].core.controller_session(vb.id)?.phase,
        SessionPhase::Active,
        "revoking one peer leaves another's connection"
    );
    assert_eq!(
        host.core.hosted_sessions()?[0].peer_id,
        viewers[1].endpoint(),
        "only the revoked peer's session ended"
    );
    expect![""].assert_eq(&trace.story(&["input"]));
    Ok(())
}

/// With each engine hosting the other, an engine's handles stay unique
/// across its two roles.
#[test]
fn handles_are_unique_across_roles_with_both_directions_active() -> TestResult {
    let _trace = traced();
    let a = Engine::start("a");
    let b = Engine::start("b");
    a.pair_for_control(&b)?;
    b.pair_for_control(&a)?;
    let b_out = b.connect_services(&a)?;
    let a_out = a.connect_services(&b)?;
    let a_hosted = wait_for("a to host b", DEFAULT_WAIT, || a.hosted_session(&b));
    let b_hosted = wait_for("b to host a", DEFAULT_WAIT, || b.hosted_session(&a));
    assert_ne!(
        a_hosted.id, a_out.id,
        "an engine's handles are unique across roles"
    );
    assert_ne!(
        b_hosted.id, b_out.id,
        "an engine's handles are unique across roles"
    );
    Ok(())
}

/// A viewer that never reads its video, flooded past its outbox, does not
/// delay a fresh keyframe to another viewer, and neither connection ends.
#[test]
fn a_saturated_viewer_does_not_slow_another_viewer() -> TestResult {
    let _trace = traced();
    let host = Engine::start("host");
    let slow = Engine::start("slow");
    let fast = Engine::start("fast");
    InputLog::install(&host, MONITOR)?;
    for viewer in [&slow, &fast] {
        host.pair_for_control(viewer)?;
        host.core.set_desktop_sharing(Grants {
            view: true,
            ..Grants::default()
        })?;
    }
    let sa = slow.connect_services(&host)?;
    let _slow_desktop = slow.open_desktop(&sa)?;
    let ha = host.serving(ServiceKind::Desktop, &slow);
    // Only the fast viewer subscribes; the slow viewer never reads video.
    let (_fast_desktop, vb, hb) = desktop_for(&fast, &host)?;
    let sb = fast
        .core
        .controller_sessions()?
        .into_iter()
        .next()
        .ok_or("the fast viewer's connection")?;
    assert_eq!(
        host.core.hosted_sessions()?.len(),
        2,
        "both viewers are hosted"
    );
    // Past the slow viewer's outbox of eight frames.
    host.core
        .publish_desktop_video(ha.id, ha.generation, keyframe(1, vec![7]))?;
    for stamp in 2..=41 {
        host.core
            .publish_desktop_video(ha.id, ha.generation, delta(stamp, vec![7]))?;
    }
    host.core
        .publish_desktop_video(hb.id, hb.generation, keyframe(777, vec![9]))?;
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
/// slow viewer reconnecting, and opening its Desktop again, goes unnoticed
/// by the fast one; the host stopping then leaves no peer's session
/// active, and each is closed and forgotten from its side.
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
        vnilink_testkit::desktop::MonitorCapture::new((1920, 1080)).separate_cursor(true),
    ))?;
    host.core
        .set_terminal_factory(Some(shell(&std::env::temp_dir())))?;
    std::fs::write(files.root().join("note.txt"), b"combined-scenario")?;

    // The viewers open Desktops under the view grant; every peer but the
    // files one connects neutrally and is authorized per feature.
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

    let sa = fast.connect_services(&host)?;
    let sb = slow.connect_services(&host)?;
    let sc = files.active(files.core.begin_connect_files(&host.endpoint())?.id);
    let sd = term.connect_services(&host)?;
    assert_eq!(
        host.core.hosted_sessions()?.len(),
        4,
        "four peers are hosted"
    );

    // Only the fast viewer reads; the slow one's outbox floods past eight
    // frames.
    let fast_desktop = fast.open_desktop(&sa)?;
    let va = fast_desktop.subscribe_video()?;
    let slow_desktop = slow.open_desktop(&sb)?;
    let _vb = slow_desktop.subscribe_video()?;
    let ha = host.serving(ServiceKind::Desktop, &fast);
    let hb = host.serving(ServiceKind::Desktop, &slow);
    host.core
        .publish_desktop_video(hb.id, hb.generation, keyframe(1, vec![7]))?;
    for stamp in 2..=41u64 {
        host.core
            .publish_desktop_video(hb.id, hb.generation, delta(stamp, vec![7]))?;
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
    host.core
        .publish_desktop_video(ha.id, ha.generation, keyframe(777, vec![9]))?;
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
    host.core
        .publish_desktop_video(ha.id, ha.generation, keyframe(778, vec![9]))?;
    assert_eq!(
        va.recv_timeout(Duration::from_secs(3))
            .ok_or("the fast viewer's keyframe within 3 s")?
            .timestamp_us,
        778,
        "the slow viewer reconnecting leaves the fast viewer streaming"
    );
    let owner = slow
        .core
        .controller_services()?
        .into_iter()
        .find(|service| service.kind == ServiceKind::Desktop)
        .ok_or("the slow viewer's Desktop")?;
    assert_eq!(
        owner.generation, sb2.generation,
        "the slow viewer's Desktop follows its connection"
    );
    drop(slow_desktop);
    let slow_desktop = slow.core.open_desktop_service(owner.id, owner.generation)?;
    let vb2 = slow_desktop.subscribe_video()?;
    let hb2 = wait_for(
        "the host to serve the reopened Desktop",
        DEFAULT_WAIT,
        || {
            host.hosted_for(ServiceKind::Desktop, &slow)
                .filter(|service| service.generation == sb2.generation)
        },
    );
    host.core
        .publish_desktop_video(hb2.id, hb2.generation, keyframe(779, vec![8]))?;
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
