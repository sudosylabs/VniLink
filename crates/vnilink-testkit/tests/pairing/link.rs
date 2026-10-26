//! The link between two paired computers: the path a session takes, the
//! picture preset it keeps, and a cursor sent beside the picture when both
//! sides can.

use std::sync::Arc;
use std::time::Duration;

use vnilink::{CursorImage, CursorState, HostCapture, Quality, SessionRole};
use vnilink_testkit::session::wait_closed;
use vnilink_testkit::{traced, wait_for, Pair, TestResult};

/// A host's capture of a 1920×1080 display that draws its cursor apart
/// from the picture, or does not.
struct CursorCapture(bool);

impl HostCapture for CursorCapture {
    fn monitor(&self) -> (u16, u16) {
        (1920, 1080)
    }

    fn separate_cursor(&self) -> bool {
        self.0
    }
}

/// A live session's two ends each report a selected direct path, over
/// which the handshake already moved bytes.
#[test]
fn a_live_session_reports_its_selected_direct_path() -> TestResult {
    let _trace = traced();
    let pair = Pair::for_desktop();
    let (host, controller) = (&pair.host.core, &pair.controller.core);
    let session = controller.connect(&pair.host.endpoint())?;
    let paths = controller.session_paths(session.id)?;
    assert!(
        paths.iter().any(|path| path.selected && !path.relay),
        "expected a selected direct path, got {paths:?}"
    );
    assert!(
        paths
            .iter()
            .map(|path| path.tx_bytes + path.rx_bytes)
            .sum::<u64>()
            > 0,
        "expected handshake bytes on the paths, got {paths:?}"
    );
    let host_session = host.snapshot()?.incoming.ok_or("the host's session")?;
    let host_paths = host.session_paths(host_session.id)?;
    assert!(
        host_paths.iter().any(|path| path.selected && !path.relay),
        "expected a selected direct path, got {host_paths:?}"
    );
    Ok(())
}

/// A picture preset the viewer chose is the session's.
#[test]
fn manual_picture_preset_persists_on_the_session() -> TestResult {
    let _trace = traced();
    let pair = Pair::for_desktop();
    let controller = &pair.controller.core;
    let session = controller.connect(&pair.host.endpoint())?;
    let chosen = Quality::new(1500, 15, 1280, 720)?;
    controller.update_session_quality(session.id, SessionRole::Controller, chosen)?;
    assert_eq!(
        controller
            .snapshot()?
            .outgoing
            .ok_or("the session")?
            .quality,
        Some(chosen),
        "the session keeps the chosen preset"
    );
    Ok(())
}

/// When the host draws its cursor apart and the viewer can show it, each
/// cursor change reaches the viewer with no picture sent, and a cursor of
/// another generation is refused on either side, and after the session.
#[test]
fn cursor_delivery_is_independent_of_video_and_fenced_by_generation() -> TestResult {
    let _trace = traced();
    let pair = Pair::for_desktop();
    let (host, controller) = (&pair.host.core, &pair.controller.core);
    host.set_capture(Box::new(CursorCapture(true)))?;
    controller.set_cursor_receiver(true)?;
    let remote = controller.connect(&pair.host.endpoint())?;
    let local = host.snapshot()?.incoming.ok_or("the host's session")?;
    assert!(
        host.separate_cursor(local.id, SessionRole::Host)?,
        "the host sends its cursor apart"
    );
    assert!(
        controller.separate_cursor(remote.id, SessionRole::Controller)?,
        "the viewer receives the cursor apart"
    );
    let image = Arc::new(CursorImage::new(1, 1, (0, 0), vec![20, 30, 40, 255])?);
    for (position, visible) in [
        ((10, 20), true),
        ((55, 66), true),
        ((55, 66), false),
        ((88, 99), true),
    ] {
        let state = CursorState {
            position,
            visible,
            image: Some(image.clone()),
        };
        host.publish_cursor(local.id, local.generation, state.clone())?;
        wait_for(
            "the cursor to arrive without video",
            Duration::from_secs(3),
            || {
                (controller
                    .cursor(remote.id, remote.generation)
                    .expect("the live session's cursor")
                    .as_deref()
                    == Some(&state))
                .then_some(())
            },
        );
    }
    let stale = CursorState {
        position: (0, 0),
        visible: true,
        image: Some(image),
    };
    assert!(
        host.publish_cursor(local.id, local.generation + 1, stale)
            .is_err(),
        "the host refuses a cursor of another generation"
    );
    assert!(
        controller.cursor(remote.id, remote.generation + 1).is_err(),
        "the viewer gives no cursor of another generation"
    );
    controller.disconnect()?;
    wait_closed(controller, SessionRole::Controller, Duration::from_secs(5));
    assert!(
        controller.cursor(remote.id, remote.generation).is_err(),
        "a closed session gives no cursor"
    );
    Ok(())
}

/// Unless the host draws its cursor apart and the viewer can show it, the
/// cursor stays in the picture, and the host refuses to send it apart.
#[test]
fn independent_cursor_requires_both_capture_and_viewer_support() -> TestResult {
    let _trace = traced();
    for (capture, viewer) in [(true, false), (false, true), (false, false)] {
        let pair = Pair::for_desktop();
        let (host, controller) = (&pair.host.core, &pair.controller.core);
        host.set_capture(Box::new(CursorCapture(capture)))?;
        controller.set_cursor_receiver(viewer)?;
        let remote = controller.connect(&pair.host.endpoint())?;
        let local = host.snapshot()?.incoming.ok_or("the host's session")?;
        assert!(
            !host.separate_cursor(local.id, SessionRole::Host)?,
            "capture {capture}, viewer {viewer}: the host keeps its cursor in the picture"
        );
        assert!(
            !controller.separate_cursor(remote.id, SessionRole::Controller)?,
            "capture {capture}, viewer {viewer}: the viewer receives no cursor apart"
        );
        assert!(
            host.publish_cursor(
                local.id,
                local.generation,
                CursorState {
                    position: (1, 2),
                    visible: true,
                    image: None
                }
            )
            .is_err(),
            "capture {capture}, viewer {viewer}: the host sends no cursor apart"
        );
    }
    Ok(())
}
