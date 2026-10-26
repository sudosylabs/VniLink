//! The desktop calls of one engine: the displays it publishes, the one a
//! stream shows, the pictures it publishes and receives, what its windows
//! decode, and a screenshot request already cancelled.

use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Barrier};

use vnilink::snapshot::ScreenSnapshot;
use vnilink::{CoreEvent, DisplayInfo, EncodedFrame, Error, VideoCodec, WindowDecoder};
use vnilink_testkit::{traced, Engine, TestResult};

const REVISION_MUST_INCREASE: &str = "the display revision must increase";

fn screen(
    id: u64,
    (x, y): (i32, i32),
    (width, height): (u16, u16),
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

/// The engine numbers its displays' revisions itself: a publication's
/// revision must increase, but one that changes no geometry keeps the
/// revision it has.
#[test]
fn topology_revision_must_increase_and_identical_geometry_keeps_it() -> TestResult {
    let _trace = traced();
    let engine = Engine::start("engine");
    let core = &engine.core;
    assert!(
        matches!(
            core.set_topology(0, &[]),
            Err(Error::InvalidSetting("a display revision is invalid"))
        ),
        "revision 0 is refused"
    );
    let mut left = screen(1, (-1920, 0), (1920, 1080), 90, 200);
    left.revision = 99;
    core.set_topology(1, &[left])?;
    let published = core.topology()?;
    assert_eq!(published.revision, 1, "the first publication is revision 1");
    assert_eq!(
        published.displays[0].revision, 1,
        "a display's revision is the engine's, not the caller's"
    );
    assert_eq!(
        published.displays[0].origin_x, -1920,
        "a negative origin is kept"
    );
    assert_eq!(
        published.displays[0].rotation_deg, 90,
        "the rotation is kept"
    );
    assert_eq!(
        published.displays[0].scale_percent, 200,
        "the scale is kept"
    );

    core.set_topology(9, &[left])?;
    assert_eq!(
        core.topology()?.revision,
        1,
        "the same geometry keeps its revision"
    );

    let wider = screen(1, (-1920, 0), (1921, 1080), 90, 200);
    assert!(
        matches!(
            core.set_topology(1, &[wider]),
            Err(Error::InvalidSetting(REVISION_MUST_INCREASE))
        ),
        "a change at the published revision is refused"
    );
    core.set_topology(2, &[wider])?;
    assert_eq!(core.topology()?.revision, 2, "a change takes its revision");
    assert_eq!(
        core.topology()?.displays[0].width,
        1921,
        "a change is published"
    );

    core.set_topology(3, &[])?;
    assert_eq!(core.topology()?.revision, 3, "no displays is a publication");
    assert!(
        core.topology()?.displays.is_empty(),
        "no displays are published"
    );
    assert!(
        matches!(
            core.set_topology(3, &[left]),
            Err(Error::InvalidSetting(REVISION_MUST_INCREASE))
        ),
        "a change at the published revision is refused"
    );
    assert!(
        matches!(
            core.set_topology(4, &[screen(1, (0, 0), (0, 10), 0, 100)]),
            Err(Error::InvalidSetting("a display size is out of range"))
        ),
        "a display without width is refused"
    );
    Ok(())
}

/// Sixteen threads publishing at once: the revision only rises, the last
/// publication wins, and the events say each revision in order.
#[test]
fn concurrent_topology_publications_never_regress_revision_or_event_order() -> TestResult {
    let _trace = traced();
    let engine = Engine::start("engine");
    let core = &engine.core;
    let events = core.subscribe()?;
    core.set_topology(1, &[screen(1, (0, 0), (101, 100), 0, 100)])?;
    let barrier = Barrier::new(16);
    std::thread::scope(|scope| {
        for worker in 0..16 {
            let barrier = &barrier;
            scope.spawn(move || {
                barrier.wait();
                for round in 0..3 {
                    let revision = 2 + round * 16 + worker;
                    let result = core.set_topology(
                        revision,
                        &[screen(1, (0, 0), (100 + revision as u16, 100), 0, 100)],
                    );
                    assert!(
                        result.is_ok()
                            || matches!(result, Err(Error::InvalidSetting(REVISION_MUST_INCREASE))),
                        "a publication is taken or refused as late: {result:?}"
                    );
                }
            });
        }
    });
    let snapshot = core.topology()?;
    assert_eq!(snapshot.revision, 49, "the highest revision wins");
    assert_eq!(
        snapshot.displays[0].width, 149,
        "the highest revision's displays are published"
    );
    let mut previous = 0;
    while let Some(event) = events.try_recv() {
        if let CoreEvent::TopologyChanged { revision } = event {
            assert!(revision > previous, "topology events regressed");
            previous = revision;
        }
    }
    assert_eq!(previous, 49, "the last event is the highest revision");
    Ok(())
}

#[test]
fn a_desktop_stream_selects_exactly_one_display() -> TestResult {
    let _trace = traced();
    let engine = Engine::start("engine");
    let one_display = "a desktop stream shows one display";
    assert!(
        matches!(
            engine.core.select_displays(1, 1, &[1, 2]),
            Err(Error::InvalidSetting(message)) if message == one_display
        ),
        "two displays are refused"
    );
    assert!(
        matches!(
            engine.core.select_displays(1, 1, &[]),
            Err(Error::InvalidSetting(message)) if message == one_display
        ),
        "no display is refused"
    );
    assert!(
        matches!(
            engine.core.select_displays(1, 1, &[0]),
            Err(Error::InvalidSetting("a display id is invalid"))
        ),
        "display 0 is refused"
    );
    Ok(())
}

/// Pictures are published to a session the engine has, and one reader at
/// a time receives them.
#[test]
fn video_api_rejects_a_second_subscriber_and_a_missing_session() -> TestResult {
    let _trace = traced();
    let engine = Engine::start("engine");
    assert!(
        !engine.core.capabilities().desktop_capture,
        "an engine without a capturer does not capture its desktop"
    );
    assert!(
        matches!(
            engine
                .core
                .publish_video(1, EncodedFrame::h264(true, 1, vec![1])?),
            Err(Error::NoSession)
        ),
        "a picture for a session the engine does not have is refused"
    );
    let first = engine.core.subscribe_video()?;
    assert!(
        matches!(engine.core.subscribe_video(), Err(Error::Busy)),
        "a second reader is refused"
    );
    drop(first);
    assert!(
        engine.core.subscribe_video().is_ok(),
        "a reader may subscribe once the first is gone"
    );
    Ok(())
}

#[test]
fn a_window_lists_each_codec_once() -> TestResult {
    let _trace = traced();
    let engine = Engine::start("engine");
    let decoder = WindowDecoder {
        codec: VideoCodec::H264,
        hardware: true,
        chroma444: false,
        bt601: false,
    };
    assert!(
        engine
            .core
            .set_window_decoders(vec![decoder, decoder])
            .is_err(),
        "a codec listed twice is refused"
    );
    engine.core.set_window_decoders(vec![decoder])?;
    Ok(())
}

/// A request cancelled before it starts says so, though it names no
/// connection the engine has.
#[test]
fn a_cancelled_request_is_not_a_missing_session() -> TestResult {
    let _trace = traced();
    let controller = Engine::start("controller");
    let cancel = Arc::new(AtomicBool::new(true));
    assert!(
        matches!(
            controller
                .core
                .request_screen_snapshot(1, 1, &cancel, false),
            Ok(ScreenSnapshot::Cancelled)
        ),
        "a cancelled request is cancelled, not a missing session"
    );
    Ok(())
}
