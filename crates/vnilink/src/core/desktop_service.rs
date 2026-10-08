//! Ownership bridge for an already negotiated desktop transport.
use super::*;
use crate::{ServiceInfo, ServiceLease, ServicePhase};

/// Retires desktop resources while retaining the connection and Files work.
/// Called under state; old-generation cleanup cannot touch a replacement.
pub(super) fn retire_owner(state: &mut State, id: u64, generation: u64, hosted: bool) {
    let session = if hosted {
        state.incoming.as_mut()
    } else {
        state
            .outgoing
            .values_mut()
            .find(|s| s.desktop_owner == Some(id))
    };
    let Some(session) =
        session.filter(|s| s.desktop_owner == Some(id) && s.generation == generation)
    else {
        return;
    };
    session.desktop_stop.send_replace(true);
    session.quality = None;
    session.video = None;
    session.separate_cursor = false;
    if let Some(outbox) = &session.outbox {
        outbox.clear();
        outbox.fail(&Error::Unsupported("the Desktop service has ended"));
    }
    if let Some(controller) = &session.controller {
        controller.inbox.close();
        *lock(&controller.cursor) = None;
    }
}

/// A lifetime handle for desktop capture/input already negotiated by Hello.
/// It does not own or disconnect the parent connection. Native renderers and
/// input adapters continue using the existing generation-scoped session APIs.
pub struct RemoteDesktopService {
    pub(super) lease: ServiceLease,
    pub(super) stop: tokio::sync::watch::Sender<bool>,
    pub(super) shared: Weak<Shared>,
    pub(super) desktop: Option<Arc<super::desktop_stream::DesktopRuntime>>,
}
impl RemoteDesktopService {
    pub fn info(&self) -> &ServiceInfo {
        self.lease.info()
    }
    pub fn is_cancelled(&self) -> bool {
        self.lease.is_cancelled()
    }
    pub fn close(&self) {
        if let Some(shared) = self.shared.upgrade() {
            finish_controller(&shared, self.info().id, self.info().generation, true);
        }
        self.stop.send_replace(true);
    }
}
impl Drop for RemoteDesktopService {
    fn drop(&mut self) {
        self.close();
    }
}

pub(super) fn finish_controller(shared: &Shared, id: u64, generation: u64, retire: bool) {
    let mut state = lock(&shared.state);
    if retire {
        retire_owner(&mut state, id, generation, false);
        if let Some(record) = state.services.get_mut(&id) {
            if record.info.generation == generation
                && record.change(generation, ServicePhase::Closed)
            {
                shared.events.publish(CoreEvent::ServiceChanged {
                    service: record.info.clone(),
                });
            }
        }
    }
}
