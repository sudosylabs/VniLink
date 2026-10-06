use super::support::*;

#[test]
fn a_live_session_reports_its_selected_direct_path() {
    let host = start_direct();
    let controller = start_direct();
    pair_for_control(&host, &controller);
    let session = controller.connect(&host.endpoint_id()).unwrap();
    let paths = controller.session_paths(session.id).unwrap();
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
    let host_session = host.snapshot().unwrap().incoming.unwrap();
    let host_paths = host.session_paths(host_session.id).unwrap();
    assert!(
        host_paths.iter().any(|path| path.selected && !path.relay),
        "expected a selected direct path, got {host_paths:?}"
    );
}

#[test]
fn an_unknown_session_has_no_paths() {
    let core = start_direct();
    assert!(matches!(
        core.session_paths(999),
        Err(crate::error::Error::NoSession)
    ));
}

#[test]
fn automatic_ticks_preserve_an_explicit_picture_preset() {
    let host = start_direct();
    let controller = start_direct();
    pair_for_control(&host, &controller);
    let session = controller.connect(&host.endpoint_id()).unwrap();
    let chosen = crate::Quality::new(1500, 15, 1280, 720).unwrap();
    controller
        .update_session_quality(session.id, crate::SessionRole::Controller, chosen)
        .unwrap();
    assert_eq!(controller.auto_quality_tick(session.id).unwrap(), None);
    assert_eq!(
        controller.snapshot().unwrap().outgoing.unwrap().quality,
        Some(chosen)
    );
}

struct CursorCapture(bool);
impl crate::video::HostCapture for CursorCapture {
    fn monitor(&self) -> (u16, u16) {
        (1920, 1080)
    }
    fn separate_cursor(&self) -> bool {
        self.0
    }
}

#[test]
fn cursor_delivery_is_independent_of_video_and_fenced_by_generation() {
    use crate::{CursorImage, CursorState, SessionRole};
    use std::sync::Arc;
    let host = start_direct();
    let controller = start_direct();
    host.set_capture(Box::new(CursorCapture(true))).unwrap();
    controller.set_cursor_receiver(true).unwrap();
    pair_for_control(&host, &controller);
    let remote = controller.connect(&host.endpoint_id()).unwrap();
    let local = host.snapshot().unwrap().incoming.unwrap();
    assert!(host.separate_cursor(local.id, SessionRole::Host).unwrap());
    assert!(controller
        .separate_cursor(remote.id, SessionRole::Controller)
        .unwrap());
    let image = Arc::new(CursorImage::new(1, 1, (0, 0), vec![20, 30, 40, 255]).unwrap());
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
        host.publish_cursor(local.id, local.generation, state.clone())
            .unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        loop {
            if controller
                .cursor(remote.id, remote.generation)
                .unwrap()
                .as_deref()
                == Some(&state)
            {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "cursor did not arrive without video"
            );
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }
    let stale = CursorState {
        position: (0, 0),
        visible: true,
        image: Some(image),
    };
    assert!(host
        .publish_cursor(local.id, local.generation + 1, stale)
        .is_err());
    assert!(controller.cursor(remote.id, remote.generation + 1).is_err());
    controller.disconnect().unwrap();
    wait_closed(&controller, SessionRole::Controller);
    assert!(controller.cursor(remote.id, remote.generation).is_err());
}

#[test]
fn independent_cursor_requires_both_capture_and_viewer_support() {
    for (capture, viewer) in [(true, false), (false, true), (false, false)] {
        let host = start_direct();
        let controller = start_direct();
        host.set_capture(Box::new(CursorCapture(capture))).unwrap();
        controller.set_cursor_receiver(viewer).unwrap();
        pair_for_control(&host, &controller);
        let remote = controller.connect(&host.endpoint_id()).unwrap();
        let local = host.snapshot().unwrap().incoming.unwrap();
        assert!(!host
            .separate_cursor(local.id, crate::SessionRole::Host)
            .unwrap());
        assert!(!controller
            .separate_cursor(remote.id, crate::SessionRole::Controller)
            .unwrap());
        assert!(host
            .publish_cursor(
                local.id,
                local.generation,
                crate::CursorState {
                    position: (1, 2),
                    visible: true,
                    image: None
                }
            )
            .is_err());
    }
}
