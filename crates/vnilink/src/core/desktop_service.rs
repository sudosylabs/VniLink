//! Ownership bridge for an already negotiated desktop transport.
use super::*;
use crate::{CloseCause, ServiceInfo, ServiceLease, ServicePhase};

/// A viewer's Desktop: its own stream on a services connection, carrying
/// pictures one way and input the other. It does not own or disconnect the
/// parent connection.
pub struct RemoteDesktopService {
    pub(super) lease: ServiceLease,
    pub(super) stop: tokio::sync::watch::Sender<bool>,
    pub(super) shared: Weak<Shared>,
    pub(super) desktop: Arc<super::desktop_stream::DesktopRuntime>,
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
            finish_controller(
                &shared,
                self.info().id,
                self.info().generation,
                true,
                CloseCause::OwnerClose,
            );
        }
        self.stop.send_replace(true);
    }
}
impl Drop for RemoteDesktopService {
    fn drop(&mut self) {
        self.close();
    }
}

pub(super) fn finish_controller(
    shared: &Shared,
    id: u64,
    generation: u64,
    retire: bool,
    cause: CloseCause,
) {
    let mut state = lock(&shared.state);
    if retire {
        if let Some(record) = state.services.get_mut(&id) {
            if record.info.generation == generation
                && record.change(generation, ServicePhase::Closed(cause))
            {
                shared.events.publish(CoreEvent::ServiceChanged {
                    service: record.info.clone(),
                });
            }
        }
    }
}
