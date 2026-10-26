use crate::{CoreEvent, DisplayInfo, Error};

use super::support::start_direct;

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
