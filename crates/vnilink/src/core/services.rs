//! Controller service records and cancellation. No service owns its peer link.

use super::*;
use crate::service::{
    CloseCause, ServiceInfo, ServiceKind, ServiceLease, ServicePhase, ServiceSupport,
};

pub(super) struct ServiceRecord {
    pub(super) info: ServiceInfo,
    pub(super) remote_id: Option<u64>,
    pub(super) stream_open: bool,
    pub(super) stop: tokio::sync::watch::Sender<bool>,
    pub(super) desktop: Option<Arc<super::desktop_stream::DesktopRuntime>>,
}

impl ServiceRecord {
    pub(super) fn change(&mut self, generation: u64, phase: ServicePhase) -> bool {
        if self.info.phase.finished()
            || (self.info.generation == generation && self.info.phase == phase)
        {
            return false;
        }
        tracing::debug!(
            target: crate::trace::SERVICE,
            service = self.info.id,
            kind = ?self.info.kind,
            generation,
            from = self.info.phase.label(),
            to = phase.label(),
            "service phase"
        );
        // Every non-active transition and generation change fences old work.
        self.stop.send_replace(true);
        self.stream_open = false;
        if let Some(desktop) = self.desktop.take() {
            desktop.close();
        }
        if phase == ServicePhase::Active {
            self.stop = tokio::sync::watch::channel(false).0;
        }
        self.info.generation = generation;
        self.info.phase = phase;
        true
    }
}

fn phase(session: &SessionPhase) -> ServicePhase {
    match session {
        SessionPhase::Active => ServicePhase::Active,
        SessionPhase::Reconnecting => ServicePhase::Suspended,
        SessionPhase::Stopping | SessionPhase::Closed => {
            ServicePhase::Closed(CloseCause::ParentLoss)
        }
        SessionPhase::Failed(message) => ServicePhase::Failed(message.clone()),
        _ => ServicePhase::Waiting,
    }
}

fn support(session: &LiveSession) -> ServiceSupport {
    ServiceSupport {
        desktop: session.services_only || (!session.files_only && !*session.desktop_stop.borrow()),
        files: true,
        terminal: session.services_only,
        camera: session.services_only,
        system_audio: session.services_only,
        microphone: session.services_only,
        system_actions: session.services_only,
    }
}

fn publish(events: &EventHub, info: &ServiceInfo) {
    events.publish(CoreEvent::ServiceChanged {
        service: info.clone(),
    });
}

/// Called under the parent state lock, so events cannot reorder close/reopen.
pub(super) fn synchronize(state: &mut State, events: &EventHub) {
    for record in state.hosted_services.values_mut() {
        let cause = if state.shut_down {
            Some(CloseCause::Shutdown)
        } else {
            match state.incoming.get(&record.info.connection_id) {
                None => Some(CloseCause::ParentLoss),
                // Pre-adoption race observation, as on the controller side:
                // hosted generations never bump in place, so this arm keeps
                // a mismatched observation truthful rather than routine.
                Some(session) if session.generation != record.info.generation => {
                    Some(CloseCause::Replaced)
                }
                Some(session) if session.phase != SessionPhase::Active => {
                    Some(CloseCause::ParentLoss)
                }
                _ => None,
            }
        };
        if let Some(cause) = cause {
            if record.change(record.info.generation, ServicePhase::Closed(cause)) {
                events.publish(CoreEvent::HostedServiceChanged {
                    service: record.info.clone(),
                });
            }
        }
    }
    for record in state.services.values_mut() {
        let (generation, next) = if state.shut_down {
            (
                record.info.generation,
                ServicePhase::Closed(CloseCause::Shutdown),
            )
        } else if let Some(session) = state.outgoing.get(&record.info.connection_id) {
            if session.generation != record.info.generation
                && !matches!(
                    phase(&session.phase),
                    ServicePhase::Waiting | ServicePhase::Active | ServicePhase::Suspended
                )
            {
                // Race window: the parent redialed and terminated before any
                // service read adopted the new epoch. Name the replacement
                // instead of a generic parent loss. Establish and the power
                // tick normally adopt first, so redial-then-close observes
                // ParentLoss; this arm keeps the pre-adoption observation
                // truthful rather than reachable-on-demand.
                (
                    session.generation,
                    ServicePhase::Closed(CloseCause::Replaced),
                )
            } else {
                (session.generation, phase(&session.phase))
            }
        } else {
            (
                record.info.generation,
                ServicePhase::Closed(CloseCause::ParentLoss),
            )
        };
        if record.change(generation, next) {
            publish(events, &record.info);
        }
    }
}

impl Core {
    /// Support on this connection. This does not bypass remote authorization.
    pub fn controller_service_support(&self, connection_id: u64) -> Result<ServiceSupport, Error> {
        let state = self.alive_state()?;
        let session = state.outgoing.get(&connection_id).ok_or(Error::NoSession)?;
        if finished(&session.phase) || session.phase == SessionPhase::Stopping {
            return Err(Error::NoSession);
        }
        Ok(support(session))
    }

    /// Attaches a local service owner to an existing outgoing connection.
    /// Pending admission is allowed, but work leases require Active. One live
    /// owner per kind/connection, except Desktop which allows a bounded set
    /// of simultaneous monitor owners; duplicate attachment returns Busy.
    /// This neither opens a backend nor changes the connection's wire mode.
    pub fn attach_controller_service(
        &self,
        connection_id: u64,
        generation: u64,
        kind: ServiceKind,
    ) -> Result<ServiceInfo, Error> {
        let mut state = self.alive_state()?;
        synchronize(&mut state, &self.shared.events);
        let session = state.outgoing.get(&connection_id).ok_or(Error::NoSession)?;
        if session.generation != generation {
            return Err(Error::StaleSession);
        }
        if finished(&session.phase) || session.phase == SessionPhase::Stopping {
            return Err(Error::NoSession);
        }
        let services_only = session.services_only;
        if !support(session).supports(kind) {
            return Err(Error::Unsupported(match kind {
                ServiceKind::Desktop => "desktop on a files-only connection",
                ServiceKind::Terminal => "terminal service",
                ServiceKind::Camera => "camera service",
                ServiceKind::SystemAudio => "system audio service",
                ServiceKind::Microphone => "microphone service",
                ServiceKind::Files => "files service",
                ServiceKind::System => "system service",
                ServiceKind::ScreenSnapshot => "screen snapshot on a files-only connection",
            }));
        }
        if state.services.len() >= crate::MAX_SERVICE_RECORDS {
            return Err(Error::Busy);
        }
        let live_same_kind = state
            .services
            .values()
            .filter(|r| {
                r.info.connection_id == connection_id
                    && r.info.kind == kind
                    && !r.info.phase.finished()
            })
            .count();
        // Simultaneous monitor owners need the per-owner services
        // transport; a legacy connection keeps the single-owner rule its
        // shared session pipeline requires.
        let allowed = match kind {
            ServiceKind::Terminal => usize::MAX,
            ServiceKind::Desktop if services_only => {
                super::desktop_stream::MAX_DESKTOP_OWNERS_PER_CONNECTION
            }
            _ => 1,
        };
        if live_same_kind >= allowed {
            return Err(Error::Busy);
        }
        let id = state.next_service_id;
        let next_id = id.checked_add(1).ok_or(Error::Busy)?;
        let info = ServiceInfo {
            id,
            connection_id,
            generation,
            peer_id: session.peer_id.clone(),
            kind,
            phase: phase(&session.phase),
        };
        let stop = tokio::sync::watch::channel(info.phase != ServicePhase::Active).0;
        state.services.insert(
            id,
            ServiceRecord {
                info: info.clone(),
                remote_id: None,
                stream_open: false,
                desktop: None,
                stop,
            },
        );
        state.next_service_id = next_id;
        publish(&self.shared.events, &info);
        Ok(info)
    }

    pub fn controller_services(&self) -> Result<Vec<ServiceInfo>, Error> {
        let mut state = self.alive_state()?;
        synchronize(&mut state, &self.shared.events);
        let mut records: Vec<_> = state.services.values().map(|r| r.info.clone()).collect();
        records.sort_by_key(|r| r.id);
        Ok(records)
    }

    pub fn controller_service(&self, id: u64) -> Result<ServiceInfo, Error> {
        let mut state = self.alive_state()?;
        synchronize(&mut state, &self.shared.events);
        Ok(state
            .services
            .get(&id)
            .ok_or(Error::NoSession)?
            .info
            .clone())
    }

    /// Resolves current transport identity, refusing closed/suspended or stale
    /// service generations. New adapters must use this and a cancellation lease
    /// rather than retaining an unchecked session id across asynchronous work.
    pub fn service_connection(&self, id: u64, generation: u64) -> Result<SessionInfo, Error> {
        let lease = self.service_lease(id, generation)?;
        let state = self.alive_state()?;
        let session = live_session(&state, false, lease.info.connection_id)?;
        if session.generation != generation || lease.is_cancelled() {
            return Err(Error::StaleSession);
        }
        Ok(session_info(session))
    }

    pub fn service_lease(&self, id: u64, generation: u64) -> Result<ServiceLease, Error> {
        let mut state = self.alive_state()?;
        synchronize(&mut state, &self.shared.events);
        let record = state.services.get(&id).ok_or(Error::NoSession)?;
        if record.info.generation != generation {
            return Err(Error::StaleSession);
        }
        if record.info.phase != ServicePhase::Active {
            return Err(Error::NoSession);
        }
        Ok(ServiceLease {
            info: record.info.clone(),
            stop: record.stop.subscribe(),
        })
    }

    /// Cancels only this owner/epoch. The peer connection and sibling services
    /// remain alive. Idempotent for a closed record of the same generation.
    pub fn close_controller_service(&self, id: u64, generation: u64) -> Result<(), Error> {
        let mut state = self.alive_state()?;
        synchronize(&mut state, &self.shared.events);
        let record = state.services.get_mut(&id).ok_or(Error::NoSession)?;
        if record.info.generation != generation {
            return Err(Error::StaleSession);
        }
        if record.change(generation, ServicePhase::Closed(CloseCause::OwnerClose)) {
            publish(&self.shared.events, &record.info);
        }
        super::desktop_service::retire_owner(&mut state, id, generation, false);
        drop(state);
        super::service_jobs::interrupt_orphaned(&self.shared);
        Ok(())
    }

    /// Releases a finished record. Never cancels running work.
    pub fn forget_controller_service(&self, id: u64) -> Result<(), Error> {
        let mut state = self.alive_state()?;
        synchronize(&mut state, &self.shared.events);
        let record = state.services.get(&id).ok_or(Error::NoSession)?;
        if !record.info.phase.finished() {
            return Err(Error::Busy);
        }
        state.services.remove(&id);
        Ok(())
    }
}
