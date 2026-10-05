//! Shared control library for VniLink.
//!
//! Native applications host this crate. It owns installation identity, relay
//! choice, pairing, authorization, the session lifecycle, keyboard and pointer
//! input, clipboard text, file transfer, and the encoded-frame stream. It does
//! not capture, encode, or draw the desktop, and it does not read the
//! operating-system clipboard. Call [`Core::enable_os_input`] on the host so
//! authorized sessions inject keys and pointer events. Call
//! [`Core::set_clipboard`] so a granted controller can paste text. Call
//! [`Core::set_transfer_directory`] so a granted peer can send a file. Call
//! [`Core::publish_video`] with encoded H.264 frames and
//! [`Core::subscribe_video`] to receive them. [`Core::submit_local_key`] is
//! the key path. The default mode inserts the character the controller
//! produced. Map mode sends the physical HID position, and Translate mode
//! asks the host to press the chord in its own layout.
//! [`Core::submit_text`] types a finished string, and
//! [`Core::set_swap_control_and_command`] exchanges Control and Command.
//! The keyboard steps are in `crates/vnilink/KEYBOARD.md`.
//!
//! # Relay
//!
//! [`RelaySettings::public_n0`] is the default and selects Iroh's public n0
//! relays. [`RelaySettings::custom`] replaces that set with the installation's
//! own URLs. Custom mode does not publish the endpoint through n0 address lookup.
//!
//! # Installation record
//!
//! [`Core::open`] loads an opaque record through [`InstallationStore`]. The
//! record holds the installation secret, paired devices, grants, used
//! invitation nonces, and the relay setting, including a relay credential.
//! The host writes those bytes to the platform credential store. [`Core::start`]
//! keeps the same state only for the process. A locked, unavailable, or invalid
//! record is returned as an error and is not replaced.
//!
//! # Pairing file
//!
//! [`Core::export_pairing_file`] returns the bytes of a `.vnl` file. The other
//! computer opens that file and [`Core::confirm_pairing`] saves the device
//! when the typed digits match the writer's screen. Control is granted later,
//! on the writer's computer, when that person types the digits from
//! [`Core::control_check`]. The session then uses the relays already
//! configured for this installation. The steps are in `crates/vnilink/PAIRING.md`.
//!
//! # Reconnect
//!
//! A dropped control session dials that saved device again. The new session
//! has a new generation, and input queued for the old one is discarded. While
//! a session is up, each side stores the other's current home relay URLs and
//! direct addresses. A local disconnect or a host close does not redial.
//! The rules are in `crates/vnilink/RECONNECT.md`.
//!
//! # Clipboard
//!
//! Clipboard is a separate grant from control. Text is UTF-8 and at most
//! 256 KiB. [`Core::submit_clipboard`] sends it to the host.
//! [`Core::publish_clipboard`] sends the host's text to the controller.
//! Neither call reads or writes the operating-system clipboard. The steps
//! are in `crates/vnilink/CLIPBOARD.md`.
//!
//! # File transfer
//!
//! File transfer is a separate grant from control and from clipboard. A file
//! is at most 8 MiB. The name is a relative path under the directory from
//! [`Core::set_transfer_directory`]. An existing file is replaced only when
//! the sender asks. The steps are in `crates/vnilink/TRANSFER.md`.
//!
//! # Threading
//!
//! [`Core`] owns its Tokio runtime. Call it from the platform thread that is
//! not running inside that runtime. Poll [`EventSubscription`] on the UI
//! thread. The library does not deliver callbacks onto a UI queue.

#![forbid(unsafe_code)]

mod address;
mod clipboard;
mod codec;
mod core;
mod error;
mod events;
mod hostname;
mod input;
mod installation;
mod invitation;
mod keyboard;
mod os_input;
mod pairing;
mod protocol;
mod reconnect;
mod relay;
mod session;
mod sidestream;
mod transfer;
mod trust;
mod video;

pub use clipboard::{HostClipboard, MAX_CLIPBOARD_TEXT};
pub use core::{
    ControlRequest, Core, CoreConfig, DeviceInfo, Listen, OpenConfig, SessionPath, Snapshot,
};
pub use error::Error;
pub use events::{CoreEvent, EventSubscription};
pub use hostname::{clean_hostname, local_hostname};
pub use input::{map_point, usage, HostDesktop, InputEvent, PointerButton, MAX_KEY_TEXT};
pub use installation::{InstallationStore, MemoryStore};
pub use keyboard::{KeyboardMode, LocalKey};
pub use os_input::{linux_evdev_keycode, OsInput};
pub use pairing::{PairingFile, PairingPreview, PAIRING_FILE_EXTENSION};
pub use protocol::{Capabilities, Quality, PROTOCOL_VERSION};
pub use relay::{CustomRelay, RelaySettings};
pub use session::{Grants, SessionInfo, SessionPhase, SessionRole};
pub use transfer::MAX_FILE_BYTES;
pub use video::{EncodedFrame, HostCapture, VideoSubscription, MAX_VIDEO_FRAME};

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
