//! Shared control library for VniLink.
//!
//! Native applications host this crate. It owns installation identity, relay
//! choice, pairing, authorization, the session lifecycle, and keyboard and
//! pointer input. It does not capture or encode the desktop. Call
//! [`Core::enable_os_input`] on the host so authorized sessions inject keys
//! and pointer events into the operating system.
//!
//! # Relay
//!
//! [`RelaySettings::public_n0`] is the default and selects Iroh's public n0
//! relays. [`RelaySettings::custom`] replaces that set with the installation's
//! own URLs. Custom mode does not publish the endpoint through n0 address lookup.
//!
//! # Threading
//!
//! [`Core`] owns its Tokio runtime. Call it from the platform thread that is
//! not running inside that runtime. Poll [`EventSubscription`] on the UI
//! thread. The library does not deliver callbacks onto a UI queue.

#![forbid(unsafe_code)]

mod core;
mod error;
mod events;
mod input;
mod invitation;
mod os_input;
mod protocol;
mod relay;
mod session;
mod trust;

pub use core::{Core, CoreConfig, DeviceInfo, Listen, Snapshot};
pub use error::Error;
pub use events::{CoreEvent, EventSubscription};
pub use input::{usage, HostDesktop, InputEvent, PointerButton};
pub use os_input::OsInput;
pub use protocol::{Capabilities, Quality, PROTOCOL_VERSION};
pub use relay::{CustomRelay, RelaySettings};
pub use session::{Grants, SessionInfo, SessionPhase, SessionRole};

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
