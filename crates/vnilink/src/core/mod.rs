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

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8};
use std::sync::{Arc, Mutex, Weak};
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
use crate::transfer::TransferSender;
use crate::trust::{Device, Trust};
use crate::video::{HostCapture, VideoInbox, VideoOutbox};

pub(crate) mod blob;
mod clipboard;
mod connect;
mod cursor;
mod desktop_clipboard;
mod desktop_service;
mod desktop_stream;
mod files;
mod fs;
mod host;
mod jobs;
mod keys;
mod lifecycle;
mod link;
mod pairing;
mod service_jobs;
mod service_permissions;
pub(crate) mod service_stream;
mod services;
mod video;
mod wire;

pub use blob::{BLOB_ALPN, BLOB_MAX_BYTES};
pub use desktop_service::RemoteDesktopService;
pub use desktop_stream::DesktopInfo;

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
    desktop_owner: Option<u64>,
    monitor_width: u16,
    monitor_height: u16,
    stop: tokio::sync::watch::Sender<bool>,
    outbox: Option<Arc<InputOutbox>>,
    video: Option<Arc<VideoOutbox>>,
    /// Live connection. A test can close it without asking the session to stop.
    link: Option<iroh::endpoint::Connection>,
    clipboard: Option<ClipboardSender>,
    transfer: Option<TransferSender>,
}

/// All mutable controller channels belong to one session, including reconnects.
struct ControllerState {
    inbox: Arc<VideoInbox>,
    subscriber: Mutex<Weak<()>>,
    cursor: cursor::CursorInbox,
    clipboard: Mutex<Option<String>>,
    keyboard: Mutex<keyboard::KeyPlan>,
    held: Mutex<crate::input::HeldInput>,
    mode: AtomicU8,
    swap: AtomicBool,
    auto: Mutex<AutoState>,
}
impl ControllerState {
    fn new(inbox: Arc<VideoInbox>, mode: u8, swap: bool) -> Self {
        Self {
            inbox,
            subscriber: Mutex::new(Weak::new()),
            cursor: Arc::new(Mutex::new(None)),
            clipboard: Mutex::new(None),
            keyboard: Mutex::new(keyboard::KeyPlan::new()),
            held: Mutex::new(crate::input::HeldInput::default()),
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
}

struct State {
    hosting: bool,
    trust: Trust,
    replay: ReplayCache,
    /// Relay written into the installation record. The endpoint keeps the relay
    /// it bound at start until the next open.
    record_relay: RelaySettings,
    incoming: Option<LiveSession>,
    outgoing: HashMap<u64, LiveSession>,
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
    endpoint: Endpoint,
    secret: SecretKey,
    relay: RelaySettings,
    store: Option<Arc<dyn InstallationStore>>,
    events: EventHub,
    state: Mutex<State>,
    desktop: Mutex<Option<Box<dyn HostDesktop>>>,
    capture: Mutex<Option<Box<dyn HostCapture>>>,
    clipboard: Mutex<Option<Box<dyn HostClipboard>>>,
    // A native write can wait for the UI thread while holding the adapter
    // lock. UI snapshots must never acquire that lock to query availability.
    clipboard_available: AtomicBool,
    /// Latest text published by the controlled computer. Not logged.
    /// Canonical directory for files this installation accepts. Not logged.
    transfer_dir: Mutex<Option<PathBuf>>,
    video_inbox: Mutex<Option<Arc<VideoInbox>>>,
    video_subscriber: Mutex<Weak<()>>,
    /// One admitted host session at a time, including the release of held keys.
    session_gate: tokio::sync::Mutex<()>,
    desktop_gate: tokio::sync::Mutex<()>,
    /// Controller-only. Exchanges Control and Command before an event is queued.
    /// Not written into the installation record.
    swap_control_command: AtomicBool,
    /// Platform-reported H.264 codec availability. The library does not probe
    /// hardware itself; the native host sets these after asking its adapters
    /// (VA-API on Linux). Both default to false so a missing probe reads as
    /// unsupported, never as silent success.
    h264_encode: AtomicBool,
    h264_decode: AtomicBool,
    /// Controller-only. How a local key becomes host input. Not saved.
    keyboard_mode: AtomicU8,
    /// This host encodes H.264 High profile. Set by the platform after
    /// probing (VA-API High entrypoint on Linux, VideoToolbox on macOS).
    /// Defaults to false so a missing probe negotiates Baseline.
    h264_high_encode: AtomicBool,
    cursor_receiver: AtomicBool,
    cursor_outbox: tokio::sync::watch::Sender<
        Option<(crate::protocol::SessionStamp, Arc<crate::CursorState>)>,
    >,
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
    /// Transfer job records. Polled by each side; senders learn
    /// completion by asking the receiver.
    jobs: Arc<Mutex<HashMap<u64, jobs::JobRecord>>>,
    /// Next local job id. Adopted peer ids bump it past collisions.
    job_next: AtomicU64,
    job_save: Mutex<()>,
    /// Cancel watches for in-flight job fetches, one per active poll.
    job_cancel: Mutex<HashMap<u64, tokio::sync::watch::Sender<bool>>>,
    /// Job ids currently being advanced, preventing concurrent polls for
    /// the same job from both the background driver and window ticks.
    job_driving: Mutex<std::collections::HashSet<u64>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
struct AutoState {
    last_dropped: u64,
    last_change_ms: u64,
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
    let desktop = lock(&shared.desktop);
    desktop
        .as_ref()
        .map(|desktop| desktop.monitor())
        .unwrap_or((0, 0))
}

fn desktop_capabilities(shared: &Shared) -> Capabilities {
    let capture = lock(&shared.capture);
    let desktop = lock(&shared.desktop);
    let (width, height) = match capture.as_ref().map(|capture| capture.monitor()) {
        Some((width, height)) if width > 0 && height > 0 => (width, height),
        _ => desktop
            .as_ref()
            .map(|desktop| desktop.monitor())
            .unwrap_or((0, 0)),
    };
    Capabilities {
        desktop_capture: capture.is_some(),
        keyboard: desktop.is_some(),
        pointer: desktop.is_some(),
        clipboard: shared
            .clipboard_available
            .load(std::sync::atomic::Ordering::Acquire),
        file_transfer: lock(&shared.transfer_dir).is_some(),
        audio: false,
        monitors: u8::from(width > 0 && height > 0),
        h264_encode: capture.is_some()
            && shared
                .h264_encode
                .load(std::sync::atomic::Ordering::Relaxed),
        h264_decode: shared
            .h264_decode
            .load(std::sync::atomic::Ordering::Relaxed),
    }
}

/// True when this host captures but has no usable H.264 encoder. The caller
/// maps this to `Error::Unsupported` so the controller sees an explicit
/// unsupported-desktop reject instead of an admitted session with no video.
pub(super) fn video_unsupported(shared: &Shared) -> bool {
    lock(&shared.capture).is_some()
        && !shared
            .h264_encode
            .load(std::sync::atomic::Ordering::Relaxed)
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

/// Rewrites one device's grants with persist-or-rollback. Returns the previous
/// grants. Both authorize and revoke funnel through here so a failed save
/// never leaves memory ahead of the store.
fn store_grants(
    shared: &Shared,
    state: &mut State,
    key: &[u8; 32],
    grants: Grants,
    clear_service_grants: bool,
) -> Result<Grants, Error> {
    let previous = state
        .trust
        .get(key)
        .map(|device| device.grants)
        .ok_or(Error::NotPaired)?;
    let previous_device = state.trust.get(key).cloned().ok_or(Error::NotPaired)?;
    state.trust.authorize(key, grants)?;
    if clear_service_grants {
        state
            .trust
            .authorize_service(key, crate::ServiceKind::Terminal, false)?;
        state
            .trust
            .authorize_service(key, crate::ServiceKind::Camera, false)?;
    }
    if let Err(error) = persist(shared, state) {
        let _ = state.trust.pair(previous_device);
        return Err(error);
    }
    if previous.transfer != grants.transfer {
        let epoch = state.transfer_epochs.entry(*key).or_default();
        *epoch = epoch.saturating_add(1);
    }
    Ok(previous)
}

/// Removes one device with persist-or-rollback. Returns the removed device.
/// A failed save restores it and never leaves memory ahead of the store.
fn forget_device(shared: &Shared, state: &mut State, key: &[u8; 32]) -> Result<Device, Error> {
    let removed = state.trust.remove(key).ok_or(Error::NotPaired)?;
    if let Err(error) = persist(shared, state) {
        let _ = state.trust.pair(removed.clone());
        return Err(error);
    }
    Ok(removed)
}

struct Installed {
    trust: Trust,
    replay: ReplayCache,
    record_relay: RelaySettings,
    store: Option<Arc<dyn InstallationStore>>,
}

fn persist(shared: &Shared, state: &State) -> Result<(), Error> {
    let Some(store) = &shared.store else {
        return Ok(());
    };
    let bytes = installation::encode(
        &shared.secret.to_bytes(),
        &state.record_relay,
        &state.trust,
        &state.replay,
    )?;
    store.save(&bytes)
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poison| poison.into_inner())
}

/// The live session on one side. Lookup, active phase, and id match live here
/// so the clipboard, transfer, and control paths share one rule.
fn live_session(state: &State, incoming: bool, session_id: u64) -> Result<&LiveSession, Error> {
    let session = if incoming {
        state.incoming.as_ref()
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
