// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! What the connection scenarios share: engines that trust each other
//! without sharing, a live connection and what ended it, a refused
//! connection's reason, and a cursor to send.
//! Pairing for control, session waits and events are the kit's
//! ([`vnilink_testkit::session`], [`vnilink_testkit::events`]).

use std::time::Duration;

use vnilink::{CursorState, Error, SessionPhase, SessionRole};
use vnilink_testkit::desktop::InputLog;
use vnilink_testkit::{wait_for, Engine, DEFAULT_WAIT};

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
    /// The host revokes the controller.
    Revoke,
    /// The host unpairs the controller.
    Unpair,
    /// The controller unpairs the host.
    ControllerUnpair,
}

/// The monitor of the desktop a host in these scenarios shows.
pub const MONITOR: (u16, u16) = (1920, 1080);

/// A host showing a desktop and a controller it paired for control, whose
/// services connection `ending` ended, closed on both sides: the host and
/// the controller.
#[track_caller]
pub fn ended(ending: SessionEnd) -> Result<(Engine, Engine), Error> {
    let host = Engine::start("host");
    let controller = Engine::start("controller");
    InputLog::install(&host, MONITOR)?;
    host.pair_for_control(&controller)?;
    controller.connect_services(&host)?;
    match ending {
        SessionEnd::StopHosting => host.core.stop_hosting()?,
        SessionEnd::Disconnect => controller.core.disconnect()?,
        SessionEnd::Revoke => host.core.revoke(&controller.endpoint())?,
        SessionEnd::Unpair => host.core.unpair(&controller.endpoint())?,
        SessionEnd::ControllerUnpair => controller.core.unpair(&host.endpoint())?,
    }
    host.wait_closed(SessionRole::Host);
    controller.wait_closed(SessionRole::Controller);
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
