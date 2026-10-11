use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::{
    Core, CoreEvent, DisplayInfo, Error, EventSubscription, InputEvent, Quality, ServiceKind,
    SessionInfo, SessionPhase, SessionRole,
};

use super::support::{pair_for_control, start_direct, RecordingDesktop};

fn screen(
    id: u64,
    x: i32,
    y: i32,
    width: u16,
    height: u16,
    rotation: u16,
    scale: u16,
) -> DisplayInfo {
    DisplayInfo {
        id,
        origin_x: x,
        origin_y: y,
        width,
        height,
        rotation_deg: rotation,
        scale_percent: scale,
        revision: 1,
    }
}

fn wait_until(label: &str, mut ready: impl FnMut() -> bool) {
    let until = Instant::now() + Duration::from_secs(5);
    while !ready() {
        assert!(Instant::now() < until, "{label}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn host_session(host: &Core, peer: &str) -> SessionInfo {
    host.hosted_sessions()
        .unwrap()
        .into_iter()
        .find(|session| session.peer_id == peer)
        .expect("hosted session")
}

fn applied_has(events: &Mutex<Vec<InputEvent>>, event: &InputEvent) -> bool {
    events
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
        .contains(event)
}

/// The next applied key proves every earlier event on that stream was decided.
fn wait_key(events: &Mutex<Vec<InputEvent>>, key: InputEvent) {
    wait_until("the following key was not applied", || {
        applied_has(events, &key)
    });
}

/// A key between pointers keeps each pointer from collapsing into the next one.
fn reject_then_key(
    submit: &mut impl FnMut(InputEvent),
    applied: &Mutex<Vec<InputEvent>>,
    events: &EventSubscription,
    host_id: u64,
    pointer: InputEvent,
    key: InputEvent,
    message: &str,
) {
    submit(pointer);
    submit(key);
    wait_key(applied, key);
    assert!(!applied_has(applied, &pointer), "{message}");
    let until = Instant::now() + Duration::from_secs(2);
    loop {
        while let Some(event) = events.try_recv() {
            if let CoreEvent::InputFailed { id, message: got } = event {
                if id == host_id && got == message {
                    return;
                }
            }
        }
        assert!(Instant::now() < until, "missing input failure: {message}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn topology_revision_must_increase_and_identical_geometry_keeps_it() {
    let core = start_direct();
    assert!(matches!(
        core.set_topology(0, &[]),
        Err(Error::InvalidSetting("a display revision is invalid"))
    ));
    let mut left = screen(1, -1920, 0, 1920, 1080, 90, 200);
    left.revision = 99;
    core.set_topology(1, &[left]).unwrap();
    let published = core.topology().unwrap();
    assert_eq!(published.revision, 1);
    assert_eq!(published.displays[0].revision, 1);
    assert_eq!(published.displays[0].origin_x, -1920);
    assert_eq!(published.displays[0].rotation_deg, 90);
    assert_eq!(published.displays[0].scale_percent, 200);

    core.set_topology(9, &[left]).unwrap();
    assert_eq!(core.topology().unwrap().revision, 1);

    let wider = screen(1, -1920, 0, 1921, 1080, 90, 200);
    assert!(matches!(
        core.set_topology(1, &[wider]),
        Err(Error::InvalidSetting("the display revision must increase"))
    ));
    core.set_topology(2, &[wider]).unwrap();
    assert_eq!(core.topology().unwrap().revision, 2);
    assert_eq!(core.topology().unwrap().displays[0].width, 1921);

    core.set_topology(3, &[]).unwrap();
    assert_eq!(core.topology().unwrap().revision, 3);
    assert!(core.topology().unwrap().displays.is_empty());
    assert!(matches!(
        core.set_topology(3, &[left]),
        Err(Error::InvalidSetting("the display revision must increase"))
    ));
    assert!(matches!(
        core.set_topology(4, &[screen(1, 0, 0, 0, 10, 0, 100)]),
        Err(Error::InvalidSetting("a display size is out of range"))
    ));
}

#[test]
fn concurrent_topology_publications_never_regress_revision_or_event_order() {
    let core = start_direct();
    let events = core.subscribe().unwrap();
    core.set_topology(1, &[screen(1, 0, 0, 101, 100, 0, 100)])
        .unwrap();
    let barrier = std::sync::Barrier::new(16);
    std::thread::scope(|scope| {
        for worker in 0..16 {
            let core = &core;
            let barrier = &barrier;
            scope.spawn(move || {
                barrier.wait();
                for round in 0..3 {
                    let revision = 2 + round * 16 + worker;
                    let result = core.set_topology(
                        revision,
                        &[screen(1, 0, 0, 100 + revision as u16, 100, 0, 100)],
                    );
                    assert!(
                        result.is_ok()
                            || matches!(
                                result,
                                Err(Error::InvalidSetting("the display revision must increase"))
                            )
                    );
                }
            });
        }
    });
    let snapshot = core.topology().unwrap();
    assert_eq!(snapshot.revision, 49);
    assert_eq!(snapshot.displays[0].width, 149);
    let mut previous = 0;
    while let Some(event) = events.try_recv() {
        if let CoreEvent::TopologyChanged { revision } = event {
            assert!(revision > previous, "topology events regressed");
            previous = revision;
        }
    }
    assert_eq!(previous, 49);
}

#[test]
fn a_desktop_stream_selects_exactly_one_display() {
    let core = start_direct();
    assert!(matches!(
        core.select_displays(1, 1, &[1, 2]),
        Err(Error::InvalidSetting("a desktop stream shows one display"))
    ));
    assert!(matches!(
        core.select_displays(1, 1, &[]),
        Err(Error::InvalidSetting("a desktop stream shows one display"))
    ));
    assert!(matches!(
        core.select_displays(1, 1, &[0]),
        Err(Error::InvalidSetting("a display id is invalid"))
    ));
}

#[test]
fn two_viewers_keep_their_own_displays_when_one_disappears() {
    let host = start_direct();
    let first = start_direct();
    let second = start_direct();
    let applied = Arc::new(Mutex::new(Vec::new()));
    host.set_desktop(Box::new(RecordingDesktop {
        events: Arc::clone(&applied),
    }))
    .unwrap();
    pair_for_control(&host, &first);
    pair_for_control(&host, &second);
    let left = screen(1, 0, 0, 1920, 1080, 0, 100);
    let right = screen(2, -1920, 0, 1280, 800, 90, 200);
    host.set_topology(1, &[left, right]).unwrap();

    let first_session = first.connect(&host.endpoint_id()).unwrap();
    let second_session = second.connect(&host.endpoint_id()).unwrap();
    wait_until("the first viewer did not receive the topology", || {
        first
            .session_topology(first_session.id, first_session.generation)
            .ok()
            .is_some_and(|displays| displays.len() == 2 && displays[1].rotation_deg == 90)
    });
    wait_until("the second viewer did not receive the topology", || {
        second
            .session_topology(second_session.id, second_session.generation)
            .ok()
            .is_some_and(|displays| displays.iter().any(|display| display.origin_x == -1920))
    });
    first
        .select_displays(first_session.id, first_session.generation, &[1])
        .unwrap();
    second
        .select_displays(second_session.id, second_session.generation, &[2])
        .unwrap();
    let first_host = host_session(&host, &first.endpoint_id());
    let second_host = host_session(&host, &second.endpoint_id());
    assert!(first_host.id < second_host.id);
    wait_until("the viewers did not select their displays", || {
        host.incoming_display_selection(first_host.id, first_host.generation)
            .ok()
            .as_deref()
            == Some(&[1][..])
            && host
                .incoming_display_selection(second_host.id, second_host.generation)
                .ok()
                .as_deref()
                == Some(&[2][..])
    });
    assert_eq!(host.capture_displays().unwrap(), vec![1, 2]);

    let events = host.subscribe().unwrap();
    let resized = screen(2, -1920, 0, 1000, 800, 90, 200);
    host.set_topology(2, &[resized]).unwrap();
    assert_eq!(
        host.incoming_display_selection(first_host.id, first_host.generation)
            .unwrap(),
        Vec::<u64>::new()
    );
    assert_eq!(
        host.incoming_display_fault(first_host.id, first_host.generation)
            .unwrap(),
        Some("the selected display is gone")
    );
    assert_eq!(
        host.incoming_display_selection(second_host.id, second_host.generation)
            .unwrap(),
        vec![2]
    );
    assert_eq!(
        host.incoming_display_fault(second_host.id, second_host.generation)
            .unwrap(),
        None
    );
    assert_eq!(host.capture_displays().unwrap(), vec![2]);
    let targets = host.video_targets().unwrap();
    assert_eq!(
        targets
            .iter()
            .find(|target| target.id == first_host.id)
            .unwrap()
            .display_id,
        0
    );
    assert_eq!(
        targets
            .iter()
            .find(|target| target.id == second_host.id)
            .unwrap()
            .display_id,
        2
    );
    let mut keyframes = Vec::new();
    let until = Instant::now() + Duration::from_secs(2);
    while keyframes.len() < 2 && Instant::now() < until {
        if let Some(CoreEvent::KeyframeRequested { id }) =
            events.recv_timeout(Duration::from_millis(20))
        {
            keyframes.push(id);
        }
    }
    keyframes.sort_unstable();
    assert_eq!(keyframes, vec![first_host.id, second_host.id]);

    let mut send = |event| {
        first
            .submit_input(first_session.id, first_session.generation, event)
            .unwrap();
    };
    reject_then_key(
        &mut send,
        &applied,
        &events,
        first_host.id,
        InputEvent::pointer(10, 10, 1, 1),
        InputEvent::key_down(crate::input::usage::B),
        "the pointer uses an outdated display topology",
    );
    reject_then_key(
        &mut send,
        &applied,
        &events,
        first_host.id,
        InputEvent::pointer(10, 10, 1, 2),
        InputEvent::key_up(crate::input::usage::B),
        "the display is not in the current topology",
    );
    reject_then_key(
        &mut send,
        &applied,
        &events,
        first_host.id,
        InputEvent::pointer(-1900, 10, 2, 2),
        InputEvent::key_down(crate::input::usage::A),
        "no display is selected",
    );

    let restored = screen(1, 0, 0, 1920, 1080, 0, 100);
    host.set_topology(3, &[restored, resized]).unwrap();
    assert!(host
        .incoming_display_selection(first_host.id, first_host.generation)
        .unwrap()
        .is_empty());
    assert_eq!(
        host.incoming_display_selection(second_host.id, second_host.generation)
            .unwrap(),
        vec![2]
    );
    let replugged = InputEvent::pointer(10, 10, 1, 3);
    let release = InputEvent::key_up(crate::input::usage::A);
    first
        .submit_input(first_session.id, first_session.generation, replugged)
        .unwrap();
    first
        .submit_input(first_session.id, first_session.generation, release)
        .unwrap();
    wait_key(&applied, release);
    assert!(!applied_has(&applied, &replugged));

    first
        .select_displays(first_session.id, first_session.generation, &[2])
        .unwrap();
    wait_until(
        "the first viewer did not move to the remaining display",
        || {
            host.incoming_display_selection(first_host.id, first_host.generation)
                .ok()
                .as_deref()
                == Some(&[2][..])
                && host
                    .incoming_display_fault(first_host.id, first_host.generation)
                    .ok()
                    == Some(None)
        },
    );
    let landed = InputEvent::pointer(-1900, 10, 2, 3);
    first
        .submit_input(first_session.id, first_session.generation, landed)
        .unwrap();
    wait_until("the pointer did not land on the remaining display", || {
        applied_has(&applied, &landed)
    });

    let other = InputEvent::pointer(-1800, 20, 2, 3);
    second
        .submit_input(second_session.id, second_session.generation, other)
        .unwrap();
    let marker = Quality::new(4_000, 24, 1600, 900).unwrap();
    second
        .update_session_quality(second_session.id, SessionRole::Controller, marker)
        .unwrap();
    wait_until("the second viewer's quality fence did not arrive", || {
        host_session(&host, &second.endpoint_id()).quality == Some(marker)
    });
    assert!(!applied_has(&applied, &other));
    assert_eq!(host.capture_displays().unwrap(), vec![2]);
}

#[test]
fn two_desktop_owners_fail_only_the_removed_display() {
    let host = start_direct();
    let first = start_direct();
    let second = start_direct();
    let applied = Arc::new(Mutex::new(Vec::new()));
    host.set_desktop(Box::new(RecordingDesktop {
        events: Arc::clone(&applied),
    }))
    .unwrap();
    pair_for_control(&host, &first);
    pair_for_control(&host, &second);
    host.authorize_service(&first.endpoint_id(), ServiceKind::Desktop, true)
        .unwrap();
    host.authorize_service(&second.endpoint_id(), ServiceKind::Desktop, true)
        .unwrap();
    let left = screen(1, 0, 0, 1920, 1080, 0, 100);
    let right = screen(2, -1920, 0, 1280, 800, 90, 200);
    host.set_topology(1, &[left, right]).unwrap();

    let first_pending = first.begin_connect_services(&host.endpoint_id()).unwrap();
    wait_until("the first connection did not become active", || {
        first.controller_session(first_pending.id).unwrap().phase == SessionPhase::Active
    });
    let first_parent = first.controller_session(first_pending.id).unwrap();
    let first_owner = first
        .attach_controller_service(
            first_parent.id,
            first_parent.generation,
            ServiceKind::Desktop,
        )
        .unwrap();
    let first_desktop = first
        .open_desktop_service(first_owner.id, first_owner.generation)
        .unwrap();
    let second_pending = second.begin_connect_services(&host.endpoint_id()).unwrap();
    wait_until("the second connection did not become active", || {
        second.controller_session(second_pending.id).unwrap().phase == SessionPhase::Active
    });
    let second_parent = second.controller_session(second_pending.id).unwrap();
    let second_owner = second
        .attach_controller_service(
            second_parent.id,
            second_parent.generation,
            ServiceKind::Desktop,
        )
        .unwrap();
    let second_desktop = second
        .open_desktop_service(second_owner.id, second_owner.generation)
        .unwrap();

    wait_until("the desktop topology did not arrive", || {
        first_desktop
            .topology()
            .ok()
            .is_some_and(|displays| displays.len() == 2)
            && second_desktop
                .topology()
                .ok()
                .is_some_and(|displays| displays.iter().any(|display| display.scale_percent == 200))
            && first
                .session_topology(first_parent.id, first_parent.generation)
                .ok()
                .is_some_and(|displays| displays.len() == 2)
    });
    first_desktop.select_displays(&[1]).unwrap();
    second_desktop.select_displays(&[2]).unwrap();
    let first_remote = host
        .hosted_services()
        .unwrap()
        .into_iter()
        .find(|service| {
            service.kind == ServiceKind::Desktop && service.peer_id == first.endpoint_id()
        })
        .unwrap();
    let second_remote = host
        .hosted_services()
        .unwrap()
        .into_iter()
        .find(|service| {
            service.kind == ServiceKind::Desktop && service.peer_id == second.endpoint_id()
        })
        .unwrap();
    assert!(first_remote.connection_id < second_remote.connection_id);
    wait_until("the desktop owners did not select their displays", || {
        host.hosted_display_selection(first_remote.id, first_remote.generation)
            .ok()
            .as_deref()
            == Some(&[1][..])
            && host
                .hosted_display_selection(second_remote.id, second_remote.generation)
                .ok()
                .as_deref()
                == Some(&[2][..])
    });
    assert_eq!(host.capture_displays().unwrap(), vec![1, 2]);
    let _ = host
        .take_desktop_keyframe_request(first_remote.id, first_remote.generation)
        .unwrap();
    let _ = host
        .take_desktop_keyframe_request(second_remote.id, second_remote.generation)
        .unwrap();

    let resized = screen(2, -1920, 0, 1000, 800, 90, 200);
    host.set_topology(2, &[resized]).unwrap();
    assert!(host
        .hosted_display_selection(first_remote.id, first_remote.generation)
        .unwrap()
        .is_empty());
    assert_eq!(
        host.hosted_display_fault(first_remote.id, first_remote.generation)
            .unwrap(),
        Some("the selected display is gone")
    );
    assert_eq!(
        host.hosted_display_selection(second_remote.id, second_remote.generation)
            .unwrap(),
        vec![2]
    );
    assert_eq!(
        host.hosted_display_fault(second_remote.id, second_remote.generation)
            .unwrap(),
        None
    );
    assert_eq!(host.capture_displays().unwrap(), vec![2]);
    assert!(host
        .take_desktop_keyframe_request(first_remote.id, first_remote.generation)
        .unwrap());
    assert!(host
        .take_desktop_keyframe_request(second_remote.id, second_remote.generation)
        .unwrap());
    wait_until("the controllers did not store the new topology", || {
        first
            .session_topology(first_parent.id, first_parent.generation)
            .ok()
            .is_some_and(|displays| displays.len() == 1 && displays[0].width == 1000)
            && first_desktop
                .topology()
                .ok()
                .is_some_and(|displays| displays.len() == 1 && displays[0].revision == 2)
    });

    let events = host.subscribe().unwrap();
    let mut send = |event| first_desktop.submit_input(event).unwrap();
    reject_then_key(
        &mut send,
        &applied,
        &events,
        first_remote.connection_id,
        InputEvent::pointer(10, 10, 1, 1),
        InputEvent::key_down(crate::input::usage::B),
        "the pointer uses an outdated display topology",
    );
    reject_then_key(
        &mut send,
        &applied,
        &events,
        first_remote.connection_id,
        InputEvent::pointer(10, 10, 1, 2),
        InputEvent::key_up(crate::input::usage::B),
        "the display is not in the current topology",
    );
    reject_then_key(
        &mut send,
        &applied,
        &events,
        first_remote.connection_id,
        InputEvent::pointer(-1900, 10, 2, 2),
        InputEvent::key_down(crate::input::usage::A),
        "no display is selected",
    );

    first_desktop.select_displays(&[2]).unwrap();
    wait_until(
        "the first desktop did not select the remaining display",
        || {
            host.hosted_display_selection(first_remote.id, first_remote.generation)
                .ok()
                .as_deref()
                == Some(&[2][..])
                && host
                    .hosted_display_fault(first_remote.id, first_remote.generation)
                    .ok()
                    == Some(None)
        },
    );
    let landed = InputEvent::pointer(-1900, 10, 2, 2);
    first_desktop.submit_input(landed).unwrap();
    wait_until("the desktop pointer did not land", || {
        applied_has(&applied, &landed)
    });

    let other = InputEvent::pointer(-1800, 20, 2, 2);
    second_desktop.submit_input(other).unwrap();
    second_desktop.select_displays(&[9]).unwrap();
    wait_until(
        "the discarded viewer's selection fault was not noted",
        || {
            let mut saw = false;
            while let Some(event) = events.try_recv() {
                if let CoreEvent::InputFailed { id, message } = event {
                    if id == second_remote.connection_id
                        && message == "the display is not in the current topology"
                    {
                        saw = true;
                    }
                }
            }
            saw
        },
    );
    assert!(!applied_has(&applied, &other));
    assert_eq!(
        host.hosted_display_selection(second_remote.id, second_remote.generation)
            .unwrap(),
        vec![2]
    );
}

#[test]
fn signed_origin_extremes_are_refused_on_both_axes() {
    for origin in [i32::MIN, i32::MAX, -32769, 32769] {
        assert!(
            crate::input::validate_topology(&[screen(1, origin, 0, 1920, 1080, 0, 100)]).is_err(),
            "origin_x {origin} accepted"
        );
        assert!(
            crate::input::validate_topology(&[screen(1, 0, origin, 1920, 1080, 0, 100)]).is_err(),
            "origin_y {origin} accepted"
        );
    }
    for origin in [-32768, 32768, 0] {
        assert!(
            crate::input::validate_topology(&[screen(1, origin, 0, 1920, 1080, 0, 100)]).is_ok(),
            "origin_x {origin} refused"
        );
        assert!(
            crate::input::validate_topology(&[screen(1, 0, origin, 1920, 1080, 0, 100)]).is_ok(),
            "origin_y {origin} refused"
        );
    }
}

#[test]
fn hostile_wire_topology_with_extreme_origin_is_malformed() {
    use crate::protocol::{ControlMessage, SessionStamp};
    for (x, y) in [(i32::MIN, 0), (0, i32::MIN), (i32::MAX, 0), (0, i32::MAX)] {
        let bytes = ControlMessage::Topology {
            stamp: SessionStamp::new(9, 1),
            revision: 1,
            displays: vec![screen(1, x, y, 1920, 1080, 0, 100)],
        }
        .encode();
        assert!(
            matches!(ControlMessage::decode(&bytes), Err(crate::Error::Malformed)),
            "hostile origin ({x}, {y}) decoded"
        );
    }
    let bytes = ControlMessage::Topology {
        stamp: SessionStamp::new(9, 1),
        revision: 1,
        displays: vec![screen(1, 0, 0, 1920, 1080, 0, 100)],
    }
    .encode();
    assert!(matches!(
        ControlMessage::decode(&bytes),
        Ok(ControlMessage::Topology { .. })
    ));
}
