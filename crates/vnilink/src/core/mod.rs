//! Session runtime.
//!
//! Split by concern:
//! - [`lifecycle`] — open, shutdown, identity, relay
//! - [`host`] — hosting and the accept loop
//! - [`pairing`] — invitations, pairing files, control approval
//! - [`connect`] — authorize, connect, disconnect, redial
//! - [`keys`] — key, pointer, and keyboard mode
//! - [`clipboard`] — clipboard stream
//! - [`files`] — file transfer stream
//! - [`video`] — encoded frames
//! - [`wire`] — control frame read and write

use std::net::SocketAddr;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, AtomicU8};
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

mod clipboard;
mod connect;
mod files;
mod host;
mod keys;
mod lifecycle;
mod pairing;
mod video;
mod wire;

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

struct LiveSession {
    id: u64,
    generation: u64,
    role: SessionRole,
    phase: SessionPhase,
    peer_id: String,
    peer_key: [u8; 32],
    quality: Option<Quality>,
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

struct State {
    hosting: bool,
    trust: Trust,
    replay: ReplayCache,
    /// Relay written into the installation record. The endpoint keeps the relay
    /// it bound at start until the next open.
    record_relay: RelaySettings,
    incoming: Option<LiveSession>,
    outgoing: Option<LiveSession>,
    next_session_id: u64,
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
    /// Latest text published by the controlled computer. Not logged.
    clipboard_inbox: Mutex<Option<String>>,
    /// Canonical directory for files this installation accepts. Not logged.
    transfer_dir: Mutex<Option<PathBuf>>,
    video_inbox: Mutex<Option<Arc<VideoInbox>>>,
    video_subscriber: Mutex<Weak<()>>,
    /// One admitted host session at a time, including the release of held keys.
    session_gate: tokio::sync::Mutex<()>,
    /// Controller-only. Exchanges Control and Command before an event is queued.
    /// Not written into the installation record.
    swap_control_command: AtomicBool,
    /// Controller-only. How a local key becomes host input. Not saved.
    keyboard_mode: AtomicU8,
    /// Physical keys this controller has seen and not yet released.
    keyboard: Mutex<keyboard::KeyPlan>,
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
        clipboard: lock(&shared.clipboard).is_some(),
        file_transfer: lock(&shared.transfer_dir).is_some(),
        audio: false,
        monitors: u8::from(width > 0 && height > 0),
    }
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
