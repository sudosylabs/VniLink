//! Connections alone: who a host admits and what it refuses, what hosting,
//! disconnecting, revoking and unpairing end and release, and several
//! sessions at once on one host or one controller, each kept apart from
//! the others.

#[path = "connection/admission.rs"]
mod admission;
#[path = "connection/hosting.rs"]
mod hosting;
#[path = "connection/many_hosts.rs"]
mod many_hosts;
#[path = "connection/many_viewers.rs"]
mod many_viewers;
#[path = "connection/support.rs"]
mod support;
