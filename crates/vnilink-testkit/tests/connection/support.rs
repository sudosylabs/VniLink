// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! What the connection scenarios share: engines that trust each other
//! without sharing, a live connection ended and what a later connect meets
//! ([`check`]), a refused connection's reason, and a cursor to send.
//! Pairing for control, session waits and events are the kit's
//! ([`vnilink_testkit::session`], [`vnilink_testkit::events`]).

use expect_test::Expect;
use std::time::Duration;

use vnilink::{CursorState, Error, SessionPhase, SessionRole};
use vnilink_testkit::desktop::InputLog;
use vnilink_testkit::{assert_story, wait_for, Engine, Traced, DEFAULT_WAIT};

/// What a scenario fails with: an engine's error, or what it did not find.
pub type Failure = Box<dyn std::error::Error>;

/// How long an invitation in these scenarios lives.
pub const INVITATION: Duration = Duration::from_secs(60);

/// `controller` trusts `host`, and `host` trusts `controller`, without
/// either sharing anything or `host` hosting.
pub fn introduce(host: &Engine, controller: &Engine) -> Result<(), Error> {
    controller
        .core
        .accept_invitation(&host.core.create_invitation(INVITATION)?)?;
    host.core
        .accept_invitation(&controller.core.create_invitation(INVITATION)?)?;
    Ok(())
}

/// What ends a live services connection between a host and its
/// controller.
pub enum SessionEnd {
    /// The host stops hosting.
    StopHosting,
    /// The controller disconnects.
    Disconnect,
    /// The host unpairs the controller.
    Unpair,
    /// The controller unpairs the host.
    ControllerUnpair,
}

/// The monitor of the desktop a host in these scenarios shows.
pub const MONITOR: (u16, u16) = (1920, 1080);

/// What a later connect meets once a connection ended.
pub enum Later {
    /// The host refuses it, saying why.
    Refused(Error),
    /// The host admits it.
    Admitted,
    /// The controller no longer knows the host, so it does not dial.
    NotDialled,
}

/// How a scenario ends a connection, and what a later connect meets.
pub struct Closing {
    pub by: SessionEnd,
    pub later: Later,
}

/// Starts a host showing a desktop and a controller it paired for control,
/// connects them and ends their services connection as `closing.by` says;
/// asserts that both sides close, that nothing is left to disconnect, that
/// a later connect meets `closing.later`, that no record holds either
/// computer's full identity, and that the engines told the story `expect`
/// of their sessions, pairing and hosting. What a test checks beyond that
/// it checks on the host and the controller returned.
#[track_caller]
pub fn check(
    trace: &Traced,
    closing: Closing,
    expect: Expect,
) -> Result<(Engine, Engine), Failure> {
    let host = Engine::start("host");
    let controller = Engine::start("controller");
    InputLog::install(&host, MONITOR)?;
    host.pair_for_control(&controller)?;
    controller.connect_services(&host)?;
    match closing.by {
        SessionEnd::StopHosting => host.core.stop_hosting()?,
        SessionEnd::Disconnect => controller.core.disconnect()?,
        SessionEnd::Unpair => host.core.unpair(&controller.endpoint())?,
        SessionEnd::ControllerUnpair => controller.core.unpair(&host.endpoint())?,
    }
    host.wait_closed(SessionRole::Host);
    controller.wait_closed(SessionRole::Controller);
    assert!(
        matches!(controller.core.disconnect(), Err(Error::NoSession)),
        "an ended connection leaves nothing to disconnect"
    );
    match closing.later {
        Later::Refused(why) => assert_eq!(
            refusal(&controller, &host)?,
            why.to_string(),
            "the host turns a later connect away"
        ),
        Later::Admitted => assert_eq!(
            controller.connect_services(&host)?.phase,
            SessionPhase::Active,
            "the host admits a later connect"
        ),
        Later::NotDialled => assert!(
            matches!(
                controller.core.begin_connect_services(&host.endpoint()),
                Err(Error::NotPaired)
            ),
            "a controller that forgot its host is refused before dialing"
        ),
    }
    assert_story(expect, || trace.story(&["session", "pairing", "hosting"]));
    trace.assert_unrecorded(
        "a full peer identity",
        &[&host.endpoint(), &controller.endpoint()],
    );
    Ok((host, controller))
}

/// A visible cursor at `position`, without an image.
pub fn cursor_at(position: (i32, i32)) -> CursorState {
    CursorState {
        position,
        visible: true,
        image: None,
    }
}

/// Why `host` refused `controller`'s services connection, as the
/// controller's record of it says; the error when the controller did not
/// dial at all. A connection to `host` still active is let end first: a
/// new one would reuse it.
pub fn refusal(controller: &Engine, host: &Engine) -> Result<String, Error> {
    let peer = host.endpoint();
    wait_for("the earlier connection to end", DEFAULT_WAIT, || {
        controller
            .core
            .controller_sessions()
            .ok()?
            .iter()
            .all(|session| session.peer_id != peer || session.phase != SessionPhase::Active)
            .then_some(())
    });
    let pending = controller.core.begin_connect_services(&host.endpoint())?;
    Ok(wait_for(
        "the connection to be refused",
        DEFAULT_WAIT,
        || match controller.core.controller_session(pending.id).ok()?.phase {
            SessionPhase::Failed(why) => Some(why),
            SessionPhase::Active => panic!("the host admitted the connection"),
            _ => None,
        },
    ))
}
