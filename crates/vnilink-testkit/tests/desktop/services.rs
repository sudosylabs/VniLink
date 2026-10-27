//! A Desktop service among the other services of its connection: its own
//! stream carries pictures and the cursor beside a Files service, and a
//! reconnect replaces it while the old handle stays fenced.

use expect_test::expect;
use std::sync::Arc;

use vnilink::{CursorImage, CursorState, ServiceKind, SessionPhase};
use vnilink_testkit::desktop::{keyframe, next_picture, InputLog, MonitorCapture};
use vnilink_testkit::{traced, wait_for, Pair, TestResult, DEFAULT_WAIT};

use super::streams::SCREEN;

/// A desktop forced onto its own stream carries a large picture and the
/// cursor's image and moves beside it, and asks the host for a keyframe;
/// closing it leaves the connection's Files service working and the
/// connection as it was.
#[test]
fn neutral_desktop_stream_fallback_cursor_and_files_are_independent() -> TestResult {
    let trace = traced();
    let pair = Pair::for_desktop();
    let (host, controller) = (&pair.host.core, &pair.controller.core);
    controller.set_cursor_receiver(true)?;
    host.set_capture(Box::new(MonitorCapture::new(SCREEN).separate_cursor(true)))?;
    let here = pair.host.location();
    let (root, folder) = (here.root, here.path(""));
    host.set_service_sharing(ServiceKind::Files, true)?;
    let connection = pair.connect_services();
    let desktop = pair.controller.open_desktop(&connection)?;
    let (_, files) = pair.controller.open_files(&connection)?;
    let remote = pair
        .host
        .hosted_for(ServiceKind::Desktop, &pair.controller)
        .ok_or("the host serves the Desktop")?;
    host.force_desktop_stream(remote.id, remote.generation)?;
    let video = desktop.subscribe_video()?;
    let cursor = CursorState {
        position: (25, 40),
        visible: true,
        image: Some(Arc::new(CursorImage::new(2, 2, (1, 1), vec![255; 16])?)),
    };
    host.publish_desktop_cursor(remote.id, remote.generation, cursor.clone())?;
    // A large picture shares the stream with the cursor's small messages.
    let payload = vec![42; 200_000];
    host.publish_desktop_video(remote.id, remote.generation, keyframe(1, payload.clone()))?;
    assert_eq!(
        next_picture(&video, "the large picture").payload,
        payload,
        "a large picture arrives whole on the desktop's own stream"
    );
    let shows = |expected: &CursorState| {
        wait_for("the viewer to show the host's cursor", DEFAULT_WAIT, || {
            (desktop.cursor().ok()?.as_deref() == Some(expected)).then_some(())
        })
    };
    shows(&cursor);
    let moved = CursorState {
        position: (50, 60),
        ..cursor
    };
    host.publish_desktop_cursor(remote.id, remote.generation, moved.clone())?;
    shows(&moved);
    desktop.request_keyframe()?;
    wait_for("the host to see the keyframe request", DEFAULT_WAIT, || {
        host.take_desktop_keyframe_request(remote.id, remote.generation)
            .ok()?
            .then_some(())
    });
    desktop.close();
    pair.host.wait_released(Some(ServiceKind::Desktop));
    files.mkdir(root, &folder, "still-connected")?;
    assert!(
        pair.host.folder().join("still-connected").is_dir(),
        "the Files service works after the desktop closed"
    );
    assert!(
        !files.is_cancelled(),
        "closing the desktop leaves the Files service open"
    );
    assert_eq!(
        controller.controller_session(connection.id)?,
        connection,
        "closing the desktop leaves the connection as it was"
    );
    trace.wait_for_records("service phase", 2);
    expect![[r#"
        controller:
          service: service phase from=active generation=1 kind=Desktop service=1 to=closed: OwnerClose
        host:
          service: service phase from=active generation=1 kind=Desktop service=1 to=closed: PeerClose
    "#]].assert_eq(&trace.story(&["service", "stream"]));
    Ok(())
}

/// After the link drops and the connection reconnects, the Desktop owner
/// opens a fresh desktop on the new generation: the old handle's close
/// leaves the fresh one, the fresh one shows the host's pictures, and the
/// old one cannot subscribe to them.
#[test]
fn neutral_desktop_reconnect_replaces_runtime_and_fences_old_handle() -> TestResult {
    let trace = traced();
    let pair = Pair::for_desktop();
    let (host, controller) = (&pair.host.core, &pair.controller.core);
    InputLog::install(&pair.host, SCREEN)?;
    let session = pair.connect_services();
    let owner = pair.attach(&session, ServiceKind::Desktop);
    let old = controller.open_desktop_service(owner.id, owner.generation)?;
    controller.drop_controller_link(session.id)?;
    wait_for("the connection to reconnect", DEFAULT_WAIT, || {
        let now = controller.controller_session(session.id).ok()?;
        (now.phase == SessionPhase::Active && now.generation > session.generation).then_some(())
    });
    let owner = controller.controller_service(owner.id)?;
    let fresh = controller.open_desktop_service(owner.id, owner.generation)?;
    old.close();
    assert!(
        !fresh.is_cancelled(),
        "closing the old handle leaves the fresh desktop"
    );
    let remote = pair
        .host
        .hosted_for(ServiceKind::Desktop, &pair.controller)
        .ok_or("the host serves the fresh Desktop")?;
    let video = fresh.subscribe_video()?;
    host.publish_desktop_video(remote.id, remote.generation, keyframe(4, vec![4]))?;
    assert_eq!(
        next_picture(&video, "the fresh desktop's picture").payload,
        vec![4],
        "the fresh desktop shows the host's picture"
    );
    assert!(
        old.subscribe_video().is_err(),
        "the old handle cannot subscribe to pictures"
    );
    let phases = |engine: &str| {
        trace
            .named("service phase")
            .into_iter()
            .filter(|event| event["engine"] == engine)
            .map(|event| event["to"].as_str().unwrap_or_default().to_owned())
            .collect::<Vec<_>>()
    };
    wait_for("the viewer to resume its Desktop", DEFAULT_WAIT, || {
        (phases("controller").len() == 2).then_some(())
    });
    let ended = wait_for("the host to end the old Desktop", DEFAULT_WAIT, || {
        phases("host").first().cloned()
    });
    // A dropped link is the parent lost, never the peer closing it.
    assert_eq!(
        ended, "closed: ParentLoss",
        "the host ends the old Desktop as lost with its connection"
    );
    // Whether the viewer records the old desktop's end, and why, follows
    // the same race: when it does, it ends once, its link lost.
    let ended: Vec<_> = trace
        .named("desktop ended")
        .into_iter()
        .filter(|event| event["engine"] == "controller")
        .map(|event| event["reason"].clone())
        .collect();
    assert!(
        ended.is_empty() || ended == ["linkLost"],
        "the viewer's old desktop ends at most once, its link lost: {ended:?}"
    );
    expect![[r#"
        controller:
          service: service phase from=active generation=1 kind=Desktop service=1 to=suspended
          service: service phase from=suspended generation=3 kind=Desktop service=1 to=active
    "#]]
    .assert_eq(&trace.engine_story("controller", &["service"]));
    Ok(())
}
