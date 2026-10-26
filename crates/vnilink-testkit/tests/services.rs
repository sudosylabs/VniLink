//! Services on a connection: how each is attached, admitted, carried across
//! a redial and closed beside its siblings; the host's consent to share
//! them; and whether each capability is available.

#[path = "services/admission.rs"]
mod admission;
#[path = "services/availability.rs"]
mod availability;
#[path = "services/closing.rs"]
mod closing;
#[path = "services/generations.rs"]
mod generations;
#[path = "services/revocation.rs"]
mod revocation;
#[path = "services/sharing.rs"]
mod sharing;
#[path = "services/siblings.rs"]
mod siblings;
#[path = "services/support.rs"]
mod support;
