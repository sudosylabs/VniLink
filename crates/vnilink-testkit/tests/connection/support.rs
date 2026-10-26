//! What the connection scenarios share: engines that trust each other
//! without sharing, and a cursor to send. Pairing for control, session waits
//! and events are the kit's ([`vnilink_testkit::session`],
//! [`vnilink_testkit::events`]).

use std::time::Duration;

use vnilink::{CursorState, Error};
use vnilink_testkit::Engine;

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

/// A visible cursor at `position`, without an image.
pub fn cursor_at(position: (i32, i32)) -> CursorState {
    CursorState {
        position,
        visible: true,
        image: None,
    }
}
