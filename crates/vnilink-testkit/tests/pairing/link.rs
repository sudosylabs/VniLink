// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! The link between two paired computers: the path it takes, and a
//! Desktop's cursor sent beside the picture when both sides can.

use std::sync::Arc;
use std::time::Duration;

use vnilink::{CursorImage, CursorState, HostCapture, ServiceKind};
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

/// A live link's two ends each report a selected direct path, over which
/// the handshake already moved bytes.
#[test]
fn a_live_session_reports_its_selected_direct_path() -> TestResult {
    let _trace = traced();
    let pair = Pair::for_desktop();
    let (host, controller) = (&pair.host.core, &pair.controller.core);
    let session = pair.connect_services();
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

/// When the host draws its cursor apart and the viewer can show it, each
/// cursor change reaches the viewer's Desktop with no picture sent; the
/// host refuses a cursor of another generation, and a closed Desktop gives
/// none.
#[test]
fn cursor_delivery_is_independent_of_video_and_fenced_by_generation() -> TestResult {
    let _trace = traced();
    let pair = Pair::for_desktop();
    let host = &pair.host.core;
    host.set_capture(Box::new(CursorCapture(true)))?;
    pair.controller.core.set_cursor_receiver(true)?;
    let connection = pair.connect_services();
    let desktop = pair.controller.open_desktop(&connection)?;
    let local = pair
        .host
        .hosted_for(ServiceKind::Desktop, &pair.controller)
        .ok_or("the host serves the Desktop")?;
    assert!(
        host.hosted_desktop_info(local.id, local.generation)?
            .separate_cursor,
        "the host sends its cursor apart"
    );
    assert!(
        desktop.desktop_info()?.separate_cursor,
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
        host.publish_desktop_cursor(local.id, local.generation, state.clone())?;
        wait_for(
            "the cursor to arrive without video",
            Duration::from_secs(3),
            || {
                (desktop
                    .cursor()
                    .expect("the live Desktop's cursor")
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
        host.publish_desktop_cursor(local.id, local.generation + 1, stale)
            .is_err(),
        "the host refuses a cursor of another generation"
    );
    desktop.close();
    assert!(
        desktop.cursor().is_err(),
        "a closed Desktop gives no cursor"
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
        let host = &pair.host.core;
        host.set_capture(Box::new(CursorCapture(capture)))?;
        pair.controller.core.set_cursor_receiver(viewer)?;
        let connection = pair.connect_services();
        let desktop = pair.controller.open_desktop(&connection)?;
        let local = pair
            .host
            .hosted_for(ServiceKind::Desktop, &pair.controller)
            .ok_or("the host serves the Desktop")?;
        assert!(
            !host
                .hosted_desktop_info(local.id, local.generation)?
                .separate_cursor,
            "capture {capture}, viewer {viewer}: the host keeps its cursor in the picture"
        );
        assert!(
            !desktop.desktop_info()?.separate_cursor,
            "capture {capture}, viewer {viewer}: the viewer receives no cursor apart"
        );
        assert!(
            host.publish_desktop_cursor(
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
