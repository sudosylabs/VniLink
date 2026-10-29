//! What the suites wait for of a session and its services: a session
//! closed, reaching a phase, dialled again or hosted, the host asking its
//! owner, and a service ended.

use std::time::Duration;

use vnilink::{ControlRequest, Core, ServicePhase, SessionInfo, SessionPhase, SessionRole};

use crate::{wait_for, Engine, DEFAULT_WAIT};

/// Waits up to `within` until `core`'s session in `role` (its one incoming
/// or outgoing session) is closed.
pub fn wait_closed(core: &Core, role: SessionRole, within: Duration) {
    wait_for(&format!("the {role:?} session to close"), within, || {
        let snapshot = core.snapshot().ok()?;
        let session = match role {
            SessionRole::Host => snapshot.incoming,
            SessionRole::Controller => snapshot.outgoing,
        };
        (session?.phase == SessionPhase::Closed).then_some(())
    });
}

/// The request `host` asks its owner to approve, once it asks, within
/// `within`.
pub fn wait_request(host: &Core, within: Duration) -> ControlRequest {
    wait_for("the host to ask its owner", within, || {
        host.control_request().ok().flatten()
    })
}

/// Whether `phase` is a service's end: closed or failed.
pub fn service_ended(phase: &ServicePhase) -> bool {
    matches!(phase, ServicePhase::Closed(_) | ServicePhase::Failed(_))
}

/// Sessions: what an engine's sessions do, within the kit's wait.
impl Engine {
    /// Waits until this engine's session in `role` is closed.
    pub fn wait_closed(&self, role: SessionRole) {
        wait_closed(&self.core, role, DEFAULT_WAIT);
    }

    /// This engine's controller session `id`, once it reaches `phase`.
    pub fn wait_phase(&self, id: u64, phase: SessionPhase) -> SessionInfo {
        wait_for(
            &format!("controller session {id} to be {phase:?}"),
            DEFAULT_WAIT,
            || {
                self.core
                    .controller_session(id)
                    .ok()
                    .filter(|session| session.phase == phase)
            },
        )
    }

    /// This engine's controller session `id`, once active.
    pub fn active(&self, id: u64) -> SessionInfo {
        self.wait_phase(id, SessionPhase::Active)
    }

    /// `connection` on this engine once it is active again under a newer
    /// generation, after its link dropped and it dialled again.
    pub fn redialed(&self, connection: &SessionInfo) -> SessionInfo {
        wait_for("the connection to dial again", DEFAULT_WAIT, || {
            let next = self.core.controller_session(connection.id).ok()?;
            (next.phase == SessionPhase::Active && next.generation > connection.generation)
                .then_some(next)
        })
    }

    /// Waits until this engine hosts `count` sessions.
    pub fn wait_hosted(&self, count: usize) {
        wait_for(
            &format!("the host to host {count} sessions"),
            DEFAULT_WAIT,
            || (self.core.hosted_sessions().ok()?.len() == count).then_some(()),
        );
    }

    /// The session this engine hosts for `peer` now, if any.
    pub fn hosted_session(&self, peer: &Engine) -> Option<SessionInfo> {
        let peer = peer.endpoint();
        self.core
            .hosted_sessions()
            .expect("the hosted sessions")
            .into_iter()
            .find(|session| session.peer_id == peer)
    }

    /// The id of the session this engine hosts for `peer`.
    pub fn hosted_id(&self, peer: &Engine) -> u64 {
        self.hosted_session(peer)
            .expect("a session hosted for the peer")
            .id
    }

    /// Waits until this engine's owner is asked to admit a controller.
    pub fn wait_request(&self) -> ControlRequest {
        wait_request(&self.core, DEFAULT_WAIT)
    }
}
