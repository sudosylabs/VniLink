//! Session runtime.
//!
//! Split by concern:
//! - [`lifecycle`] — open, shutdown, identity, relay
//! - [`host`] — hosting and the accept loop
//! - [`pairing`] — invitations, pairing files, control approval
//! - [`connect`] — authorize, connect, disconnect, redial
//! - [`keys`] — key, pointer, and keyboard mode
//! - [`link`] — the live link and the address refresh both roles share
//! - [`clipboard`] — clipboard stream
//! - [`files`] — file transfer stream
//! - [`video`] — encoded frames
//! - [`wire`] — control frame read and write

use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, Weak};
use std::time::Duration;

use iroh::{Endpoint, EndpointAddr, PublicKey, SecretKey};

use crate::clipboard::{ClipboardSender, HostClipboard};
use crate::error::Error;
use crate::events::{CoreEvent, EventHub};
use crate::input::{HostDesktop, InputOutbox};
use crate::installation::{self, InstallationStore};
use crate::invitation::ReplayCache;
use crate::keyboard::{self};
use crate::protocol::{Capabilities, Quality};
use crate::relay::RelaySettings;
use crate::session::{Grants, SessionInfo, SessionPhase, SessionRole};
use crate::trust::{Device, Trust};
use crate::video::{HostCapture, VideoInbox, VideoOutbox};

mod activity;
mod audio_stream;
mod availability;
pub(crate) mod blob;
mod camera_stream;
mod clipboard;
mod connect;
mod cursor;
mod desktop_clipboard;
mod desktop_service;
mod desktop_stream;
mod diagnostics;
mod file_resources;
mod fs;
mod host;
mod jobs;
#[cfg(unix)]
pub(crate) use jobs::PrestageHooks;
mod input_cleanup;
mod keys;
mod lifecycle;
mod link;
mod pairing;
mod power;
mod presence;
mod service_jobs;
mod service_permissions;
pub(crate) mod service_stream;
mod services;
mod sharing;
pub(crate) mod snapshot;
pub use audio_stream::RemoteAudioService;
pub use camera_stream::RemoteCameraService;
pub use system_stream::RemoteSystemService;
mod system_stream;
mod terminal_runtime;
mod terminal_stream;
mod topology;
mod video;
mod wire;

pub use blob::{BLOB_ALPN, BLOB_MAX_BYTES};
pub use desktop_service::RemoteDesktopService;
pub use desktop_stream::{DesktopControlState, DesktopInfo};
pub use terminal_stream::RemoteTerminalService;
pub use topology::TopologySnapshot;
pub use video::{VideoRouting, VideoTarget};

/// UDP sockets the endpoint binds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Listen {
    /// Unspecified IPv4 and IPv6, the ordinary internet install.
    #[default]
    Default,
    /// IPv4 loopback only. Same-machine tests use this.
    Loopback,
}

/// Per-installation settings. Nothing here is compiled in for a particular person.
#[derive(Clone)]
pub struct CoreConfig {
    pub relay: RelaySettings,
    pub listen: Listen,
    pub event_capacity: usize,
    /// Restored installation secret for [`Core::start`]. `None` generates one.
    /// [`Core::open`] ignores this and loads the secret from the installation store.
    pub identity: Option<[u8; 32]>,
}

/// Settings for [`Core::open`]. The saved record supplies the identity and relay
/// after the first successful open. `initial_relay` is used only while the store
/// is empty.
#[derive(Clone, Debug)]
pub struct OpenConfig {
    pub listen: Listen,
    pub event_capacity: usize,
    pub initial_relay: RelaySettings,
}

impl Default for CoreConfig {
    fn default() -> Self {
        Self {
            relay: RelaySettings::public_n0(),
            listen: Listen::Default,
            event_capacity: 64,
            identity: None,
        }
    }
}

impl Default for OpenConfig {
    fn default() -> Self {
        Self {
            listen: Listen::Default,
            event_capacity: 64,
            initial_relay: RelaySettings::public_n0(),
        }
    }
}

impl std::fmt::Debug for CoreConfig {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CoreConfig")
            .field("relay", &self.relay)
            .field("listen", &self.listen)
            .field("event_capacity", &self.event_capacity)
            .field("identity", &self.identity.as_ref().map(|_| "[redacted]"))
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceInfo {
    pub os: Option<crate::OperatingSystem>,
    pub presence: crate::DevicePresence,
    pub endpoint_id: String,
    pub hostname: Option<String>,
    pub pet_name: Option<String>,
    pub relays: Vec<String>,
    pub direct: Vec<SocketAddr>,
    pub grants: Grants,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot {
    pub endpoint_id: String,
    pub public_relay: bool,
    pub relay_urls: Vec<String>,
    pub hosting: bool,
    pub devices: Vec<DeviceInfo>,
    /// Latest hosted session, including finished records. Legacy single-view
    /// for callers that address one peer; use `hosted_sessions()` to iterate
    /// all live hosted peers.
    pub incoming: Option<SessionInfo>,
    pub outgoing: Option<SessionInfo>,
    pub capabilities: Capabilities,
}

/// One open transport path of a live session, for diagnostics and the
/// measurement hooks. Empty while the session has no connection yet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionPath {
    /// True when this path goes through a relay; false when direct.
    pub relay: bool,
    /// True when the connection currently sends application data here.
    pub selected: bool,
    /// Round-trip estimate in milliseconds.
    pub rtt_ms: u64,
    /// UDP payload bytes sent on this path, cumulative.
    pub tx_bytes: u64,
    /// UDP payload bytes received on this path, cumulative.
    pub rx_bytes: u64,
}

struct LiveSession {
    /// Local controller handle; host sessions retain their host id.
    id: u64,
    /// Peer-assigned protocol identity, never used for local controller lookup.
    wire_stamp: crate::protocol::SessionStamp,
    controller: Option<Arc<ControllerState>>,
    generation: u64,
    role: SessionRole,
    phase: SessionPhase,
    peer_id: String,
    peer_key: [u8; 32],
    quality: Option<Quality>,
    /// Agreed H.264 codec id (`CODEC_H264` or `CODEC_H264_HIGH`). Decided in
    /// Hello/Accept; the serve path opens the matching encoder profile.
    /// `None` for files-only sessions (no video).
    codec: u8,
    separate_cursor: bool,
    /// Files-only session (Transfer grant, no Control): suppresses desktop
    /// video, cursor, input and automatic clipboard transport. File roots,
    /// listings and transfers enforce Transfer per operation.
    files_only: bool,
    services_only: bool,
    /// A desktop may retire without ending its parent connection.
    desktop_stop: tokio::sync::watch::Sender<bool>,
    host_held: Arc<Mutex<crate::input::HeldInput>>,
    /// Native workers and cleanup retain admission after parent closure.
    incoming_slot: Option<Arc<input_cleanup::AdmissionSlot>>,
    /// One latest sample for this exact owner generation, never another peer.
    cursor_outbox: tokio::sync::watch::Sender<
        Option<(crate::protocol::SessionStamp, Arc<crate::CursorState>)>,
    >,
    /// Fences legacy native work across revoke/regrant and retirement.
    input_epoch: Arc<AtomicU64>,
    desktop_owner: Option<u64>,
    monitor_width: u16,
    monitor_height: u16,
    /// Topology last received from the host. Empty until the first snapshot.
    peer_topology: Vec<crate::input::DisplayInfo>,
    /// Display this hosted session is allowed to control. Empty until selected.
    selected_displays: Vec<u64>,
    /// Set when a selected display disappears. Not cleared by a later pointer.
    display_fault: Option<&'static str>,
    stop: tokio::sync::watch::Sender<bool>,
    outbox: Option<Arc<InputOutbox>>,
    video: Option<Arc<VideoOutbox>>,
    /// Live connection. A test can close it without asking the session to stop.
    link: Option<iroh::endpoint::Connection>,
    clipboard: Option<ClipboardSender>,
    /// Latest file selection this session's peer offered. Revisioned so a
    /// paste prepared earlier is stale, and cleared on revoke because a
    /// selection is a live capability, unlike inert text.
    clipboard_files: Option<crate::clipboard::StoredFileSelection>,
    /// This core's own file offers on this hosted session. Revisioned:
    /// paste preparation serves only the revision it was asked for, so a
    /// same-path re-offer cannot satisfy an older preparation. An empty
    /// offer stores an empty selection at a fresh revision instead of
    /// dropping the chain.
    published_files: Option<crate::clipboard::StoredFileSelection>,
    /// Next own-offer revision minus one. Monotonic across clears and
    /// revokes so a revision is never reused within the session.
    published_rev: AtomicU64,
    /// On a controller's link: the host's latest report of what it allows.
    remote_availability: Option<Vec<(crate::Capability, crate::RemoteStatus)>>,
}

/// All mutable controller channels belong to one session, including reconnects.
struct ControllerState {
    inbox: Arc<VideoInbox>,
    subscriber: Mutex<Weak<()>>,
    cursor: cursor::CursorInbox,
    clipboard: Mutex<Option<String>>,
    clipboard_files: Mutex<Option<crate::clipboard::StoredFileSelection>>,
    /// This core's own file offers on this session, for binding pastes to
    /// offers. Revisioned: a prepare naming an older revision is refused
    /// even when its paths match, and the Transfer grant alone never
    /// authorizes a fetch.
    published_files: Mutex<Option<crate::clipboard::StoredFileSelection>>,
    /// Next own-offer revision minus one. Monotonic across clears and
    /// revokes so a revision is never reused within the session.
    published_rev: AtomicU64,
    keyboard: Mutex<keyboard::KeyPlan>,
    held: Mutex<crate::input::HeldInput>,
    pointer: Mutex<crate::input::PointerPlan>,
    mode: AtomicU8,
    swap: AtomicBool,
    auto: Mutex<AutoState>,
}
impl ControllerState {
    fn new(inbox: Arc<VideoInbox>, mode: u8, swap: bool, pointer: crate::PointerSettings) -> Self {
        Self {
            inbox,
            subscriber: Mutex::new(Weak::new()),
            cursor: Arc::new(Mutex::new(None)),
            clipboard: Mutex::new(None),
            clipboard_files: Mutex::new(None),
            published_files: Mutex::new(None),
            published_rev: AtomicU64::new(0),
            keyboard: Mutex::new(keyboard::KeyPlan::new()),
            held: Mutex::new(crate::input::HeldInput::default()),
            pointer: Mutex::new(crate::input::PointerPlan::with_settings(pointer)),
            mode: AtomicU8::new(mode),
            swap: AtomicBool::new(swap),
            auto: Mutex::new(AutoState::default()),
        }
    }
}

impl State {
    /// Legacy APIs may address only an unambiguous outgoing session.
    fn single_outgoing(&self) -> Option<&LiveSession> {
        if self.outgoing.len() == 1 {
            return self.outgoing.values().next();
        }
        let mut live = self.outgoing.values().filter(|s| !finished(&s.phase));
        let Some(one) = live.next() else {
            return self.outgoing.values().max_by_key(|s| s.id);
        };
        if live.next().is_none() {
            Some(one)
        } else {
            None
        }
    }
    fn outgoing_peer(&self, peer: &[u8; 32]) -> Option<&LiveSession> {
        self.outgoing
            .values()
            .find(|s| s.peer_key == *peer && !finished(&s.phase))
    }
    /// Live hosted session for one peer, if any. Peers are looked up by
    /// identity, never by assuming a single hosted connection.
    fn incoming_peer(&self, peer: &[u8; 32]) -> Option<&LiveSession> {
        self.incoming
            .values()
            .find(|s| s.peer_key == *peer && !finished(&s.phase))
    }
    /// Live hosted session for one peer and wire generation. Job control
    /// frames carry no host session id, so they fence on peer identity plus
    /// the generation inside the message.
    fn incoming_live_for(&self, peer: &[u8; 32], generation: u64) -> Option<&LiveSession> {
        self.incoming.values().find(|s| {
            s.peer_key == *peer
                && s.wire_stamp.generation == generation
                && s.phase == SessionPhase::Active
        })
    }
    /// Explicit service input ownership precedes legacy desktop admission.
    /// A retiring service keeps its reservation until held input is released.
    fn input_holder(&self) -> Option<u64> {
        if self
            .input_cleanup
            .load(std::sync::atomic::Ordering::Acquire)
            != 0
        {
            return Some(u64::MAX);
        }
        if let Some(owner) = self.desktop_control {
            if let Some(record) = self.hosted_services.get(&owner.id) {
                if record.info.generation == owner.generation {
                    return Some(record.info.connection_id);
                }
            }
        }
        let mut holder: Option<u64> = None;
        let mut consider = |id: u64| {
            holder = Some(holder.map_or(id, |current| current.min(id)));
        };
        for session in self.incoming.values() {
            if session.phase == SessionPhase::Active
                && !session.files_only
                && !session.services_only
                && !*session.desktop_stop.borrow()
                && self
                    .trust
                    .granted(&session.peer_key, crate::session::Grant::Control)
            {
                consider(session.id);
            }
        }
        holder
    }
    /// A live hosted session, for legacy single-view callers. New code that
    /// already knows the session id must use `incoming.get(&id)` instead.
    fn incoming_live(&self) -> Option<&LiveSession> {
        self.incoming.values().find(|s| !finished(&s.phase))
    }
    /// The most recent hosted session, including finished records. Backs
    /// `Snapshot::incoming`, which reports the last session even after close.
    fn incoming_latest(&self) -> Option<&LiveSession> {
        self.incoming.values().max_by_key(|s| s.id)
    }
    fn incoming_live_mut(&mut self) -> Option<&mut LiveSession> {
        self.incoming.values_mut().find(|s| !finished(&s.phase))
    }
    /// Live hosted sessions other than presence links, which carry no
    /// service by themselves and are bounded by the paired devices instead.
    fn live_incoming_count(&self) -> usize {
        self.incoming
            .values()
            .filter(|s| !finished(&s.phase) && !s.services_only)
            .count()
    }
    fn incoming_admission_count(&self) -> usize {
        self.live_incoming_count().max(
            self.incoming_slots
                .load(std::sync::atomic::Ordering::Acquire),
        )
    }
    fn live_outgoing_count(&self) -> usize {
        self.outgoing
            .values()
            .filter(|s| !finished(&s.phase))
            .count()
    }
}

struct State {
    input_cleanup: Arc<AtomicUsize>,
    power_policy: crate::power::PowerPolicy,
    file_resource_policy: crate::job::FileResourcePolicy,
    file_retention_policy: crate::job::FileRetentionPolicy,
    terminal_policy: crate::terminal::TerminalPolicy,
    system_policy: crate::system::SystemPolicy,
    desktop_quality_default: crate::protocol::Quality,
    desktop_codec_default: Option<crate::VideoCodec>,
    system_operations: HashMap<u64, crate::system::SystemOperationRecord>,
    next_operation_id: u64,
    privacy: Option<crate::system::PrivacyLease>,
    terminal_pending: HashMap<[u8; 32], usize>,
    hosting: bool,
    trust: Trust,
    replay: ReplayCache,
    /// Relay written into the installation record. The endpoint keeps the relay
    /// it bound at start until the next open.
    record_relay: RelaySettings,
    /// Hosted peer connections by local session id. Connection-scoped: every
    /// lookup carries the session id, never a global current session.
    /// Capacity is enforced at admission (`MAX_LIVE_INCOMING`).
    incoming: HashMap<u64, LiveSession>,
    /// Live parents plus reservations retained by uncancellable native work.
    incoming_slots: Arc<AtomicUsize>,
    outgoing: HashMap<u64, LiveSession>,
    /// Hosted session selected for legacy session-clipboard sync. Host OS
    /// clipboard changes publish only to this session, and peer text applies
    /// to the OS clipboard only from it, so one peer's selection is never
    /// broadcast to another. Admission selects the new session; closing it
    /// hands selection to the lowest remaining live hosted session, if any.
    clipboard_owner: Option<u64>,
    /// Host-local Desktop service identity plus parent generation.
    desktop_control: Option<crate::protocol::SessionStamp>,
    /// Lock-after-control edge state. Grant sites arm it synchronously so
    /// a control session briefer than a power tick still locks; the tick
    /// observes ends and fires.
    lock_watch: crate::power::LockAfterControl,
    next_session_id: u64,
    services: HashMap<u64, services::ServiceRecord>,
    hosted_services: HashMap<u64, services::ServiceRecord>,
    next_service_id: u64,
    transfer_epochs: HashMap<[u8; 32], u64>,
    generation: u64,
    shut_down: bool,
    /// Latest `.vnl` export. Process-local, like hosting. The allow switch
    /// lives here and is not written into the file.
    pairing_window: Option<PairingWindow>,
    control_request: Option<PendingControl>,
    next_control_request: u64,
    /// Last admitted screenshot per peer. Entries expire after a minute.
    snapshot_recent: HashMap<[u8; 32], std::time::Instant>,
    /// Whether the application asked for presence links (`set_presence_links`).
    presence_links: bool,
    /// Reachability per paired computer while presence links run, by display id.
    presence: HashMap<String, crate::DevicePresence>,
    /// Paired computers whose last link was refused because they stopped hosting.
    link_refused: std::collections::HashSet<[u8; 32]>,
}

struct PairingWindow {
    expires_at_ms: u64,
    allow_control: bool,
}

struct PendingControl {
    id: u64,
    endpoint_id: [u8; 32],
    display_id: String,
    check: String,
    relays: Vec<String>,
    direct: Vec<SocketAddr>,
    decision: tokio::sync::oneshot::Sender<bool>,
}

/// A controller is waiting on this host. `check` matches [`Core::control_check`]
/// on that controller.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ControlRequest {
    pub endpoint_id: String,
    pub check: String,
}

struct Shared {
    /// Wakes one paired computer's link supervisor so it dials now.
    presence_wake: Mutex<HashMap<[u8; 32], Arc<tokio::sync::Notify>>>,
    /// Serializes persisted sharing changes and their native revocation cleanup.
    sharing_serial: Mutex<()>,
    power_factory: Mutex<power::Factory>,
    power_status: Mutex<[crate::power::PowerStatus; 2]>,
    file_retention_policy: Arc<Mutex<crate::job::FileRetentionPolicy>>,
    endpoint: Endpoint,
    secret: SecretKey,
    relay: RelaySettings,
    store: Option<Arc<dyn InstallationStore>>,
    /// Closes the endpoint after a storage fence without borrowing `Core`.
    runtime_handle: tokio::runtime::Handle,
    self_weak: Weak<Shared>,
    /// 0 idle, 1 closing, 2 finished. A claim that does not finish the close
    /// returns to 0 so a later shutdown can retry. The core stays fenced.
    teardown_phase: AtomicU8,
    teardown_lock: Mutex<()>,
    teardown_cv: Condvar,
    #[cfg(any(test, feature = "test-probes"))]
    close_count: AtomicUsize,
    /// Non-zero test budget, in milliseconds, for a blocked blob or endpoint
    /// close. A real close keeps the production limit.
    #[cfg(test)]
    teardown_budget_ms: AtomicU64,
    #[cfg(test)]
    block_blob_shutdown: AtomicBool,
    #[cfg(test)]
    block_endpoint_close: AtomicBool,
    events: EventHub,
    state: Mutex<State>,
    desktop: Arc<Mutex<Option<Box<dyn HostDesktop>>>>,
    /// Immutable adapter metadata, readable without waiting for native input.
    desktop_info: Mutex<Option<(u16, u16)>>,
    cleanup_executor: input_cleanup::Executor,
    desktop_owners: Arc<AtomicUsize>,
    input_cleanup: Arc<AtomicUsize>,
    capture: Mutex<Option<Box<dyn HostCapture>>>,
    clipboard: Mutex<Option<Box<dyn HostClipboard>>>,
    clipboard_file_source: Mutex<Option<std::sync::Arc<dyn crate::clipboard::HostClipboardFiles>>>,
    /// One in-flight paste preparation per session side, keyed
    /// `(incoming, session id)`. A second concurrent prepare is Busy.
    paste_waiters: Mutex<HashMap<(bool, u64), clipboard::PasteWaiter>>,
    /// Serving-side twin of `paste_waiters`: one worker per session side
    /// at a time, so repeated wire Prepares cannot spawn unbounded
    /// detached threads. Released by RAII when the worker settles.
    paste_preparing: Mutex<HashSet<(bool, u64)>>,
    /// Request nonce source for paste preparation. Each prepare binds its
    /// waiter and its wire answer to one value, so a late answer for a
    /// replaced selection cannot satisfy the live waiter.
    paste_nonce: AtomicU64,
    // A native write can wait for the UI thread while holding the adapter
    // lock. UI snapshots must never acquire that lock to query availability.
    clipboard_available: AtomicBool,
    camera_factory: Mutex<Option<Arc<dyn crate::camera::HostCameraFactory>>>,
    /// The host's user turned the camera off; it has a backend it could offer.
    camera_turned_off: std::sync::atomic::AtomicBool,
    snapshot_factory: Mutex<Option<Arc<dyn crate::snapshot::HostSnapshotFactory>>>,
    /// One attested desktop picture, replaced at most once a second.
    snapshot_tap: Mutex<Option<Arc<snapshot::AttestedShot>>>,
    /// Camera device ids with a live host serve. One opener per device;
    /// a second concurrent open is refused with structured Busy.
    camera_open: Mutex<HashSet<String>>,
    audio_factory: Mutex<Option<Arc<dyn crate::audio::HostAudioFactory>>>,
    terminal_factory: Mutex<Option<Arc<dyn crate::terminal::HostTerminalFactory>>>,
    system_actions: Mutex<Option<Arc<dyn crate::system::HostSystemActions>>>,
    /// Serializes native privacy/input transitions so a revoke restore can
    /// never interleave with an in-flight enable. Never held across network.
    privacy_serial: Mutex<()>,
    /// [`crate::system::SYSTEM_ACTION_NOTICE`], shortened by tests.
    system_notice: Mutex<Duration>,
    terminal_sessions: Mutex<HashMap<String, Arc<terminal_runtime::HostedTerminal>>>,
    /// Latest text published by the controlled computer. Not logged.
    video_inbox: Mutex<Option<Arc<VideoInbox>>>,
    video_subscriber: Mutex<Weak<()>>,
    /// One admitted host session at a time, including the release of held keys.
    session_gate: tokio::sync::Mutex<()>,
    /// Held only while capture, input or the clipboard adapter is replaced.
    /// Live Desktop streams are counted separately so viewers do not exclude
    /// each other.
    desktop_gate: tokio::sync::Mutex<()>,
    desktop_streams: AtomicUsize,
    /// Controller-only. Connections with a live desktop datagram router,
    /// keyed `(connection id, generation)`. One router per key owns
    /// `read_datagram`, so sibling owners never split each other's fragments.
    desktop_routers: Mutex<HashSet<(u64, u64)>>,
    topology: Mutex<topology::TopologySnapshot>,
    topology_watch: tokio::sync::watch::Sender<u64>,
    /// Controller-only. Exchanges Control and Command before an event is queued.
    /// Not written into the installation record.
    swap_control_command: AtomicBool,
    /// Platform-reported H.264 codec availability. The library does not probe
    /// Probe-validated video codec abilities. The native layer registers
    /// these after initializing its encoder and decoder; every bit means a
    /// backend processed a sample. Defaults to nothing so a missing probe
    /// reads as unsupported, never as silent success.
    video_caps: Mutex<crate::video_caps::VideoCodecCaps>,
    /// Controller-only. How a local key becomes host input. Not saved.
    keyboard_mode: AtomicU8,
    pointer_defaults: Mutex<crate::PointerSettings>,
    cursor_receiver: AtomicBool,
    /// Managed blob directory for the transfer manager. Empty until set.
    blob_dir: Mutex<Option<PathBuf>>,
    /// Owner-approved filesystem roots for the file manager. Empty until
    /// the owner sets them; pairing and grants never populate this.
    file_roots: Mutex<crate::fs::RootSet>,
    /// Persistent blob store shared by serving and fetching. Opened lazily
    /// inside the core runtime once the directory is set.
    blob_store: Mutex<Option<iroh_blobs::store::fs::FsStore>>,
    /// Serializes store opens: concurrent loads of one directory hang.
    blob_open: tokio::sync::Mutex<()>,
    /// Fetch authorizations: (peer, hash) pairs this installation serves.
    /// Legacy (non-job) blob API (`blob_offer`): pairing only, no active-job
    /// check (used by spike tests and legacy single-file paths).
    /// Job blob API uses `blob_job_offers` plus an active `Serving` job
    /// needing the hash, so completed/cancelled jobs never authorize.
    /// A hash is content, not capability; the grant is re-checked per
    /// connection on the receiver side.
    blob_offers: Arc<Mutex<blob::OfferSet>>,
    blob_job_offers: Mutex<blob::OfferSet>,
    /// Temporary import pins bridge preparation until durable job ownership.
    blob_recent_fetches: Arc<Mutex<HashMap<[u8; 32], (std::time::Instant, u64)>>>,
    blob_import_pins: Arc<Mutex<HashMap<[u8; 32], (std::time::Instant, iroh_blobs::api::TempTag)>>>,
    /// In-flight blob serving connections by peer, closed on revoke/unpair
    /// so an accepted request cannot finish after its offers are removed.
    blob_serving: Mutex<HashMap<[u8; 32], Vec<iroh::endpoint::Connection>>>,
    /// One fetching connection per peer, reused by later fetches so a
    /// small file does not pay for a new handshake; dropped on revoke/unpair.
    blob_fetching: Mutex<HashMap<[u8; 32], iroh::endpoint::Connection>>,
    /// Transfer job records. Polled by each side; senders learn
    /// completion by asking the receiver.
    jobs: Arc<Mutex<HashMap<u64, jobs::JobRecord>>>,
    /// Next local job id. Adopted peer ids bump it past collisions.
    job_next: AtomicU64,
    job_save: Mutex<()>,
    job_owner_gate: Mutex<()>,
    /// Cancel watches for in-flight job fetches, one per active poll.
    job_cancel: Mutex<HashMap<u64, tokio::sync::watch::Sender<bool>>>,
    /// Job ids currently being advanced, preventing concurrent polls for
    /// the same job from both the background driver and window ticks.
    job_driving: Mutex<std::collections::HashSet<u64>>,
    /// Advances whenever a job changes: its state, its decision or its
    /// bytes. Watchers compare it to the last value they saw instead of
    /// polling every job.
    job_revision: AtomicU64,
    /// Set and signalled with each job change, so the background driver
    /// takes the next step at once instead of at its next tick.
    job_wake: (Mutex<bool>, std::sync::Condvar),
    file_resources: Arc<Mutex<file_resources::Resources>>,
    /// Last-known backend facts per capability, published by adapters and
    /// serve loops. Read by the availability query; never guessed.
    backend_state:
        Mutex<HashMap<crate::availability::Capability, crate::availability::BackendState>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
struct AutoState {
    manual_session: Option<(u64, u64)>,
}

/// Owns the Iroh endpoint and the session rules.
///
/// Call methods from outside the runtime this core creates. The core never
/// invokes a UI callback. Read [`Core::snapshot`] for the current picture and
/// poll [`Core::subscribe`] on the UI thread. A full event queue drops events
/// for that subscriber and increments [`EventSubscription::lagged`] instead of
/// blocking the connection loop.
pub struct Core {
    runtime: tokio::runtime::Runtime,
    shared: Arc<Shared>,
}

impl Core {
    fn local_endpoint_id(&self) -> [u8; 32] {
        *self.shared.secret.public().as_bytes()
    }

    fn ensure_alive(&self) -> Result<(), Error> {
        if lock(&self.shared.state).shut_down {
            Err(Error::ShutDown)
        } else {
            Ok(())
        }
    }

    fn alive_state(&self) -> Result<std::sync::MutexGuard<'_, State>, Error> {
        let state = lock(&self.shared.state);
        if state.shut_down {
            Err(Error::ShutDown)
        } else {
            Ok(state)
        }
    }

    fn emit_session(
        &self,
        id: u64,
        generation: u64,
        role: SessionRole,
        phase: SessionPhase,
        peer_id: &str,
    ) {
        self.shared.events.publish(CoreEvent::SessionChanged {
            id,
            generation,
            role,
            phase,
            peer_id: peer_id.to_owned(),
        });
    }
}

impl Drop for Core {
    fn drop(&mut self) {
        self.shutdown();
    }
}

const CONNECT_WAIT: Duration = Duration::from_secs(75);
const CONTROL_APPROVAL_WAIT: Duration = Duration::from_secs(60);
/// How long a controller waits for the answer to its Hello. The host may park
/// an unpaired Hello for [`CONTROL_APPROVAL_WAIT`] while its owner types the
/// digits, so the controller outlasts that instead of hanging up first.
const HELLO_ANSWER_WAIT: Duration = CONTROL_APPROVAL_WAIT.saturating_add(Duration::from_secs(5));

fn load_device(shared: &Shared, peer_key: &[u8; 32]) -> Result<Device, Error> {
    lock(&shared.state)
        .trust
        .get(peer_key)
        .cloned()
        .ok_or(Error::NotPaired)
}

fn device_addr(device: &Device) -> Result<EndpointAddr, Error> {
    let key = PublicKey::from_bytes(&device.endpoint_id)
        .map_err(|_| Error::InvalidInvitation("endpoint id is not a valid key"))?;
    let mut addr = EndpointAddr::new(key);
    for relay in &device.relays {
        addr = addr.with_relay_url(crate::relay::parse_relay_url(relay)?);
    }
    for socket in &device.direct {
        addr = addr.with_ip_addr(*socket);
    }
    Ok(addr)
}

fn direct_addresses(endpoint: &Endpoint) -> Vec<SocketAddr> {
    endpoint
        .bound_sockets()
        .into_iter()
        .filter(|addr| !addr.ip().is_unspecified() && addr.port() != 0)
        .collect()
}

fn session_info(session: &LiveSession) -> SessionInfo {
    SessionInfo {
        id: session.id,
        generation: session.generation,
        role: session.role,
        phase: session.phase.clone(),
        peer_id: session.peer_id.clone(),
        quality: session.quality,
        monitor_width: session.monitor_width,
        monitor_height: session.monitor_height,
    }
}

fn monitor_size(shared: &Shared) -> (u16, u16) {
    let capture = lock(&shared.capture);
    if let Some(capture) = capture.as_ref() {
        let size = capture.monitor();
        if size.0 > 0 && size.1 > 0 {
            return size;
        }
    }
    lock(&shared.desktop_info).unwrap_or((0, 0))
}

fn desktop_capabilities(shared: &Shared) -> Capabilities {
    let capture = lock(&shared.capture);
    let desktop = *lock(&shared.desktop_info);
    let (width, height) = match capture.as_ref().map(|capture| capture.monitor()) {
        Some((width, height)) if width > 0 && height > 0 => (width, height),
        _ => desktop.unwrap_or((0, 0)),
    };
    Capabilities {
        desktop_capture: capture.is_some(),
        keyboard: desktop.is_some(),
        pointer: desktop.is_some(),
        clipboard: shared
            .clipboard_available
            .load(std::sync::atomic::Ordering::Acquire),
        file_transfer: !lock(&shared.file_roots).is_empty(),
        audio: false,
        monitors: u8::from(width > 0 && height > 0),
        h264_encode: capture.is_some()
            && lock(&shared.video_caps).supports_encode(crate::VideoCodec::H264),
        h264_decode: lock(&shared.video_caps).supports_decode(crate::VideoCodec::H264),
    }
}

/// True when this host captures but has no usable H.264 encoder. The caller
/// maps this to `Error::Unsupported` so the controller sees an explicit
/// unsupported-desktop reject instead of an admitted session with no video.
pub(super) fn video_unsupported(shared: &Shared) -> bool {
    lock(&shared.capture).is_some() && !lock(&shared.video_caps).can_encode()
}

fn finished(phase: &SessionPhase) -> bool {
    matches!(phase, SessionPhase::Closed | SessionPhase::Failed(_))
}

fn signal_stop(session: Option<&mut LiveSession>) {
    if let Some(session) = session {
        let _ = session.stop.send(true);
    }
}

fn parse_endpoint_id(value: &str) -> Result<[u8; 32], Error> {
    let key = PublicKey::from_str(value)
        .map_err(|_| Error::InvalidSetting("endpoint id is not a valid key"))?;
    Ok(*key.as_bytes())
}

/// Removes one device with persist-or-rollback. Returns the removed device.
/// A failed save restores it and never leaves memory ahead of the store.
fn forget_device(shared: &Shared, state: &mut State, key: &[u8; 32]) -> Result<Device, Error> {
    let previous = state.trust.clone();
    let removed = state.trust.remove(key).ok_or(Error::NotPaired)?;
    if let Err(error) = persist(shared, state) {
        state.trust = previous;
        return Err(error);
    }
    Ok(removed)
}

struct Installed {
    power_policy: crate::power::PowerPolicy,
    file_resource_policy: crate::job::FileResourcePolicy,
    file_retention_policy: crate::job::FileRetentionPolicy,
    terminal_policy: crate::terminal::TerminalPolicy,
    system_policy: crate::system::SystemPolicy,
    trust: Trust,
    replay: ReplayCache,
    record_relay: RelaySettings,
    store: Option<Arc<dyn InstallationStore>>,
}

fn persist(shared: &Shared, state: &mut State) -> Result<(), Error> {
    let Some(store) = &shared.store else {
        return Ok(());
    };
    let bytes = installation::encode(
        &shared.secret.to_bytes(),
        &state.record_relay,
        &state.trust,
        &state.replay,
        &state.terminal_policy,
        &state.file_retention_policy,
        &state.power_policy,
        &state.file_resource_policy,
        &state.system_policy,
    )?;
    save_installation(shared, state, store.as_ref(), &bytes)
}

/// Writes installation bytes. An uncertain commit fences this core before
/// the error returns. The caller still holds `state` and must drop it before
/// endpoint teardown can finish; teardown is scheduled, not run, here.
fn save_installation(
    shared: &Shared,
    state: &mut State,
    store: &dyn InstallationStore,
    bytes: &[u8],
) -> Result<(), Error> {
    // A panic leaves the durable outcome unknown and skips every `Err` fence.
    // Catch it while this caller still holds `state`, then report the same
    // restart-required error so the caller can roll the change back.
    let saved = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| store.save(bytes)));
    match saved {
        Ok(Err(Error::StoreRecoveryRequired)) | Err(_) => {
            fence_unrecoverable(shared, state);
            Err(Error::StoreRecoveryRequired)
        }
        Ok(other) => other,
    }
}

/// Stops admission and owned remote work. `shut_down` is set by the caller
/// before this runs so service synchronization reports shutdown.
fn stop_owned_work(state: &mut State, shared: &Shared) {
    state.pairing_window = None;
    state.control_request = None;
    for session in state.incoming.values_mut() {
        signal_stop(Some(session));
    }
    for session in state.outgoing.values_mut() {
        signal_stop(Some(session));
    }
    let jobs: Vec<_> = lock(&shared.job_cancel).values().cloned().collect();
    for stop in jobs {
        let _ = stop.send(true);
    }
    for terminal in lock(&shared.terminal_sessions).values() {
        terminal.stop.store(true, Ordering::Release);
    }
    services::synchronize(state, &shared.events);
}

fn fence_unrecoverable(shared: &Shared, state: &mut State) {
    if !state.shut_down {
        state.shut_down = true;
        stop_owned_work(state, shared);
    }
    schedule_endpoint_close(shared);
}

/// Claims endpoint close and runs it after `state` is free. Safe to call
/// while the caller still holds `state`: the task waits for that lock.
///
/// Blob shutdown and endpoint close each get the teardown budget. A blob
/// timeout still closes the endpoint. A close that does not finish releases
/// the claim so a later shutdown can retry. The core stays fenced.
fn schedule_endpoint_close(shared: &Shared) {
    if !claim_endpoint_close(shared) {
        return;
    }
    let Some(arc) = shared.self_weak.upgrade() else {
        mark_teardown_finished(shared);
        return;
    };
    let handle = arc.runtime_handle.clone();
    handle.spawn(async move {
        if !wait_until_state_unlocked(&arc).await {
            release_teardown_claim(&arc);
            return;
        }
        let blob = {
            let mut blobs = lock(&arc.blob_store);
            blobs.take()
        };
        let block_blob = take_blob_block(&arc);
        if blob.is_some() || block_blob {
            let limit = teardown_limit(&arc, block_blob);
            let stalled = tokio::time::timeout(limit, async move {
                if let Some(blob) = blob {
                    let _ = blob.shutdown().await;
                }
                if block_blob {
                    std::future::pending::<()>().await;
                }
            })
            .await
            .is_err();
            if stalled {
                eprintln!("vnilink: installation blob close exceeded 5s; restart required");
            }
        }
        if close_endpoint(&arc).await {
            mark_teardown_finished(&arc);
        } else {
            eprintln!("vnilink: installation endpoint close exceeded 5s; restart required");
            release_teardown_claim(&arc);
        }
    });
}

fn claim_endpoint_close(shared: &Shared) -> bool {
    if shared
        .teardown_phase
        .compare_exchange(0, 1, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return false;
    }
    #[cfg(any(test, feature = "test-probes"))]
    shared.close_count.fetch_add(1, Ordering::Relaxed);
    true
}

fn teardown_limit(shared: &Shared, shortened: bool) -> Duration {
    #[cfg(test)]
    if shortened {
        let millis = shared.teardown_budget_ms.load(Ordering::Acquire);
        if millis > 0 {
            return Duration::from_millis(millis);
        }
    }
    let _ = (shared, shortened);
    Duration::from_secs(5)
}

fn take_blob_block(shared: &Shared) -> bool {
    #[cfg(test)]
    {
        shared.block_blob_shutdown.swap(false, Ordering::AcqRel)
    }
    #[cfg(not(test))]
    {
        let _ = shared;
        false
    }
}

fn take_close_block(shared: &Shared) -> bool {
    #[cfg(test)]
    {
        shared.block_endpoint_close.swap(false, Ordering::AcqRel)
    }
    #[cfg(not(test))]
    {
        let _ = shared;
        false
    }
}

async fn wait_until_state_unlocked(shared: &Arc<Shared>) -> bool {
    let started = std::time::Instant::now();
    let budget = Duration::from_secs(5);
    loop {
        match shared.state.try_lock() {
            Ok(_guard) => return true,
            Err(std::sync::TryLockError::Poisoned(_)) => return true,
            Err(std::sync::TryLockError::WouldBlock) => {}
        }
        if started.elapsed() >= budget {
            eprintln!("vnilink: installation shutdown did not start within 5s; restart required");
            return false;
        }
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
}

async fn close_endpoint(shared: &Arc<Shared>) -> bool {
    let blocked = take_close_block(shared);
    let limit = teardown_limit(shared, blocked);
    let endpoint = shared.endpoint.clone();
    tokio::time::timeout(limit, async move {
        if blocked {
            std::future::pending::<()>().await;
        }
        endpoint.close().await;
    })
    .await
    .is_ok()
}

fn mark_teardown_finished(shared: &Shared) {
    let _guard = lock(&shared.teardown_lock);
    if shared
        .teardown_phase
        .compare_exchange(1, 2, Ordering::AcqRel, Ordering::Acquire)
        .is_ok()
    {
        shared.teardown_cv.notify_all();
    }
}

/// Returns the claim so a later shutdown can close the endpoint.
fn release_teardown_claim(shared: &Shared) {
    let _guard = lock(&shared.teardown_lock);
    if shared
        .teardown_phase
        .compare_exchange(1, 0, Ordering::AcqRel, Ordering::Acquire)
        .is_ok()
    {
        shared.teardown_cv.notify_all();
    }
}

/// Waits until a close finishes or its claim is released.
fn wait_for_endpoint_close(shared: &Shared) {
    let guard = lock(&shared.teardown_lock);
    let mut guard = guard;
    let started = std::time::Instant::now();
    loop {
        let phase = shared.teardown_phase.load(Ordering::Acquire);
        if phase != 1 {
            return;
        }
        let left = Duration::from_secs(5).saturating_sub(started.elapsed());
        if left.is_zero() {
            eprintln!("vnilink: installation endpoint close exceeded 5s; restart required");
            return;
        }
        let (next, wait) = shared
            .teardown_cv
            .wait_timeout(guard, left)
            .unwrap_or_else(|poison| poison.into_inner());
        guard = next;
        if wait.timed_out() && shared.teardown_phase.load(Ordering::Acquire) == 1 {
            eprintln!("vnilink: installation endpoint close exceeded 5s; restart required");
            return;
        }
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poison| poison.into_inner())
}

/// The live session on one side. Lookup, active phase, and id match live here
/// so the clipboard, transfer, and control paths share one rule.
fn live_session(state: &State, incoming: bool, session_id: u64) -> Result<&LiveSession, Error> {
    let session = if incoming {
        state.incoming.get(&session_id)
    } else {
        state.outgoing.get(&session_id)
    }
    .ok_or(Error::NoSession)?;
    if session.phase != SessionPhase::Active || session.id != session_id {
        return Err(Error::NoSession);
    }
    Ok(session)
}

/// Sends one side-stream request and waits for its reply. A full queue is
/// busy; a gone stream and a silent one end the wait.
fn round_trip<R>(
    tx: &tokio::sync::mpsc::Sender<R>,
    closed: &'static str,
    request: impl FnOnce(std::sync::mpsc::SyncSender<Result<(), Error>>) -> R,
) -> Result<(), Error> {
    let (reply_tx, reply_rx) = std::sync::mpsc::sync_channel(1);
    match tx.try_send(request(reply_tx)) {
        Ok(()) => {}
        Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => return Err(Error::Busy),
        Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => {
            return Err(Error::Unsupported(closed));
        }
    }
    match reply_rx.recv_timeout(Duration::from_secs(10)) {
        Ok(result) => result,
        Err(_) => Err(Error::Timeout),
    }
}
