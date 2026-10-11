//! Directional feature permissions, independent of backend availability.
use super::*;
use crate::{CloseCause, ServiceKind, ServicePermissions, ServicePhase};

impl Core {
    /// Permissions this installation grants the paired computer. This is not
    /// a query of remote permissions or platform/backend support.
    pub fn service_permissions(&self, endpoint_id: &str) -> Result<ServicePermissions, Error> {
        let peer = parse_endpoint_id(endpoint_id)?;
        self.alive_state()?.trust.service_permissions(&peer)
    }

    /// Recording consent is directional and separate from desktop/Files grants.
    /// Persist before notifying active original Desktop owners.
    pub fn authorize_recording(&self, endpoint_id: &str, allowed: bool) -> Result<(), Error> {
        let peer = parse_endpoint_id(endpoint_id)?;
        let mut state = self.alive_state()?;
        let previous = state.trust.get(&peer).cloned().ok_or(Error::NotPaired)?;
        state.trust.authorize_recording(&peer, allowed)?;
        if let Err(error) = persist(&self.shared, &state) {
            state.trust.pair(previous)?;
            return Err(error);
        }
        let connections: Vec<_> = state
            .incoming
            .values()
            .filter(|s| s.peer_key == peer)
            .map(|s| s.id)
            .collect();
        for desktop in state
            .hosted_services
            .values()
            .filter(|r| connections.contains(&r.info.connection_id))
            .filter_map(|r| r.desktop.as_ref())
        {
            desktop.set_recording_allowed(allowed);
        }
        self.shared.events.publish(CoreEvent::DeviceAuthorized {
            endpoint_id: endpoint_id.into(),
        });
        Ok(())
    }

    /// Updates exactly one feature, persisting before any cancellation. A
    /// failed save restores the previous trust record and keeps live work.
    pub fn authorize_service(
        &self,
        endpoint_id: &str,
        kind: ServiceKind,
        allowed: bool,
    ) -> Result<(), Error> {
        let peer = parse_endpoint_id(endpoint_id)?;
        {
            let mut state = self.alive_state()?;
            let previous = state.trust.get(&peer).cloned().ok_or(Error::NotPaired)?;
            state.trust.authorize_service(&peer, kind, allowed)?;
            if let Err(error) = persist(&self.shared, &state) {
                state.trust.pair(previous)?;
                return Err(error);
            }
            if kind == ServiceKind::Desktop {
                let grants = state.trust.get(&peer).ok_or(Error::NotPaired)?.grants;
                super::desktop_stream::reconcile_grants(&self.shared, &mut state, &peer, grants);
            }
            if kind == ServiceKind::Files && previous.grants.transfer != allowed {
                let epoch = state.transfer_epochs.entry(peer).or_default();
                *epoch = epoch.saturating_add(1);
            }
            if !allowed {
                let incoming: Vec<u64> = state
                    .incoming
                    .values()
                    .filter(|s| s.peer_key == peer)
                    .map(|s| s.id)
                    .collect();
                for record in state
                    .hosted_services
                    .values_mut()
                    .filter(|r| incoming.contains(&r.info.connection_id) && r.info.kind == kind)
                {
                    if record.change(
                        record.info.generation,
                        ServicePhase::Closed(CloseCause::Revoked),
                    ) {
                        self.shared.events.publish(CoreEvent::HostedServiceChanged {
                            service: record.info.clone(),
                        });
                    }
                }
                if kind == ServiceKind::Desktop {
                    let targets: Vec<(u64, u64)> = state
                        .incoming
                        .values()
                        .filter(|s| s.peer_key == peer)
                        .filter_map(|s| s.desktop_owner.map(|owner| (owner, s.generation)))
                        .collect();
                    for (owner, generation) in targets {
                        super::desktop_service::retire_owner(&mut state, owner, generation, true);
                    }
                }
            }
        }
        if !allowed && kind == ServiceKind::Files {
            super::blob::drop_blob_offers(&self.shared, &peer);
            super::jobs::interrupt_peer_jobs(&self.shared, &peer);
        }
        super::service_jobs::cancel_orphaned(&self.shared);
        self.shared.events.publish(CoreEvent::DeviceAuthorized {
            endpoint_id: endpoint_id.into(),
        });
        Ok(())
    }
}

/// Permission gate for service factories. Keep backend support separate so a
/// permission never makes a missing terminal/camera implementation available.
pub(super) fn admitted(state: &State, peer: &[u8; 32], kind: ServiceKind) -> bool {
    state.trust.service_granted(peer, kind)
}
