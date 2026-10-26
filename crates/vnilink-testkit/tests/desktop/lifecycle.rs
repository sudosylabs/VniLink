//! Opening, reopening, reconnecting and closing desktops over one connection
//! or several, each owner fenced from the others.
//!
//! These scenarios publish the host's pictures by hand
//! (`publish_desktop_video`), so each picture is one the test named, and
//! several engines take part where one viewer's close must leave another's
//! desktop alone.

use expect_test::expect;

use vnilink::{usage, Error, Grants, InputEvent, PointerButton, ServiceKind, SessionPhase};
use vnilink_testkit::desktop::{keyframe, next_picture, InputLog, MonitorCapture, SCREEN};
use vnilink_testkit::{traced, wait_for, Engine, Pair, TestResult, DEFAULT_WAIT};

/// What a host shares with the controllers these scenarios pair: it may be
/// seen and controlled.
const SEEN_AND_CONTROLLED: Grants = Grants {
    view: true,
    control: true,
    clipboard: false,
    transfer: false,
};

/// `host` and `controller` trust each other as two paired computers do, the
/// host serving and sharing itself to be seen and controlled. [`Pair`] does
/// this for one host and one controller; these scenarios pair several.
pub(super) fn pair(host: &Engine, controller: &Engine) -> Result<(), Error> {
    host.pair_with(controller)?;
    host.core.set_desktop_sharing(SEEN_AND_CONTROLLED)
}

/// A Desktop service opens only while the host shares Desktop, and a
/// connection outlives its desktops: one closed and another opened on the
/// same connection are fenced from each other, and each releases the input
/// it held when it releases, closes or is withdrawn.
#[test]
fn a_desktop_reopens_on_its_connection_and_releases_held_input_each_time() -> TestResult {
    let trace = traced();
    let pair = Pair::for_desktop();
    let (host, controller) = (&pair.host.core, &pair.controller.core);
    let applied = InputLog::install(&pair.host, SCREEN)?;
    host.set_service_sharing(ServiceKind::Desktop, false)?;
    let session = pair.connect_services();
    assert!(
        controller
            .remote_service_support(session.id, session.generation)?
            .desktop,
        "the host supports Desktop while it does not share it"
    );
    let owner = pair.attach(&session, ServiceKind::Desktop);
    assert!(
        matches!(
            controller.open_desktop_service(owner.id, owner.generation),
            Err(Error::Unauthorized)
        ),
        "a Desktop the host does not share does not open"
    );
    assert!(
        host.hosted_services()?.is_empty(),
        "a refused Desktop leaves the host serving nothing"
    );

    host.set_service_sharing(ServiceKind::Desktop, true)?;
    let first = controller.open_desktop_service(owner.id, owner.generation)?;
    let remote = pair
        .host
        .hosted_for(ServiceKind::Desktop, &pair.controller)
        .ok_or("the host serves the Desktop")?;
    let info = first.desktop_info()?;
    assert_eq!(
        (info.monitor_width, info.monitor_height),
        SCREEN,
        "the desktop learns the host's monitor"
    );
    assert!(
        controller.controller_session(session.id)?.quality.is_none(),
        "a services connection carries no picture quality of its own"
    );
    let incoming = host.snapshot()?.incoming.ok_or("the host's connection")?;
    assert!(
        !host.host_desktop_active(incoming.id, remote.generation)?,
        "a Desktop service does not make the connection a desktop session"
    );
    let video = first.subscribe_video()?;
    assert!(
        matches!(first.subscribe_video(), Err(Error::Busy)),
        "a desktop has one picture subscriber"
    );
    host.publish_desktop_video(remote.id, remote.generation, keyframe(1, vec![1]))?;
    assert_eq!(
        next_picture(&video, "the first desktop's picture").payload,
        vec![1],
        "the first desktop shows the host's picture"
    );

    let key = InputEvent::key_down(usage::A);
    let button = InputEvent::button_down(PointerButton::Left);
    first.submit_input(key)?;
    first.submit_input(button)?;
    applied.wait_for(2);
    first.release_input()?;
    assert_eq!(
        applied.wait_for(4),
        vec![
            key,
            button,
            InputEvent::button_up(PointerButton::Left),
            InputEvent::key_up(usage::A),
        ],
        "releasing input lifts what was held, last pressed first"
    );
    first.submit_input(key)?;
    applied.exactly(5);
    first.close();
    pair.host.wait_released(Some(ServiceKind::Desktop));
    assert_eq!(
        applied.exactly(6).last(),
        Some(&InputEvent::key_up(usage::A)),
        "closing a desktop lifts the key it held"
    );
    assert!(
        first.submit_input(key).is_err(),
        "a closed desktop sends no input"
    );
    assert!(
        video.try_recv().is_none(),
        "a closed desktop shows no picture"
    );
    assert!(
        host.publish_desktop_video(remote.id, remote.generation, keyframe(2, vec![2]))
            .is_err(),
        "the host cannot publish to a closed desktop"
    );

    let next = pair.attach(&session, ServiceKind::Desktop);
    let second = controller.open_desktop_service(next.id, next.generation)?;
    let new_remote = pair
        .host
        .hosted_for(ServiceKind::Desktop, &pair.controller)
        .ok_or("the host serves the Desktop")?;
    assert_ne!(
        new_remote.id, remote.id,
        "a reopened desktop is a new service on the host"
    );
    first.close();
    assert!(
        !second.is_cancelled(),
        "closing the old desktop again leaves the new one open"
    );
    let second_video = second.subscribe_video()?;
    host.publish_desktop_video(new_remote.id, new_remote.generation, keyframe(3, vec![3]))?;
    assert_eq!(
        next_picture(&second_video, "the second desktop's picture").payload,
        vec![3],
        "the reopened desktop shows the host's picture"
    );
    assert_eq!(
        controller.controller_session(session.id)?,
        session,
        "reopening a desktop does not reconnect"
    );
    second.submit_input(key)?;
    applied.exactly(7);
    host.set_service_sharing(ServiceKind::Desktop, false)?;
    wait_for("the withdrawn desktop to end", DEFAULT_WAIT, || {
        second.is_cancelled().then_some(())
    });
    assert_eq!(
        applied.exactly(8).last(),
        Some(&InputEvent::key_up(usage::A)),
        "a withdrawn desktop lifts the key it held"
    );
    assert_eq!(
        controller.controller_session(session.id)?,
        session,
        "withdrawing Desktop leaves the connection as it was"
    );
    assert!(
        second_video.try_recv().is_none(),
        "a withdrawn desktop shows no picture"
    );
    trace.wait_for_records("service phase", 4);
    trace.wait_for_records("desktop ended", 2);
    expect![[r#"
        controller:
          service: service phase from=active generation=1 kind=Desktop service=1 to=closed: OwnerClose
          session: desktop ended hosted=false reason=revoked service=2
          service: service phase from=active generation=1 kind=Desktop service=2 to=closed: PeerClose
        host:
          service: service phase from=active generation=1 kind=Desktop service=1 to=closed: PeerClose
          service: service phase from=active generation=1 kind=Desktop service=2 to=closed: Revoked
          session: desktop ended hosted=true reason=revoked service=2
    "#]].assert_eq(&trace.story(&["service", "session"]));
    Ok(())
}

/// One controller watching two computers: closing one computer's desktop
/// leaves the other's streaming, and neither connection changes.
#[test]
fn closing_one_computers_desktop_keeps_another_computer_streaming() -> TestResult {
    let trace = traced();
    let controller = Engine::start("controller");
    let hosts = [Engine::start("host"), Engine::start("other-host")];
    let mut connections = Vec::new();
    let mut desktops = Vec::new();
    let mut videos = Vec::new();
    for host in &hosts {
        InputLog::install(host, SCREEN)?;
        pair(host, &controller)?;
        let connection = controller.connect_services(&host)?;
        let desktop = controller.open_desktop(&connection)?;
        videos.push(desktop.subscribe_video()?);
        desktops.push(desktop);
        connections.push(connection);
    }
    for (index, host) in (0u8..).zip(&hosts) {
        let owner = host
            .hosted_for(ServiceKind::Desktop, &controller)
            .ok_or("the host serves the Desktop")?;
        host.core
            .publish_desktop_video(owner.id, owner.generation, keyframe(1, vec![index]))?;
    }
    for (index, video) in (0u8..).zip(&videos) {
        assert_eq!(
            next_picture(video, "each computer's picture").payload,
            vec![index],
            "each desktop shows its own computer's picture"
        );
    }
    desktops[0].close();
    hosts[0].wait_released(Some(ServiceKind::Desktop));
    assert!(
        !desktops[1].is_cancelled(),
        "closing one computer's desktop leaves the other's open"
    );
    let kept = hosts[1]
        .hosted_for(ServiceKind::Desktop, &controller)
        .ok_or("the other host serves the Desktop")?;
    hosts[1]
        .core
        .publish_desktop_video(kept.id, kept.generation, keyframe(2, vec![9]))?;
    assert_eq!(
        next_picture(&videos[1], "the other computer's next picture").payload,
        vec![9],
        "the other computer keeps streaming"
    );
    for connection in connections {
        assert_eq!(
            controller.core.controller_session(connection.id)?,
            connection,
            "closing a desktop changes no connection"
        );
    }
    trace.wait_for_records("service phase", 2);
    expect![[r#"
        controller:
          service: service phase from=active generation=1 kind=Desktop service=1 to=closed: OwnerClose
        host:
          service: service phase from=active generation=1 kind=Desktop service=1 to=closed: PeerClose
    "#]].assert_eq(&trace.story(&["service"]));
    Ok(())
}

/// A host's capture and desktop may be swapped while its connection carries
/// no desktop; while one is open both are refused, and once it closes they
/// may be swapped again, the connection unchanged.
#[test]
fn adapters_change_while_no_desktop_is_open_and_are_refused_while_one_is() -> TestResult {
    let _trace = traced();
    let pair = Pair::for_desktop();
    let host = &pair.host.core;
    let connection = pair.connect_services();
    host.set_capture(Box::new(MonitorCapture(SCREEN)))?;
    InputLog::install(&pair.host, SCREEN)?;
    let desktop = pair.controller.open_desktop(&connection)?;
    assert!(
        matches!(
            host.set_capture(Box::new(MonitorCapture(SCREEN))),
            Err(Error::Busy)
        ),
        "an open desktop refuses a new capture"
    );
    assert!(
        matches!(InputLog::install(&pair.host, SCREEN), Err(Error::Busy)),
        "an open desktop refuses a new input desktop"
    );
    desktop.close();
    wait_for(
        "the closed desktop to free the input desktop",
        DEFAULT_WAIT,
        || InputLog::install(&pair.host, SCREEN).ok(),
    );
    host.set_capture(Box::new(MonitorCapture(SCREEN)))?;
    assert_eq!(
        pair.controller.core.controller_session(connection.id)?,
        connection,
        "opening and closing a desktop changes no connection"
    );
    Ok(())
}

/// A close addressed to one Desktop service ends that owner alone, from the
/// host's side or the controller's: another controller's desktop keeps
/// streaming, and both connections stay up.
#[test]
fn closing_a_desktop_by_its_address_ends_only_that_owner() -> TestResult {
    let trace = traced();
    let host = Engine::start("host");
    let controllers = [Engine::start("first"), Engine::start("second")];
    InputLog::install(&host, SCREEN)?;
    let mut opened = Vec::new();
    for controller in &controllers {
        pair(&host, controller)?;
        let connection = controller.connect_services(&host)?;
        let owner = controller.core.attach_controller_service(
            connection.id,
            connection.generation,
            ServiceKind::Desktop,
        )?;
        let desktop = controller
            .core
            .open_desktop_service(owner.id, owner.generation)?;
        let remote = host
            .hosted_for(ServiceKind::Desktop, &controller)
            .ok_or("the host serves the Desktop")?;
        let video = desktop.subscribe_video()?;
        opened.push((desktop, owner, remote, connection, video));
    }
    let [(first, _, first_remote, first_connection, first_video), (second, second_owner, second_remote, second_connection, second_video)] =
        &opened[..]
    else {
        unreachable!("two controllers opened two desktops");
    };
    host.core.publish_desktop_video(
        first_remote.id,
        first_remote.generation,
        keyframe(1, vec![31; 64]),
    )?;
    host.core.publish_desktop_video(
        second_remote.id,
        second_remote.generation,
        keyframe(2, vec![32; 64]),
    )?;
    assert_eq!(
        next_picture(first_video, "the first desktop's picture").timestamp_us,
        1,
        "the first desktop shows its own picture"
    );
    assert_eq!(
        next_picture(second_video, "the second desktop's picture").timestamp_us,
        2,
        "the second desktop shows its own picture"
    );

    host.core
        .close_hosted_service(first_remote.id, first_remote.generation)?;
    wait_for(
        "the host's close to end the first desktop",
        DEFAULT_WAIT,
        || first.is_cancelled().then_some(()),
    );
    host.core.publish_desktop_video(
        second_remote.id,
        second_remote.generation,
        keyframe(3, vec![33; 64]),
    )?;
    assert_eq!(
        next_picture(second_video, "the second desktop's next picture").timestamp_us,
        3,
        "the host's addressed close leaves the other desktop streaming"
    );

    controllers[1]
        .core
        .close_controller_service(second_owner.id, second_owner.generation)?;
    wait_for(
        "the controller's close to end the second desktop",
        DEFAULT_WAIT,
        || second.is_cancelled().then_some(()),
    );
    for (controller, connection) in [
        (&controllers[0], first_connection),
        (&controllers[1], second_connection),
    ] {
        assert_eq!(
            controller.core.controller_session(connection.id)?.phase,
            SessionPhase::Active,
            "a service's close leaves its connection up"
        );
    }
    host.wait_released(Some(ServiceKind::Desktop));
    trace.wait_for_records("service phase", 4);
    expect![[r#"
        first:
          service: service phase from=active generation=1 kind=Desktop service=1 to=closed: PeerClose
        host:
          service: service phase from=active generation=1 kind=Desktop service=1 to=closed: OwnerClose
          service: service phase from=active generation=2 kind=Desktop service=2 to=closed: PeerClose
        second:
          service: service phase from=active generation=2 kind=Desktop service=1 to=closed: OwnerClose
    "#]].assert_eq(&trace.story(&["service"]));
    Ok(())
}
