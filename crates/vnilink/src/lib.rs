//! Shared control library for VniLink.
//!
//! Native applications host this crate. It owns installation identity, relay
//! choice, pairing, authorization, the session lifecycle, keyboard and pointer
//! input, clipboard text, file transfer, and the encoded-frame stream. It does
//! not capture, encode, or draw the desktop, and it does not read the
//! operating-system clipboard. Call [`Core::enable_os_input`] on the host so
//! authorized sessions inject keys and pointer events. Call
//! [`Core::set_clipboard`] so a granted controller can paste text. Call
//! [`Core::set_file_roots`] so a granted peer can browse and receive files.
//! Call [`Core::publish_video`] with encoded H.264 frames and
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
//! record holds the installation secret, paired devices, global sharing consent, used
//! invitation nonces, and the relay setting, including a relay credential.
//! The host chooses where those bytes live. The production adapter encrypts
//! them in a local database and keeps only the encryption key in the platform
//! credential store. [`Core::start`] keeps the same state only for the process.
//! A locked, unavailable, or invalid record is returned as an error and is not
//! replaced. An uncertain commit stops this core until the process restarts.
//!
//! # Pairing file
//!
//! [`Core::export_pairing_file`] returns the bytes of a `.vnl` file. The other
//! computer opens that file and [`Core::confirm_pairing`] saves the device
//! when the typed digits match the writer's screen. The writer remembers the
//! importing computer after its owner confirms [`Core::control_check`].
//! Pairing does not grant services; [`Core::change_sharing_policy`] sets
//! installation-wide consent. The session uses the relays already
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
//! File transfer is a separate grant from control and from clipboard. The
//! owner approves roots with [`Core::set_file_roots`]; the peer browses
//! them with `fs_*` calls and moves bytes with resumable transfer jobs
//! (`job_submit_*`/`job_poll`). The steps are in `crates/vnilink/TRANSFER.md`.
//!
//! # Threading
//!
//! [`Core`] owns its Tokio runtime. Call it from the platform thread that is
//! not running inside that runtime. Poll [`EventSubscription`] on the UI
//! thread. The library does not deliver callbacks onto a UI queue.

#![forbid(unsafe_code)]

mod adaptation;
mod bitrate;
pub use bitrate::RateLimit;
mod address;
pub mod availability;
pub use adaptation::{AdaptationReason, DesktopMediaStatus};
pub mod audio;
pub mod camera;
mod clipboard;
mod codec;
mod core;
mod device_metadata;
pub mod diagnostics;
mod error;
mod events;
pub mod fs;
#[cfg(feature = "fuzzing")]
#[doc(hidden)]
pub mod fuzzing;
mod hostname;
mod input;
mod installation;
mod invitation;
pub mod snapshot;
pub use device_metadata::{DevicePresence, OperatingSystem};
pub mod job;
mod keyboard;
mod manifest;
mod os_input;
mod pairing;
mod protocol;
mod reconnect;
mod relay;
mod service;
mod session;
mod sidestream;
pub mod system;
pub mod terminal;
pub mod trace;
/// What tests reach in an engine that the application never does: hooks
/// that park work at an exact point, and limits a test must know. Built
/// for this crate's tests and the `test-probes` feature (the test kit's).
#[cfg(any(test, feature = "test-probes"))]
pub mod probes {
    #[cfg(unix)]
    pub use crate::core::PrestageHooks;
    /// The test agent's channel, which only the `test-probes` build has.
    #[cfg(feature = "test-probes")]
    pub use crate::core::{AgentChannel, AgentListener, AGENT_ALPN, MAX_AGENT_FRAME};
    pub use crate::core::{MAX_ACTIVE_JOBS, SERVE_SOURCE_PROBE};
    pub use crate::manifest::ScanHooks;
}
mod transfer;
mod trust;
mod video;
pub mod video_caps;

pub use availability::{
    evaluate, policy_descriptor, Availability, AvailabilityInputs, AvailabilityReason,
    BackendState, Capability, PolicyChangeScope, PolicyDescriptor, RemoteStatus, POLICIES,
};
pub use clipboard::{
    ClipboardFileEntry, FileSelection, HostClipboard, StoredFileSelection, MAX_CLIPBOARD_FILES,
    MAX_CLIPBOARD_FILES_BYTES, MAX_CLIPBOARD_TEXT,
};
pub use core::service_stream::RemoteFilesService;
pub use core::RemoteDesktopService;
pub use core::TopologySnapshot;
pub use core::{
    ControlRequest, Core, CoreConfig, DeviceInfo, Listen, OpenConfig, SessionPath, Snapshot,
    VideoRouting, VideoTarget,
};
pub use core::{DesktopControlState, DesktopInfo};
pub use diagnostics::{
    build_snapshot, export_json, file_name_only, fingerprint, monotonic_ms, ConnectionSummary,
    DiagnosticSnapshot, QueueDepths, ResourceCounts, ServiceSummary, SnapshotInputs,
    TransferSummary, MAX_EXPORT_BYTES, MAX_SNAPSHOT_LISTED, SNAPSHOT_VERSION,
};
pub use error::Error;
pub use events::{CoreEvent, EventSubscription};
pub use fs::{
    DirPage, EntryKind, EntryToken, FolderKind, FsEntry, Place, PlaceRoot, Places, RevisionGate,
    MAX_LISTED, MAX_PAGE, MAX_ROOTS,
};
pub use hostname::{clean_hostname, local_hostname};
pub use input::{
    desktop_bounds, display_for_point, map_point, map_window_to_desktop, usage, validate_topology,
    DisplayInfo, HostDesktop, InputEvent, PointerButton, PointerSettings, MAX_DESKTOP_ORIGIN,
    MAX_DISPLAYS, MAX_DISPLAY_EDGE, MAX_KEY_TEXT, MAX_PRECISE_WHEEL, WHEEL_DETENT,
};
pub use installation::{InstallationStore, MemoryStore};
pub use job::{ConflictChoice, JobDestination, JobDirection, JobItem, JobSnapshot, JobState};
pub use keyboard::{KeyboardMode, LocalKey};
pub use manifest::{
    keep_both_name, Manifest, ManifestEntry, MAX_MANIFEST_DEPTH, MAX_MANIFEST_FILES,
};
pub use os_input::{linux_evdev_keycode, OsInput};
pub use pairing::{PairingFile, PairingPreview, PAIRING_FILE_EXTENSION};
pub use protocol::{Capabilities, Quality, CODEC_H264, CODEC_H264_HIGH, PROTOCOL_VERSION};
pub use relay::{CustomRelay, RelaySettings};
pub use service::{
    CloseCause, DesktopEnd, ServiceInfo, ServiceKind, ServiceLease, ServicePermissions,
    ServicePhase, ServiceSupport, SharingPolicy, SharingPolicyUpdate, StreamEnd, StreamFailure,
    MAX_SERVICE_RECORDS,
};
pub use session::{Grants, SessionInfo, SessionPhase, SessionRole};
pub use system::{
    HostSessionState, HostSystemActions, OperationState, PrivacyState, ScheduledSystemAction,
    SystemAction, SystemOperation, SystemPolicy, SystemSupport, SYSTEM_ACTION_NOTICE,
};
pub use video::{
    picture_buffer_bits, CodecSupport, Condition, ConditionEntry, ConditionSet, EncodedFrame,
    Evidence, FrameRole, FrameTiming, HostCapture, RecordingVideoTap, Region, VideoCodec,
    VideoSubscription, YuvMatrix, MAX_REGIONS, MAX_VIDEO_FRAME, PICTURE_DRAIN, STILL_DRAIN,
};
pub use video_caps::{
    clamp_quality, negotiate as negotiate_video_codec, AgreedCodec, EncoderImplementation,
    EncoderInput, VideoCodecCaps, WindowDecoder,
};

#[cfg(test)]
pub(crate) mod test_support;

mod cursor;
pub use cursor::{CursorImage, CursorState, MAX_CURSOR_EDGE};

pub use core::RemoteAudioService;
pub use core::RemoteCameraService;
pub use core::RemoteSystemService;
pub use core::RemoteTerminalService;

pub use service::BackendStartup;

pub mod power;
