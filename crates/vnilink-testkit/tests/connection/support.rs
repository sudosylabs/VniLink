//! What the connection scenarios share: pairing several engines for
//! control, waiting on a session's phase, and the events an engine
//! publishes.

use std::time::Duration;

use vnilink::{
    CoreEvent, CursorState, Error, EventSubscription, Grants, HostCapture, SessionInfo,
    SessionPhase, SessionRole,
};
use vnilink_testkit::{wait_for, Engine, DEFAULT_WAIT};

/// What a host shares once paired for control: it may be seen and
/// controlled.
pub const SEEN_AND_CONTROLLED: Grants = Grants {
    view: true,
    control: true,
    clipboard: false,
    transfer: false,
};

/// How long an invitation in these scenarios lives.
pub const INVITATION: Duration = Duration::from_secs(60);

/// `host` and `controller` trust each other as two paired computers do,
/// the host serving and sharing itself to be seen and controlled. A kit
/// [`vnilink_testkit::Pair`] does this for one host and one controller;
/// these scenarios pair several.
pub fn pair(host: &Engine, controller: &Engine) -> Result<(), Error> {
    host.pair_with(controller)?;
    host.core.set_desktop_sharing(SEEN_AND_CONTROLLED)
}

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

/// Waits until `engine`'s session in `role` (its one incoming or outgoing
/// session) is closed.
pub fn wait_closed(engine: &Engine, role: SessionRole) {
    wait_for(
        &format!("the {role:?} session to close"),
        DEFAULT_WAIT,
        || {
            let snapshot = engine.core.snapshot().ok()?;
            let session = match role {
                SessionRole::Host => snapshot.incoming,
                SessionRole::Controller => snapshot.outgoing,
            };
            (session?.phase == SessionPhase::Closed).then_some(())
        },
    );
}

/// `engine`'s controller session `id`, once it reaches `phase`.
pub fn wait_phase(engine: &Engine, id: u64, phase: SessionPhase) -> SessionInfo {
    wait_for(
        &format!("controller session {id} to be {phase:?}"),
        DEFAULT_WAIT,
        || {
            engine
                .core
                .controller_session(id)
                .ok()
                .filter(|session| session.phase == phase)
        },
    )
}

/// `engine`'s controller session `id`, once active.
pub fn active(engine: &Engine, id: u64) -> SessionInfo {
    wait_phase(engine, id, SessionPhase::Active)
}

/// Waits until `host` hosts `count` sessions.
pub fn wait_hosted(host: &Engine, count: usize) {
    wait_for(
        &format!("the host to host {count} sessions"),
        DEFAULT_WAIT,
        || (host.core.hosted_sessions().ok()?.len() == count).then_some(()),
    );
}

/// The id of the session `host` hosts for `peer`.
pub fn hosted_id(host: &Engine, peer: &Engine) -> u64 {
    let peer = peer.endpoint();
    host.core
        .hosted_sessions()
        .expect("the hosted sessions")
        .into_iter()
        .find(|session| session.peer_id == peer)
        .expect("a session hosted for the peer")
        .id
}

/// The next event `events` publishes that `wanted` picks.
pub fn next_event(events: &EventSubscription, wanted: impl Fn(&CoreEvent) -> bool) -> CoreEvent {
    wait_for("the engine to publish the event", DEFAULT_WAIT, || {
        events
            .recv_timeout(Duration::from_millis(20))
            .filter(|event| wanted(event))
    })
}

/// A host's capture of one 1920x1080 display that sends its cursor apart
/// from the picture.
pub struct CursorCapture;

impl HostCapture for CursorCapture {
    fn monitor(&self) -> (u16, u16) {
        (1920, 1080)
    }

    fn separate_cursor(&self) -> bool {
        true
    }
}

/// A visible cursor at `position`, without an image.
pub fn cursor_at(position: (i32, i32)) -> CursorState {
    CursorState {
        position,
        visible: true,
        image: None,
    }
}
