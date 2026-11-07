// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! What the services scenarios share: a service's phase on the controller,
//! and a Desktop beside Files ended one way or another ([`check`]).
//! Waits on sessions, the closes an engine published, paths inside a
//! host's folder and a host offering its shell are the kit's
//! ([`vnilink_testkit::session`], [`vnilink_testkit::events`],
//! [`vnilink_testkit::files::Location`], [`vnilink_testkit::terminal`]).

use expect_test::Expect;
use std::time::Duration;

use vnilink::{
    usage, CloseCause, Error, InputEvent, PointerButton, ServiceInfo, ServiceKind, ServicePhase,
    SessionPhase,
};
use vnilink_testkit::desktop::{keyframe, InputLog};
use vnilink_testkit::{assert_story, wait_for, Engine, Pair, TestResult, Traced, DEFAULT_WAIT};

/// The phase of `service` on the controller now.
pub(super) fn phase(controller: &Engine, service: &ServiceInfo) -> Result<ServicePhase, Error> {
    Ok(controller.core.controller_service(service.id)?.phase)
}

/// How a test ends a Desktop.
#[derive(Clone, Copy, Debug)]
pub(super) enum DesktopClose {
    /// The controller closes the Desktop's handle.
    HandleClose,
    /// The controller closes the Desktop's owner.
    OwnerClose,
    /// The host closes the Desktop it serves.
    HostClose,
    /// The host stops sharing Desktop.
    HostStopsSharing,
}

/// Opens a Desktop and Files on one services connection to a host sharing
/// both, holds a key and a button on the Desktop and sees one of the host's
/// pictures, then ends the Desktop by `close`. Asserts that the close
/// releases what the Desktop held, latest first and once; that the Desktop
/// closes naming who closed it and then takes no picture and no input;
/// that the connection on both sides and Files carry on; that the engines
/// told the story `expect` of their services up to then; that a redial
/// keeps the Desktop's cause while Files opens again; and that no record
/// holds a folder Files made.
#[track_caller]
pub(super) fn check(trace: &Traced, close: DesktopClose, expect: Expect) -> TestResult {
    let pair = Pair::for_desktop();
    let (host, controller) = (&pair.host.core, &pair.controller.core);
    let here = pair.host.location();
    let (root, folder) = (here.root, here.path(""));
    let applied = InputLog::install(&pair.host, (1920, 1080))?;
    host.set_service_sharing(ServiceKind::Files, true)?;
    let session = pair.connect_services();
    let incoming = host.snapshot()?.incoming.ok_or("the host's session")?;
    let desktop = pair.attach(&session, ServiceKind::Desktop);
    let handle = controller.open_desktop_service(desktop.id, desktop.generation)?;
    assert!(
        matches!(
            controller.open_desktop_service(desktop.id, desktop.generation),
            Err(Error::Busy)
        ),
        "{close:?}: a Desktop owner opens once"
    );
    let (files, pipe) = pair.controller.open_files(&session)?;
    let key = InputEvent::key_down(usage::A);
    let button = InputEvent::button_down(PointerButton::Left);
    handle.submit_input(key)?;
    handle.submit_input(button)?;
    applied.exactly(2);
    let subscription = handle.subscribe_video()?;
    let remote = pair.host.serving(ServiceKind::Desktop, &pair.controller);
    host.publish_desktop_video(remote.id, remote.generation, keyframe(7, vec![7]))?;
    assert_eq!(
        subscription
            .recv_timeout(Duration::from_secs(5))
            .ok_or("a picture")?
            .payload,
        vec![7],
        "{close:?}: the Desktop carries the host's pictures"
    );

    match close {
        DesktopClose::HandleClose => handle.close(),
        DesktopClose::OwnerClose => {
            controller.close_controller_service(desktop.id, desktop.generation)?
        }
        DesktopClose::HostClose => host.close_hosted_service(remote.id, remote.generation)?,
        DesktopClose::HostStopsSharing => host.set_service_sharing(ServiceKind::Desktop, false)?,
    }
    assert_eq!(
        applied.wait_for(4),
        vec![
            key,
            button,
            InputEvent::button_up(PointerButton::Left),
            InputEvent::key_up(usage::A)
        ],
        "{close:?}: the close releases what the Desktop held, latest first"
    );
    let expected = match close {
        DesktopClose::HostClose | DesktopClose::HostStopsSharing => {
            ServicePhase::Closed(CloseCause::PeerClose)
        }
        _ => ServicePhase::Closed(CloseCause::OwnerClose),
    };
    wait_for(
        &format!("{close:?}: the Desktop to close as {expected:?}"),
        DEFAULT_WAIT,
        || (phase(&pair.controller, &desktop).ok()? == expected).then_some(()),
    );
    wait_for(
        &format!("{close:?}: the Desktop's handle to end"),
        DEFAULT_WAIT,
        || handle.is_cancelled().then_some(()),
    );
    assert!(
        handle.submit_input(key).is_err(),
        "{close:?}: a closed Desktop takes no input"
    );
    // The host ends its side of the desktop just after the controller's.
    pair.host.wait_released(Some(ServiceKind::Desktop));
    assert!(
        matches!(
            host.publish_desktop_video(remote.id, remote.generation, keyframe(8, vec![8])),
            Err(Error::NoSession | Error::StaleSession)
        ),
        "{close:?}: a closed Desktop takes no pictures"
    );
    assert!(
        subscription.try_recv().is_none(),
        "{close:?}: no picture follows the close"
    );
    assert_eq!(
        applied.events().len(),
        4,
        "{close:?}: the close releases each held key and button once"
    );

    let incoming_now = host.snapshot()?.incoming.ok_or("the host's session")?;
    assert_eq!(
        controller.controller_session(session.id)?.phase,
        SessionPhase::Active,
        "{close:?}: the controller's connection stays"
    );
    assert_eq!(
        (incoming_now.id, incoming_now.phase),
        (incoming.id, SessionPhase::Active),
        "{close:?}: the host's session stays"
    );
    pipe.mkdir(root, &folder, "after-desktop-close")?;
    assert_eq!(
        pipe.list(root, &folder, 0, 20)?.entries[0].name,
        "after-desktop-close",
        "{close:?}: Files works after the Desktop closed"
    );
    assert!(!pipe.is_cancelled(), "{close:?}: Files is not cancelled");
    assert_eq!(
        phase(&pair.controller, &files)?,
        ServicePhase::Active,
        "{close:?}: Files stays active"
    );
    // Told before the redial: the host names the old Files' lost parent on
    // either of two paths, at a moment of its own.
    assert_story(expect, || trace.story(&["service", "hosting"]));
    controller.drop_controller_link(session.id)?;
    pair.controller.redialed(&session);
    assert_eq!(
        phase(&pair.controller, &desktop)?,
        expected,
        "{close:?}: the redial keeps the first close's cause"
    );
    let files = controller.controller_service(files.id)?;
    let fresh_pipe = controller.open_files_service(files.id, files.generation)?;
    pipe.close();
    assert!(
        !fresh_pipe.is_cancelled(),
        "{close:?}: closing the old pipe leaves the new one"
    );
    fresh_pipe.mkdir(root, &folder, "after-reconnect")?;
    trace.assert_unrecorded(
        "a folder's name",
        &["after-desktop-close", "after-reconnect"],
    );
    Ok(())
}
