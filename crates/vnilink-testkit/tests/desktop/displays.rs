//! Which displays a viewer sees: monitors selected per connection, and
//! displays that disappear under one viewer or owner but not another.

use expect_test::expect;
use std::time::Duration;

use vnilink::{
    usage, CoreEvent, DesktopControlState, DisplayInfo, Error, EventSubscription, InputEvent,
    Quality, ServiceKind, SessionInfo, SessionPhase, SessionRole,
};
use vnilink_testkit::desktop::{input_failed, keyframe, next_picture, InputLog, MonitorCapture};
use vnilink_testkit::events::next_event;
use vnilink_testkit::{traced, wait_for, Pair, TestResult, DEFAULT_WAIT};

use super::streams::{display, hosted_showing, SCREEN};

/// A display anywhere on the host's desktop, turned and scaled, at
/// revision 1.
fn placed(id: u64, x: i32, (width, height): (u16, u16), rotation: u16, scale: u16) -> DisplayInfo {
    DisplayInfo {
        id,
        origin_x: x,
        origin_y: 0,
        width,
        height,
        rotation_deg: rotation,
        scale_percent: scale,
        revision: 1,
    }
}

/// Whether the host's desktop applied `event`.
fn has_applied(log: &InputLog, event: &InputEvent) -> bool {
    log.events().contains(event)
}

/// Sends `pointer` and then `key`, and says the pointer was refused for
/// `why` on connection `id`: the key applied after it proves the pointer
/// was decided, and a key between pointers keeps each from collapsing into
/// the next.
fn refused_before_key(
    send: &mut impl FnMut(InputEvent),
    applied_events: &InputLog,
    events: &EventSubscription,
    id: u64,
    (pointer, key): (InputEvent, InputEvent),
    why: &str,
) {
    send(pointer);
    send(key);
    wait_for("the following key to apply", DEFAULT_WAIT, || {
        has_applied(applied_events, &key).then_some(())
    });
    assert!(
        !has_applied(applied_events, &pointer),
        "a pointer refused as {why:?} applied"
    );
    next_event(
        events,
        &format!("the host to refuse a pointer: {why}"),
        DEFAULT_WAIT,
        |event| {
            matches!(event, CoreEvent::InputFailed { id: failed, message }
                if *failed == id && *message == why)
        },
    );
}

/// One connection opens two Desktop owners, each showing its own monitor
/// with its own pictures; only one controls, and owners beyond the bound
/// are refused. Removing one monitor faults only its owner, a stale
/// selection on the survivor is refused, and closing one owner spares the
/// other's stream and the connection.
#[test]
fn one_connection_streams_two_selected_monitors_independently() -> TestResult {
    let trace = traced();
    let pair = Pair::for_desktop();
    let (host, controller) = (&pair.host.core, &pair.controller.core);
    host.set_capture(Box::new(MonitorCapture::new(SCREEN)))?;
    host.set_topology(1, &[display(1, 0, 1920, 1), display(2, 1920, 1280, 1)])?;
    let connection = pair.connect_services();
    // A second Desktop owner on the same connection is admitted, not Busy.
    let desktop_a = pair.controller.open_desktop(&connection)?;
    let desktop_b = pair.controller.open_desktop(&connection)?;
    wait_for("both owners to learn the monitors", DEFAULT_WAIT, || {
        (desktop_a.topology().ok()?.len() == 2 && desktop_b.topology().ok()?.len() == 2)
            .then_some(())
    });
    assert!(
        matches!(
            desktop_a.select_displays(&[1, 2]),
            Err(Error::InvalidSetting(_))
        ),
        "an owner shows one display, refused at once otherwise"
    );
    desktop_a.select_displays(&[1])?;
    desktop_b.select_displays(&[2])?;
    let remote_a = hosted_showing(&pair.host, 1);
    let remote_b = hosted_showing(&pair.host, 2);
    let mut targets: Vec<_> = host
        .video_targets()?
        .into_iter()
        .filter(|target| target.service)
        .map(|target| target.display_id)
        .collect();
    targets.sort_unstable();
    assert_eq!(
        targets,
        vec![1, 2],
        "the encoder sees one capture target per selected monitor"
    );
    assert_eq!(
        host.capture_displays()?,
        vec![1, 2],
        "the host captures both selected monitors"
    );
    wait_for(
        "one owner to control and the other to wait",
        DEFAULT_WAIT,
        || {
            let states = [
                desktop_a.control_state().ok()?,
                desktop_b.control_state().ok()?,
            ];
            let controlling = states
                .iter()
                .filter(|state| **state == DesktopControlState::Controlling)
                .count();
            (controlling == 1 && states.contains(&DesktopControlState::Contended)).then_some(())
        },
    );

    let video_a = desktop_a.subscribe_video()?;
    let video_b = desktop_b.subscribe_video()?;
    for round in 0..3u64 {
        host.publish_desktop_video(
            remote_a.id,
            remote_a.generation,
            keyframe(round, vec![31; 1000]),
        )?;
        host.publish_desktop_video(
            remote_b.id,
            remote_b.generation,
            keyframe(round, vec![42; 1000]),
        )?;
        assert_eq!(
            next_picture(&video_a, "the first monitor's picture").payload,
            vec![31; 1000],
            "the first owner receives its own monitor's pictures"
        );
        assert_eq!(
            next_picture(&video_b, "the second monitor's picture").payload,
            vec![42; 1000],
            "the second owner receives its own monitor's pictures"
        );
    }

    let owner_c = controller.attach_controller_service(
        connection.id,
        connection.generation,
        ServiceKind::Desktop,
    )?;
    let owner_d = controller.attach_controller_service(
        connection.id,
        connection.generation,
        ServiceKind::Desktop,
    )?;
    assert!(
        matches!(
            controller.attach_controller_service(
                connection.id,
                connection.generation,
                ServiceKind::Desktop
            ),
            Err(Error::Busy)
        ),
        "owners beyond the bound are refused as busy"
    );
    controller.close_controller_service(owner_c.id, owner_c.generation)?;
    controller.close_controller_service(owner_d.id, owner_d.generation)?;

    host.set_topology(2, &[display(2, 0, 1280, 1)])?;
    assert_eq!(
        (
            host.hosted_display_selection(remote_a.id, remote_a.generation)?,
            host.hosted_display_fault(remote_a.id, remote_a.generation)?
        ),
        (Vec::new(), Some("the selected display is gone")),
        "the removed monitor's owner loses its selection and says why"
    );
    assert_eq!(
        (
            host.hosted_display_selection(remote_b.id, remote_b.generation)?,
            host.hosted_display_fault(remote_b.id, remote_b.generation)?
        ),
        (vec![2], None),
        "the other owner keeps its monitor"
    );
    host.publish_desktop_video(remote_b.id, remote_b.generation, keyframe(9, vec![43; 500]))?;
    assert_eq!(
        next_picture(&video_b, "the survivor's picture").payload,
        vec![43; 500],
        "removing one monitor spares the other owner's stream"
    );
    let events = host.subscribe()?;
    desktop_b.select_displays(&[1])?;
    assert_eq!(
        input_failed(&events).1,
        "the display is not in the current topology",
        "a selection of the removed monitor is refused"
    );
    assert_eq!(
        host.hosted_display_selection(remote_b.id, remote_b.generation)?,
        vec![2],
        "the refused selection leaves the survivor's own"
    );

    desktop_a.close();
    wait_for("the host to let the first owner go", DEFAULT_WAIT, || {
        (pair.host.hosted(Some(ServiceKind::Desktop)).len() == 1).then_some(())
    });
    host.publish_desktop_video(
        remote_b.id,
        remote_b.generation,
        keyframe(10, vec![44; 500]),
    )?;
    assert_eq!(
        next_picture(&video_b, "the survivor's picture after the close").payload,
        vec![44; 500],
        "closing one owner spares the other's stream"
    );
    assert_eq!(
        host.capture_displays()?,
        vec![2],
        "the host captures only the monitor still shown"
    );
    assert_eq!(
        controller.controller_session(connection.id)?.phase,
        SessionPhase::Active,
        "the connection outlives the closed owner"
    );
    expect![[r#"
        controller:
          service: service phase from=active generation=1 kind=Desktop service=3 to=closed: OwnerClose
          service: service phase from=active generation=1 kind=Desktop service=4 to=closed: OwnerClose
          service: service phase from=active generation=1 kind=Desktop service=1 to=closed: OwnerClose
        host:
          warn input: input failed reason=the display is not in the current topology session=1
          service: service phase from=active generation=1 kind=Desktop service=1 to=closed: PeerClose
    "#]].assert_eq(&trace.story(&["service", "input"]));
    Ok(())
}

/// Two viewers' sessions each show their own display. When the first
/// viewer's display goes, it alone loses its selection and says why, and
/// both streams are asked for a keyframe; a display that returns is not
/// selected again by itself, and the viewer moves to the display that
/// remains. The second viewer's display and its quality stay its own.
/// (Pointers refused for each reason are the Desktop service's, in
/// [`two_desktop_owners_fail_only_the_removed_display`]: a session carries
/// no input.)
#[test]
fn two_viewers_keep_their_own_displays_when_one_disappears() -> TestResult {
    let trace = traced();
    let pair = Pair::for_desktop();
    let host = &pair.host.core;
    InputLog::install(&pair.host, SCREEN)?;
    let (first, second) = (&pair.controller, &pair.add_viewer("second"));
    let left = placed(1, 0, (1920, 1080), 0, 100);
    let right = placed(2, -1920, (1280, 800), 90, 200);
    host.set_topology(1, &[left, right])?;

    let first_session = first.core.connect(&pair.host.endpoint())?;
    let second_session = second.core.connect(&pair.host.endpoint())?;
    wait_for(
        "the first viewer to learn the displays",
        DEFAULT_WAIT,
        || {
            first
                .core
                .session_topology(first_session.id, first_session.generation)
                .ok()
                .filter(|displays| displays.len() == 2 && displays[1].rotation_deg == 90)
        },
    );
    wait_for(
        "the second viewer to learn the displays",
        DEFAULT_WAIT,
        || {
            second
                .core
                .session_topology(second_session.id, second_session.generation)
                .ok()
                .filter(|displays| displays.iter().any(|display| display.origin_x == -1920))
        },
    );
    first
        .core
        .select_displays(first_session.id, first_session.generation, &[1])?;
    second
        .core
        .select_displays(second_session.id, second_session.generation, &[2])?;
    let first_host = pair.host.hosted_session(first).ok_or(Error::NoSession)?;
    let second_host = pair.host.hosted_session(second).ok_or(Error::NoSession)?;
    assert!(
        first_host.id < second_host.id,
        "the host numbers sessions in the order they arrived"
    );
    let selected = |session: &SessionInfo| {
        host.incoming_display_selection(session.id, session.generation)
            .ok()
    };
    wait_for("the viewers to select their displays", DEFAULT_WAIT, || {
        (selected(&first_host)? == [1] && selected(&second_host)? == [2]).then_some(())
    });
    assert_eq!(
        host.capture_displays()?,
        vec![1, 2],
        "the host captures both selected displays"
    );

    let events = host.subscribe()?;
    let resized = placed(2, -1920, (1000, 800), 90, 200);
    host.set_topology(2, &[resized])?;
    assert_eq!(
        (
            host.incoming_display_selection(first_host.id, first_host.generation)?,
            host.incoming_display_fault(first_host.id, first_host.generation)?
        ),
        (Vec::new(), Some("the selected display is gone")),
        "the first viewer loses its display and says why"
    );
    assert_eq!(
        (
            host.incoming_display_selection(second_host.id, second_host.generation)?,
            host.incoming_display_fault(second_host.id, second_host.generation)?
        ),
        (vec![2], None),
        "the second viewer keeps its display"
    );
    assert_eq!(
        host.capture_displays()?,
        vec![2],
        "the host captures only the display still shown"
    );
    let targets = host.video_targets()?;
    let shows = |id: u64| {
        targets
            .iter()
            .find(|target| target.id == id)
            .map(|target| target.display_id)
    };
    assert_eq!(
        (shows(first_host.id), shows(second_host.id)),
        (Some(0), Some(2)),
        "the first viewer's stream follows the capture; the second shows its display"
    );
    let mut keyframes = Vec::new();
    wait_for(
        "both streams to ask for a keyframe",
        Duration::from_secs(2),
        || {
            if let Some(CoreEvent::KeyframeRequested { id }) =
                events.recv_timeout(Duration::from_millis(20))
            {
                keyframes.push(id);
            }
            (keyframes.len() >= 2).then_some(())
        },
    );
    keyframes.sort_unstable();
    assert_eq!(
        keyframes,
        vec![first_host.id, second_host.id],
        "a changed topology asks each stream for a keyframe"
    );

    let restored = placed(1, 0, (1920, 1080), 0, 100);
    host.set_topology(3, &[restored, resized])?;
    assert_eq!(
        (
            host.incoming_display_selection(first_host.id, first_host.generation)?,
            host.incoming_display_selection(second_host.id, second_host.generation)?
        ),
        (Vec::new(), vec![2]),
        "a display that returns is not selected again by itself"
    );
    first
        .core
        .select_displays(first_session.id, first_session.generation, &[2])?;
    wait_for(
        "the first viewer to move to the remaining display",
        DEFAULT_WAIT,
        || {
            (selected(&first_host)? == [2]
                && host
                    .incoming_display_fault(first_host.id, first_host.generation)
                    .ok()?
                    .is_none())
            .then_some(())
        },
    );
    let marker = Quality::new(4_000, 24, 1600, 900)?;
    second
        .core
        .update_session_quality(second_session.id, SessionRole::Controller, marker)?;
    wait_for(
        "the second viewer's quality to arrive",
        DEFAULT_WAIT,
        || (pair.host.hosted_session(second)?.quality == Some(marker)).then_some(()),
    );
    assert_eq!(
        host.capture_displays()?,
        vec![2],
        "both viewers now show the remaining display"
    );
    expect![[r#""#]].assert_eq(&trace.story(&["topology", "input"]));
    Ok(())
}

/// Two Desktop owners on two computers' connections each show their own
/// display. When the first owner's display goes, it alone loses its
/// selection and says why, both are owed a keyframe, both viewers learn the
/// new displays, and its pointers are refused for the reason that fits
/// each until it selects the display that remains. The second owner's
/// selection survives a selection of a display that does not exist.
#[test]
fn two_desktop_owners_fail_only_the_removed_display() -> TestResult {
    let trace = traced();
    let pair = Pair::for_services(&[ServiceKind::Desktop]);
    let host = &pair.host.core;
    let applied_events = InputLog::install(&pair.host, SCREEN)?;
    let (first, second) = (&pair.controller, &pair.add_viewer("second"));
    let left = placed(1, 0, (1920, 1080), 0, 100);
    let right = placed(2, -1920, (1280, 800), 90, 200);
    host.set_topology(1, &[left, right])?;

    let first_connection = first.connect_services(&pair.host)?;
    let first_desktop = first.open_desktop(&first_connection)?;
    let second_connection = second.connect_services(&pair.host)?;
    let second_desktop = second.open_desktop(&second_connection)?;
    wait_for("the viewers to learn the displays", DEFAULT_WAIT, || {
        (first_desktop.topology().ok()?.len() == 2
            && second_desktop
                .topology()
                .ok()?
                .iter()
                .any(|display| display.scale_percent == 200)
            && first
                .core
                .session_topology(first_connection.id, first_connection.generation)
                .ok()?
                .len()
                == 2)
            .then_some(())
    });
    first_desktop.select_displays(&[1])?;
    second_desktop.select_displays(&[2])?;
    let first_remote = pair.host.serving(ServiceKind::Desktop, first);
    let second_remote = pair.host.serving(ServiceKind::Desktop, second);
    assert!(
        first_remote.connection_id < second_remote.connection_id,
        "the host numbers connections in the order they arrived"
    );
    let selected = |id: u64, generation: u64| host.hosted_display_selection(id, generation).ok();
    wait_for("the owners to select their displays", DEFAULT_WAIT, || {
        (selected(first_remote.id, first_remote.generation)? == [1]
            && selected(second_remote.id, second_remote.generation)? == [2])
        .then_some(())
    });
    assert_eq!(
        host.capture_displays()?,
        vec![1, 2],
        "the host captures both selected displays"
    );
    // Whatever keyframes opening owed, taken now, are not the topology's.
    host.take_desktop_keyframe_request(first_remote.id, first_remote.generation)?;
    host.take_desktop_keyframe_request(second_remote.id, second_remote.generation)?;

    let resized = placed(2, -1920, (1000, 800), 90, 200);
    host.set_topology(2, &[resized])?;
    assert_eq!(
        (
            host.hosted_display_selection(first_remote.id, first_remote.generation)?,
            host.hosted_display_fault(first_remote.id, first_remote.generation)?
        ),
        (Vec::new(), Some("the selected display is gone")),
        "the first owner loses its display and says why"
    );
    assert_eq!(
        (
            host.hosted_display_selection(second_remote.id, second_remote.generation)?,
            host.hosted_display_fault(second_remote.id, second_remote.generation)?
        ),
        (vec![2], None),
        "the second owner keeps its display"
    );
    assert_eq!(
        host.capture_displays()?,
        vec![2],
        "the host captures only the display still shown"
    );
    assert!(
        host.take_desktop_keyframe_request(first_remote.id, first_remote.generation)?,
        "the faulted owner is owed a keyframe"
    );
    assert!(
        host.take_desktop_keyframe_request(second_remote.id, second_remote.generation)?,
        "the surviving owner is owed a keyframe"
    );
    wait_for(
        "the first viewer to learn the new displays",
        DEFAULT_WAIT,
        || {
            let connection = first
                .core
                .session_topology(first_connection.id, first_connection.generation)
                .ok()?;
            let owner = first_desktop.topology().ok()?;
            (connection.len() == 1
                && connection[0].width == 1000
                && owner.len() == 1
                && owner[0].revision == 2)
                .then_some(())
        },
    );

    let events = host.subscribe()?;
    let mut send = |event| {
        first_desktop
            .submit_input(event)
            .expect("the first owner sends input");
    };
    for (pointer, key, why) in [
        (
            InputEvent::pointer(10, 10, 1, 1),
            InputEvent::key_down(usage::B),
            "the pointer uses an outdated display topology",
        ),
        (
            InputEvent::pointer(10, 10, 1, 2),
            InputEvent::key_up(usage::B),
            "the display is not in the current topology",
        ),
        (
            InputEvent::pointer(-1900, 10, 2, 2),
            InputEvent::key_down(usage::A),
            "no display is selected",
        ),
    ] {
        refused_before_key(
            &mut send,
            &applied_events,
            &events,
            first_remote.connection_id,
            (pointer, key),
            why,
        );
    }

    first_desktop.select_displays(&[2])?;
    wait_for(
        "the first owner to select the remaining display",
        DEFAULT_WAIT,
        || {
            (selected(first_remote.id, first_remote.generation)? == [2]
                && host
                    .hosted_display_fault(first_remote.id, first_remote.generation)
                    .ok()?
                    .is_none())
            .then_some(())
        },
    );
    let landed = InputEvent::pointer(-1900, 10, 2, 2);
    send(landed);
    wait_for(
        "the pointer to land on the remaining display",
        DEFAULT_WAIT,
        || has_applied(&applied_events, &landed).then_some(()),
    );

    // The second owner does not control: its pointer is discarded, and a
    // selection of no display, sent after it, fences that it was decided.
    let other = InputEvent::pointer(-1800, 20, 2, 2);
    second_desktop.submit_input(other)?;
    second_desktop.select_displays(&[9])?;
    next_event(
        &events,
        "the host to refuse the unknown display",
        DEFAULT_WAIT,
        |event| {
            matches!(event, CoreEvent::InputFailed { id, message }
                if *id == second_remote.connection_id
                    && *message == "the display is not in the current topology")
        },
    );
    assert!(
        !has_applied(&applied_events, &other),
        "the owner without control moves no pointer"
    );
    assert_eq!(
        host.hosted_display_selection(second_remote.id, second_remote.generation)?,
        vec![2],
        "a refused selection leaves the second owner's own"
    );
    expect![[r#"
        host:
          warn input: input refused kind=pointer reason=the pointer uses an outdated display topology service=1
          warn input: input refused kind=pointer reason=the display is not in the current topology service=1
          warn input: input refused kind=pointer reason=no display is selected service=1
          input: input refused kind=pointer reason=another viewer controls this computer service=2
          warn input: input failed reason=the display is not in the current topology session=2
    "#]]
    .assert_eq(&trace.story(&["topology", "input"]));
    Ok(())
}
