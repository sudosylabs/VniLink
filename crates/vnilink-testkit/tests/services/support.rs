//! What the services scenarios share: a service's phase on the controller.
//! Waits on sessions, the closes an engine published and a host's whole
//! disk are the kit's ([`vnilink_testkit::session`],
//! [`vnilink_testkit::events`], [`vnilink_testkit::files::whole_disk`]).

use vnilink::{ServiceInfo, ServicePhase};
use vnilink_testkit::Engine;

/// The phase of `service` on the controller now.
pub(super) fn phase(controller: &Engine, service: &ServiceInfo) -> ServicePhase {
    controller
        .core
        .controller_service(service.id)
        .expect("the controller keeps the service's record")
        .phase
}
