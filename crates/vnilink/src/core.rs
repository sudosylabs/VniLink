use std::net::SocketAddr;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, SystemTime};

use iroh::endpoint::presets;
use iroh::{Endpoint, EndpointAddr, EndpointId, PublicKey, RelayMode, SecretKey, TransportAddr};

use crate::address::Announcement;
use crate::clipboard::{self, ClipboardRequest, ClipboardSender, HostClipboard};
use crate::error::Error;
use crate::events::{CoreEvent, EventHub, EventSubscription};
use crate::input::{self, HeldInput, HostDesktop, InputEvent, InputOutbox, Outbound};
use crate::installation::{self, InstallationStore};
use crate::invitation::{self, ReplayCache};
use crate::os_input::OsInput;
use crate::pairing::{self, PairingFile, PairingPreview};
use crate::protocol::{
    error_code, Capabilities, ControlMessage, KeyText, Quality, ALPN, MAX_ADDRESS_FRAME,
    MAX_CONTROL_FRAME, PROTOCOL_VERSION,
};
use crate::reconnect::{self, LinkEnd};
use crate::relay::RelaySettings;
use crate::session::{self, Grants, SessionInfo, SessionPhase, SessionRole};
use crate::transfer::{self, TransferRequest, TransferSender};
use crate::trust::{Device, Trust};
use crate::video::{
    decode_video_body, encode_video_body, EncodedFrame, HostCapture, KeyframeFlag, VideoInbox,
    VideoOutbox, VideoPush, VideoSubscription,
};

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
    pub fn start(config: CoreConfig) -> Result<Self, Error> {
        let relay_mode = config.relay.to_relay_mode()?;
        Self::boot(config, relay_mode, false)
    }

    /// Loads the installation record and binds an endpoint.
    ///
    /// An empty store creates an identity, saves it, and uses
    /// [`OpenConfig::initial_relay`]. The next open restores that identity,
    /// the paired devices, their grants, used invitations, and the relay
    /// setting. The host implements [`InstallationStore`] with the platform
    /// credential store. [`crate::MemoryStore`] keeps the bytes only in this
    /// process.
    ///
    /// A locked, unavailable, or invalid store leaves any existing bytes
    /// where they are. The saved relay is the one that is bound, including
    /// the public n0 relays when that is the saved setting.
    pub fn open(store: Arc<dyn InstallationStore>, config: OpenConfig) -> Result<Self, Error> {
        Self::open_with(store, config, false)
    }

    pub(crate) fn open_direct(
        store: Arc<dyn InstallationStore>,
        initial_relay: RelaySettings,
    ) -> Result<Self, Error> {
        Self::open_with(
            store,
            OpenConfig {
                listen: Listen::Loopback,
                event_capacity: 64,
                initial_relay,
            },
            true,
        )
    }

    pub(crate) fn boot(
        config: CoreConfig,
        relay_mode: RelayMode,
        direct_only: bool,
    ) -> Result<Self, Error> {
        Self::boot_installed(config, relay_mode, direct_only, None)
    }

    fn open_with(
        store: Arc<dyn InstallationStore>,
        config: OpenConfig,
        direct_only: bool,
    ) -> Result<Self, Error> {
        let loaded = match store.load()? {
            None => None,
            Some(bytes) => Some(installation::decode(&bytes)?),
        };
        let fresh = loaded.is_none();
        let (identity, relay, trust, replay) = match loaded {
            None => (
                None,
                config.initial_relay.clone(),
                Trust::default(),
                ReplayCache::new(),
            ),
            Some(record) => (
                Some(record.secret),
                record.relay,
                record.trust,
                record.replay,
            ),
        };
        let relay_mode = if direct_only {
            RelayMode::Disabled
        } else {
            relay.to_relay_mode()?
        };
        let core = Self::boot_installed(
            CoreConfig {
                relay: relay.clone(),
                listen: config.listen,
                event_capacity: config.event_capacity,
                identity,
            },
            relay_mode,
            direct_only,
            Some(Installed {
                trust,
                replay,
                record_relay: relay,
                store: Some(store),
            }),
        )?;
        if fresh {
            let saved = {
                let state = lock(&core.shared.state);
                persist(&core.shared, &state)
            };
            if let Err(error) = saved {
                core.shutdown();
                return Err(error);
            }
        }
        Ok(core)
    }

    fn boot_installed(
        config: CoreConfig,
        relay_mode: RelayMode,
        direct_only: bool,
        installed: Option<Installed>,
    ) -> Result<Self, Error> {
        let event_capacity = match config.event_capacity {
            1..=1024 => config.event_capacity,
            0 => return Err(Error::InvalidSetting("event capacity must be at least 1")),
            _ => return Err(Error::InvalidSetting("event capacity must be at most 1024")),
        };
        let secret = match config.identity {
            Some(bytes) => SecretKey::from_bytes(&bytes),
            None => SecretKey::generate(),
        };
        let bound_relay = config.relay.clone();
        let (trust, replay, record_relay, store) = match installed {
            Some(installed) => (
                installed.trust,
                installed.replay,
                installed.record_relay,
                installed.store,
            ),
            None => (
                Trust::default(),
                ReplayCache::new(),
                bound_relay.clone(),
                None,
            ),
        };
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .thread_name("vnilink")
            .build()
            .map_err(Error::network)?;
        let listen = if direct_only {
            Listen::Loopback
        } else {
            config.listen
        };
        let endpoint = runtime.block_on(bind_endpoint(
            secret.clone(),
            relay_mode,
            listen,
            direct_only || !bound_relay.is_public(),
        ))?;
        let shared = Arc::new(Shared {
            endpoint,
            secret,
            relay: bound_relay,
            store,
            events: EventHub::new(event_capacity),
            state: Mutex::new(State {
                hosting: false,
                trust,
                replay,
                record_relay,
                incoming: None,
                outgoing: None,
                next_session_id: 1,
                generation: 0,
                shut_down: false,
                pairing_window: None,
                control_request: None,
                next_control_request: 1,
            }),
            desktop: Mutex::new(None),
            capture: Mutex::new(None),
            clipboard: Mutex::new(None),
            clipboard_inbox: Mutex::new(None),
            transfer_dir: Mutex::new(None),
            video_inbox: Mutex::new(None),
            video_subscriber: Mutex::new(Weak::new()),
            session_gate: tokio::sync::Mutex::new(()),
            swap_control_command: AtomicBool::new(false),
        });
        let accept = Arc::clone(&shared);
        runtime.spawn(async move { accept_loop(accept).await });
        Ok(Self { runtime, shared })
    }

    pub fn shutdown(&self) {
        {
            let mut state = lock(&self.shared.state);
            if state.shut_down {
                return;
            }
            state.shut_down = true;
            state.pairing_window = None;
            state.control_request = None;
            signal_stop(state.incoming.as_mut());
            signal_stop(state.outgoing.as_mut());
        }
        *lock(&self.shared.clipboard_inbox) = None;
        let endpoint = self.shared.endpoint.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        self.runtime.spawn(async move {
            endpoint.close().await;
            let _ = tx.send(());
        });
        let _ = rx.recv_timeout(Duration::from_secs(5));
    }

    pub fn endpoint_id(&self) -> String {
        self.shared.secret.public().to_string()
    }

    /// Secret bytes for the platform credential store. Do not log them.
    ///
    /// [`Core::open`] already saves this secret in the installation record.
    /// After [`Core::reset_identity`], the new secret is only in the store.
    /// This stopped core still reports the key it bound.
    pub fn export_identity(&self) -> [u8; 32] {
        self.shared.secret.to_bytes()
    }

    /// Saves the relay the next [`Core::open`] binds.
    ///
    /// The running endpoint, [`Core::snapshot`], and new invitations keep the
    /// relay bound at start. A credential stays in the installation record and
    /// out of [`Debug`]. This does not dial the relay.
    pub fn set_relay(&self, relay: RelaySettings) -> Result<(), Error> {
        relay.to_relay_mode()?;
        let mut state = self.alive_state()?;
        if self.shared.store.is_none() {
            return Err(Error::InvalidSetting(
                "saving the relay requires an installation store",
            ));
        }
        let previous = state.record_relay.clone();
        state.record_relay = relay;
        if let Err(error) = persist(&self.shared, &state) {
            state.record_relay = previous;
            return Err(error);
        }
        Ok(())
    }

    /// Replaces the saved identity and forgets paired devices, grants, and
    /// used invitations. The saved relay setting stays.
    ///
    /// The running endpoint stops. Open the store again for the new endpoint
    /// id. There is no path back to the previous identity or its grants. A
    /// locked or unavailable store leaves this process and the saved record
    /// unchanged.
    pub fn reset_identity(&self) -> Result<(), Error> {
        {
            let state = self.alive_state()?;
            let store = self.shared.store.as_ref().ok_or(Error::InvalidSetting(
                "resetting identity requires an installation store",
            ))?;
            let secret = SecretKey::generate();
            let bytes = installation::encode(
                &secret.to_bytes(),
                &state.record_relay,
                &Trust::default(),
                &ReplayCache::new(),
            )?;
            store.save(&bytes)?;
        }
        self.shutdown();
        Ok(())
    }

    pub fn subscribe(&self) -> Result<EventSubscription, Error> {
        self.ensure_alive()?;
        self.shared.events.subscribe()
    }

    /// Receives encoded frames for the controlling session. One subscriber at a time.
    ///
    /// Frames are not core events. A slow subscriber does not block input. Drop the
    /// subscription to free the slot.
    pub fn subscribe_video(&self) -> Result<VideoSubscription, Error> {
        self.ensure_alive()?;
        let mut slot = lock(&self.shared.video_subscriber);
        if slot.upgrade().is_some() {
            return Err(Error::Busy);
        }
        let inbox = video_inbox(&self.shared);
        let alive = Arc::new(());
        *slot = Arc::downgrade(&alive);
        Ok(VideoSubscription::new(inbox, alive))
    }

    pub fn snapshot(&self) -> Result<Snapshot, Error> {
        let state = lock(&self.shared.state);
        if state.shut_down {
            return Err(Error::ShutDown);
        }
        Ok(Snapshot {
            endpoint_id: self.endpoint_id(),
            public_relay: self.shared.relay.is_public(),
            relay_urls: self.shared.relay.relay_urls()?,
            hosting: state.hosting,
            devices: state
                .trust
                .devices()
                .map(|device| DeviceInfo {
                    endpoint_id: device.display_id.clone(),
                    relays: device.relays.clone(),
                    direct: device.direct.clone(),
                    grants: device.grants,
                })
                .collect(),
            incoming: state.incoming.as_ref().map(session_info),
            outgoing: state.outgoing.as_ref().map(session_info),
            capabilities: desktop_capabilities(&self.shared),
        })
    }

    pub fn capabilities(&self) -> Capabilities {
        desktop_capabilities(&self.shared)
    }

    /// Installs the adapter that applies keyboard and pointer events on this host.
    ///
    /// Refused while a controlling session is active. The session releases held
    /// keys when it ends, so swap the adapter between sessions.
    pub fn set_desktop(&self, desktop: Box<dyn HostDesktop>) -> Result<(), Error> {
        let state = self.alive_state()?;
        if state
            .incoming
            .as_ref()
            .is_some_and(|session| !finished(&session.phase))
        {
            return Err(Error::Busy);
        }
        drop(state);
        *lock(&self.shared.desktop) = Some(desktop);
        Ok(())
    }

    /// Reports the monitor this host captures.
    ///
    /// The platform captures and encodes, then passes bytes to [`Core::publish_video`].
    /// Refused while a controlling session is active.
    pub fn set_capture(&self, capture: Box<dyn HostCapture>) -> Result<(), Error> {
        let state = self.alive_state()?;
        if state
            .incoming
            .as_ref()
            .is_some_and(|session| !finished(&session.phase))
        {
            return Err(Error::Busy);
        }
        drop(state);
        *lock(&self.shared.capture) = Some(capture);
        Ok(())
    }

    /// Installs the adapter that receives clipboard text on this host.
    ///
    /// The library does not read or write the operating-system clipboard.
    /// Refused while a controlling session is active.
    pub fn set_clipboard(&self, clipboard: Box<dyn HostClipboard>) -> Result<(), Error> {
        let state = self.alive_state()?;
        if state
            .incoming
            .as_ref()
            .is_some_and(|session| !finished(&session.phase))
        {
            return Err(Error::Busy);
        }
        drop(state);
        *lock(&self.shared.clipboard) = Some(clipboard);
        Ok(())
    }

    /// Chooses the only directory this installation will write transferred files into.
    ///
    /// The directory is created when it is missing. A symlink is followed only
    /// for this path. Files inside it are not allowed to leave it, and a
    /// symlink there is not a destination. Refused while a controlling session
    /// is active. The transfer grant is separate and stays off until
    /// [`Core::authorize`] sets it.
    pub fn set_transfer_directory(&self, path: impl AsRef<std::path::Path>) -> Result<(), Error> {
        let state = self.alive_state()?;
        if state
            .incoming
            .as_ref()
            .is_some_and(|session| !finished(&session.phase))
        {
            return Err(Error::Busy);
        }
        drop(state);
        let canonical = transfer::prepare_directory(path.as_ref())?;
        *lock(&self.shared.transfer_dir) = Some(canonical);
        Ok(())
    }

    /// Inject authorized keyboard and pointer events into this operating system.
    ///
    /// On macOS this can prompt for Accessibility permission. It does not capture
    /// the screen. Linux support is the X11 backend; Wayland is not claimed.
    pub fn enable_os_input(&self) -> Result<(), Error> {
        self.set_desktop(Box::new(OsInput::open()?))
    }

    pub fn start_hosting(&self) -> Result<(), Error> {
        let mut state = self.alive_state()?;
        state.hosting = true;
        drop(state);
        self.shared
            .events
            .publish(CoreEvent::HostingChanged { hosting: true });
        Ok(())
    }

    pub fn stop_hosting(&self) -> Result<(), Error> {
        let active = {
            let state = self.alive_state()?;
            state
                .incoming
                .as_ref()
                .is_some_and(|session| !finished(&session.phase))
        };
        if active {
            match self.stop_incoming() {
                Ok(()) | Err(Error::NoSession) => {}
                Err(error) => return Err(error),
            }
        }
        let mut state = self.alive_state()?;
        state.hosting = false;
        drop(state);
        self.shared
            .events
            .publish(CoreEvent::HostingChanged { hosting: false });
        Ok(())
    }

    /// Ends the hosted session and leaves hosting enabled.
    pub fn stop_hosted_session(&self) -> Result<(), Error> {
        self.stop_incoming()
    }

    pub fn create_invitation(&self, ttl: Duration) -> Result<String, Error> {
        self.ensure_alive()?;
        let relays = self.shared.relay.relay_urls()?;
        let direct = direct_addresses(&self.shared.endpoint);
        let (token, _) = invitation::issue(
            &self.shared.secret,
            &relays,
            &direct,
            ttl,
            SystemTime::now(),
        )?;
        Ok(token)
    }

    /// Builds a `.vnl` file for another computer to open.
    ///
    /// `ttl` follows the invitation rules: 1 second through 15 minutes.
    /// `allow_control` stays on this computer. While the file is unexpired,
    /// one incoming controller can be granted control after
    /// [`Core::confirm_control_request`]. The returned bytes do not contain
    /// that switch. The app writes them to a file ending in
    /// [`crate::PAIRING_FILE_EXTENSION`]. The steps are in
    /// `crates/vnilink/PAIRING.md`.
    pub fn export_pairing_file(
        &self,
        ttl: Duration,
        allow_control: bool,
    ) -> Result<PairingFile, Error> {
        self.export_pairing_file_at(ttl, allow_control, SystemTime::now())
    }

    pub(crate) fn export_pairing_file_at(
        &self,
        ttl: Duration,
        allow_control: bool,
        now: SystemTime,
    ) -> Result<PairingFile, Error> {
        self.ensure_alive()?;
        let relays = self.shared.relay.relay_urls()?;
        let direct = direct_addresses(&self.shared.endpoint);
        let (token, invitation) =
            invitation::issue(&self.shared.secret, &relays, &direct, ttl, now)?;
        let (_, nonce) = invitation::open(&token, now)?;
        let bytes = pairing::enclose(&token)?;
        let check = pairing::file_digits(&invitation.endpoint_id, &nonce, invitation.expires_at_ms);
        let mut state = self.alive_state()?;
        state.pairing_window = Some(PairingWindow {
            expires_at_ms: invitation.expires_at_ms,
            allow_control,
        });
        Ok(PairingFile {
            bytes,
            check,
            expires_at_ms: invitation.expires_at_ms,
        })
    }

    /// Reads a `.vnl` file. This does not save trust.
    ///
    /// Show `preview.check` and confirm with the digits on the computer that
    /// wrote the file.
    pub fn open_pairing_file(&self, bytes: &[u8]) -> Result<PairingPreview, Error> {
        self.open_pairing_file_at(bytes, SystemTime::now())
    }

    pub(crate) fn open_pairing_file_at(
        &self,
        bytes: &[u8],
        now: SystemTime,
    ) -> Result<PairingPreview, Error> {
        self.ensure_alive()?;
        pairing::inspect(bytes, now, self.local_endpoint_id())
    }

    /// Saves the device that wrote `preview` when the digits match that screen.
    ///
    /// A mismatch saves nothing and emits no event. A match accepts the signed
    /// invitation. Control of this computer is not granted here.
    pub fn confirm_pairing(
        &self,
        preview: &PairingPreview,
        check_from_other_screen: &str,
    ) -> Result<DeviceInfo, Error> {
        pairing::require_same_check(preview, check_from_other_screen)?;
        self.accept_invitation(&preview.token)
    }

    /// Three digits this computer shows while it is the controller.
    ///
    /// The host sees the same digits on [`CoreEvent::ControlRequested`].
    pub fn control_check(&self) -> String {
        pairing::control_digits(&self.local_endpoint_id())
    }

    /// The controller waiting for this host to grant control, if one is connected.
    pub fn control_request(&self) -> Result<Option<ControlRequest>, Error> {
        let state = self.alive_state()?;
        Ok(state
            .control_request
            .as_ref()
            .map(|pending| ControlRequest {
                endpoint_id: pending.display_id.clone(),
                check: pending.check.clone(),
            }))
    }

    /// Grants control to the waiting controller when the digits match that screen.
    ///
    /// Clipboard and file transfer stay off. A mismatch rejects the connection
    /// and saves nothing. The pairing file can still admit a later attempt
    /// until it expires.
    pub fn confirm_control_request(
        &self,
        check_from_other_screen: &str,
    ) -> Result<DeviceInfo, Error> {
        self.finish_control_request(check_from_other_screen)
    }

    /// Rejects the waiting controller. The pairing file stays open until it expires.
    pub fn reject_control_request(&self) -> Result<(), Error> {
        let mut state = self.alive_state()?;
        let pending = state
            .control_request
            .take()
            .ok_or(Error::NoControlRequest)?;
        drop(state);
        let _ = pending.decision.send(false);
        Ok(())
    }

    pub fn accept_invitation(&self, token: &str) -> Result<DeviceInfo, Error> {
        let (opened, nonce) = invitation::open(token, SystemTime::now())?;
        if opened.endpoint_id == *self.shared.secret.public().as_bytes() {
            return Err(Error::InvalidInvitation(
                "an installation cannot pair with itself",
            ));
        }
        let now_ms = SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|_| Error::InvalidInvitation("system clock is before the unix epoch"))?
            .as_millis() as u64;
        let display = PublicKey::from_bytes(&opened.endpoint_id)
            .map_err(|_| Error::InvalidInvitation("endpoint id is not a valid key"))?
            .to_string();
        let info = DeviceInfo {
            endpoint_id: display.clone(),
            relays: opened.relays.clone(),
            direct: opened.direct.clone(),
            grants: Grants::default(),
        };
        {
            let mut state = self.alive_state()?;
            let previous_device = state.trust.get(&opened.endpoint_id).cloned();
            let previous_replay = state.replay.clone();
            if let Err(error) = state.replay.remember(nonce, opened.expires_at_ms, now_ms) {
                state.replay = previous_replay;
                return Err(error);
            }
            if let Err(error) = state.trust.pair(Device {
                endpoint_id: opened.endpoint_id,
                display_id: display.clone(),
                relays: opened.relays,
                direct: opened.direct,
                grants: Grants::default(),
            }) {
                state.replay = previous_replay;
                return Err(error);
            }
            if let Err(error) = persist(&self.shared, &state) {
                state.replay = previous_replay;
                match previous_device {
                    Some(device) => {
                        let _ = state.trust.pair(device);
                    }
                    None => {
                        state.trust.remove(&opened.endpoint_id);
                    }
                }
                return Err(error);
            }
        }
        self.shared.events.publish(CoreEvent::DevicePaired {
            endpoint_id: display,
        });
        Ok(info)
    }

    pub fn authorize(&self, endpoint_id: &str, grants: Grants) -> Result<(), Error> {
        let key = parse_endpoint_id(endpoint_id)?;
        {
            let mut state = self.alive_state()?;
            let previous = state
                .trust
                .get(&key)
                .map(|device| device.grants)
                .ok_or(Error::NotPaired)?;
            state.trust.authorize(&key, grants)?;
            if let Err(error) = persist(&self.shared, &state) {
                let _ = state.trust.authorize(&key, previous);
                return Err(error);
            }
        }
        self.shared.events.publish(CoreEvent::DeviceAuthorized {
            endpoint_id: endpoint_id.to_owned(),
        });
        Ok(())
    }

    pub fn revoke(&self, endpoint_id: &str) -> Result<(), Error> {
        let key = parse_endpoint_id(endpoint_id)?;
        let mut stopped = None;
        {
            let mut state = self.alive_state()?;
            let previous = state
                .trust
                .get(&key)
                .map(|device| device.grants)
                .ok_or(Error::NotPaired)?;
            state.trust.revoke(&key)?;
            if let Err(error) = persist(&self.shared, &state) {
                let _ = state.trust.authorize(&key, previous);
                return Err(error);
            }
            if state
                .incoming
                .as_ref()
                .is_some_and(|session| session.peer_key == key && !finished(&session.phase))
            {
                state.generation = state.generation.saturating_add(1);
                if let Some(session) = state.incoming.as_mut() {
                    session.phase = SessionPhase::Stopping;
                    stopped = Some(session.stop.clone());
                }
            }
        }
        if let Some(stop) = stopped {
            let _ = stop.send(true);
        }
        self.shared.events.publish(CoreEvent::DeviceRevoked {
            endpoint_id: endpoint_id.to_owned(),
        });
        Ok(())
    }

    pub fn connect(&self, endpoint_id: &str) -> Result<SessionInfo, Error> {
        let key = parse_endpoint_id(endpoint_id)?;
        let device = {
            let state = self.alive_state()?;
            if state
                .outgoing
                .as_ref()
                .is_some_and(|session| !finished(&session.phase))
            {
                return Err(Error::Busy);
            }
            state.trust.get(&key).cloned().ok_or(Error::NotPaired)?
        };
        let (stop, stop_rx) = tokio::sync::watch::channel(false);
        let outbox = Arc::new(InputOutbox::new());
        {
            let mut state = self.alive_state()?;
            state.outgoing = Some(LiveSession {
                id: 0,
                generation: 0,
                role: SessionRole::Controller,
                phase: SessionPhase::Connecting,
                peer_id: device.display_id.clone(),
                peer_key: device.endpoint_id,
                quality: None,
                monitor_width: 0,
                monitor_height: 0,
                stop: stop.clone(),
                outbox: Some(Arc::clone(&outbox)),
                video: None,
                link: None,
                clipboard: None,
                transfer: None,
            });
        }
        self.emit_session(
            0,
            0,
            SessionRole::Controller,
            SessionPhase::Connecting,
            &device.display_id,
        );
        let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel(1);
        let shared = Arc::clone(&self.shared);
        let peer_key = device.endpoint_id;
        self.runtime.spawn(async move {
            run_controller(shared, peer_key, stop_rx, outbox, ready_tx).await;
        });
        match ready_rx.recv_timeout(CONNECT_WAIT) {
            Ok(result) => result,
            Err(_) => {
                let _ = stop.send(true);
                Err(Error::Timeout)
            }
        }
    }

    pub fn disconnect(&self) -> Result<(), Error> {
        let mut state = self.alive_state()?;
        let session = state.outgoing.as_mut().ok_or(Error::NoSession)?;
        if finished(&session.phase) {
            return Err(Error::NoSession);
        }
        session.phase = SessionPhase::Stopping;
        let _ = session.stop.send(true);
        Ok(())
    }

    /// Closes the live connection and leaves the stop signal unset.
    ///
    /// The controller treats that as a dropped link and dials again.
    #[cfg(test)]
    pub(crate) fn drop_link(&self) -> Result<(), Error> {
        let link = {
            let state = self.alive_state()?;
            state
                .outgoing
                .as_ref()
                .and_then(|session| session.link.clone())
                .or_else(|| {
                    state
                        .incoming
                        .as_ref()
                        .and_then(|session| session.link.clone())
                })
                .ok_or(Error::NoSession)?
        };
        link.close(1u8.into(), b"dropped");
        Ok(())
    }

    /// Queues one keyboard or pointer event for the active controlling session.
    ///
    /// Returns when the event is queued, not when the host has applied it.
    /// Consecutive pointer positions collapse to the latest one. A later call
    /// returns the host's rejection if input was refused.
    pub fn submit_input(
        &self,
        session_id: u64,
        generation: u64,
        event: InputEvent,
    ) -> Result<(), Error> {
        let outbox = {
            let state = self.alive_state()?;
            let session = state.outgoing.as_ref().ok_or(Error::NoSession)?;
            if session.phase != SessionPhase::Active || session.id != session_id {
                return Err(Error::NoSession);
            }
            if session.generation != generation {
                return Err(Error::StaleSession);
            }
            session.outbox.clone().ok_or(Error::NoSession)?
        };
        let event = if self.shared.swap_control_command.load(Ordering::Relaxed) {
            input::swap_command_key(event)
        } else {
            event
        };
        outbox.push(event)
    }

    /// Exchanges Control and Command on events this controller sends.
    ///
    /// Off by default. A Mac Command shortcut can be sent as Control for a
    /// Windows or Linux host, and the reverse. The host injects the physical
    /// key it receives. The choice belongs to this process and is not saved.
    /// Release held modifiers before changing it, so the key that goes down
    /// is the key that comes up.
    pub fn set_swap_control_and_command(&self, enabled: bool) -> Result<(), Error> {
        drop(self.alive_state()?);
        self.shared
            .swap_control_command
            .store(enabled, Ordering::Relaxed);
        Ok(())
    }

    /// Whether this controller exchanges Control and Command.
    pub fn swap_control_and_command(&self) -> Result<bool, Error> {
        drop(self.alive_state()?);
        Ok(self.shared.swap_control_command.load(Ordering::Relaxed))
    }

    /// Queues finished text for the controlled computer.
    ///
    /// This is not a clipboard paste and does not use the clipboard grant.
    /// The host types the characters, so the result does not depend on which
    /// physical key was pressed. The text is UTF-8, at most
    /// [`MAX_KEY_TEXT`](crate::MAX_KEY_TEXT) bytes, and is not logged.
    /// Returns when the text is queued, not when the host has typed it.
    /// A dropped session does not send it again.
    pub fn submit_text(&self, session_id: u64, generation: u64, text: &str) -> Result<(), Error> {
        input::check_key_text(text)?;
        let outbox = {
            let state = self.alive_state()?;
            let session = state.outgoing.as_ref().ok_or(Error::NoSession)?;
            if session.phase != SessionPhase::Active || session.id != session_id {
                return Err(Error::NoSession);
            }
            if session.generation != generation {
                return Err(Error::StaleSession);
            }
            session.outbox.clone().ok_or(Error::NoSession)?
        };
        outbox.push_text(text.to_owned())
    }

    /// Sends clipboard text to the controlled computer.
    ///
    /// Returns when the host has applied it or refused it. The clipboard grant
    /// is required. Control alone is not enough. Text longer than
    /// [`MAX_CLIPBOARD_TEXT`](crate::MAX_CLIPBOARD_TEXT) is rejected here.
    /// Empty text clears the host clipboard. This does not read the local
    /// operating-system clipboard.
    pub fn submit_clipboard(
        &self,
        session_id: u64,
        generation: u64,
        text: &str,
    ) -> Result<(), Error> {
        clipboard::check_text(text)?;
        let tx = {
            let state = self.alive_state()?;
            clipboard_sender(&state, false, session_id, Some(generation))?
        };
        clipboard_round_trip(tx, text)
    }

    /// Sends this host's clipboard text to the controller.
    ///
    /// The controller reads it with [`Core::clipboard_text`]. Nothing is sent
    /// when the clipboard grant is off. Empty text clears the controller's
    /// copy. This does not read the operating-system clipboard.
    pub fn publish_clipboard(&self, session_id: u64, text: &str) -> Result<(), Error> {
        clipboard::check_text(text)?;
        let tx = {
            let state = self.alive_state()?;
            let session = state.incoming.as_ref().ok_or(Error::NoSession)?;
            if session.phase != SessionPhase::Active || session.id != session_id {
                return Err(Error::NoSession);
            }
            if !state.trust.clipboard_granted(&session.peer_key) {
                return Err(Error::ClipboardDenied);
            }
            session
                .clipboard
                .clone()
                .ok_or(Error::Unsupported("clipboard stream is not open"))?
        };
        clipboard_round_trip(tx, text)
    }

    /// Sends one file to the controlled computer.
    ///
    /// Returns when the host has written it or refused it. The transfer grant
    /// is required. Control alone is not enough. A name that leaves the host's
    /// transfer directory, or a file larger than
    /// [`MAX_FILE_BYTES`](crate::MAX_FILE_BYTES), is rejected here. `replace`
    /// must be set to change an existing file. Nothing is logged.
    pub fn submit_file(
        &self,
        session_id: u64,
        generation: u64,
        name: &str,
        bytes: &[u8],
        replace: bool,
    ) -> Result<(), Error> {
        transfer::check_submission(name, bytes)?;
        let tx = {
            let state = self.alive_state()?;
            transfer_sender(&state, false, session_id, Some(generation))?
        };
        transfer_round_trip(tx, name, bytes, replace)
    }

    /// Sends one file from this host to the controller.
    ///
    /// The controller writes it under its own transfer directory. Nothing is
    /// sent when the transfer grant is off. `replace` must be set to change
    /// an existing file. Nothing is logged.
    pub fn publish_file(
        &self,
        session_id: u64,
        name: &str,
        bytes: &[u8],
        replace: bool,
    ) -> Result<(), Error> {
        transfer::check_submission(name, bytes)?;
        let tx = {
            let state = self.alive_state()?;
            let session = state.incoming.as_ref().ok_or(Error::NoSession)?;
            if session.phase != SessionPhase::Active || session.id != session_id {
                return Err(Error::NoSession);
            }
            if !state.trust.transfer_granted(&session.peer_key) {
                return Err(Error::TransferDenied);
            }
            session
                .transfer
                .clone()
                .ok_or(Error::Unsupported("transfer stream is not open"))?
        };
        transfer_round_trip(tx, name, bytes, replace)
    }

    /// Latest clipboard text published by the controlled computer.
    ///
    /// `None` before the first paste of this session. The text is replaced,
    /// not queued. Do not log the returned string.
    pub fn clipboard_text(&self) -> Result<Option<String>, Error> {
        self.ensure_alive()?;
        Ok(lock(&self.shared.clipboard_inbox).clone())
    }

    /// Queues one encoded frame for the active hosted session.
    ///
    /// A full queue drops predicted frames until the next keyframe and still returns
    /// `Ok`. The frame bytes are not logged.
    pub fn publish_video(&self, session_id: u64, frame: EncodedFrame) -> Result<(), Error> {
        self.ensure_alive()?;
        frame.check()?;
        let outbox = {
            let state = self.alive_state()?;
            let session = state.incoming.as_ref().ok_or(Error::NoSession)?;
            if session.id != session_id || session.phase != SessionPhase::Active {
                return Err(Error::NoSession);
            }
            session.video.clone().ok_or(Error::NoSession)?
        };
        if matches!(outbox.push(frame), VideoPush::KeyframeNeeded) {
            self.shared
                .events
                .publish(CoreEvent::KeyframeRequested { id: session_id });
        }
        Ok(())
    }

    pub fn update_session_quality(&self, session_id: u64, quality: Quality) -> Result<(), Error> {
        let mut state = self.alive_state()?;
        let on_incoming = state.incoming.as_ref().is_some_and(|session| {
            session.id == session_id && session.phase == SessionPhase::Active
        });
        let session = if on_incoming {
            state.incoming.as_mut()
        } else if state.outgoing.as_ref().is_some_and(|session| {
            session.id == session_id && session.phase == SessionPhase::Active
        }) {
            state.outgoing.as_mut()
        } else {
            None
        };
        let Some(session) = session else {
            return Err(Error::NoSession);
        };
        session.quality = Some(quality);
        drop(state);
        self.shared.events.publish(CoreEvent::QualityUpdated {
            id: session_id,
            quality,
        });
        Ok(())
    }

    fn stop_incoming(&self) -> Result<(), Error> {
        let mut state = self.alive_state()?;
        let stop = {
            let session = state.incoming.as_mut().ok_or(Error::NoSession)?;
            if finished(&session.phase) {
                return Err(Error::NoSession);
            }
            session.phase = SessionPhase::Stopping;
            session.stop.clone()
        };
        state.generation = state.generation.saturating_add(1);
        drop(state);
        let _ = stop.send(true);
        Ok(())
    }

    fn local_endpoint_id(&self) -> [u8; 32] {
        *self.shared.secret.public().as_bytes()
    }

    fn finish_control_request(&self, check: &str) -> Result<DeviceInfo, Error> {
        let mut state = self.alive_state()?;
        let pending = state
            .control_request
            .take()
            .ok_or(Error::NoControlRequest)?;
        if check != pending.check {
            let _ = pending.decision.send(false);
            return Err(Error::PairingCheckMismatch);
        }
        let grants = Grants {
            control: true,
            clipboard: false,
            transfer: false,
        };
        let info = DeviceInfo {
            endpoint_id: pending.display_id.clone(),
            relays: pending.relays.clone(),
            direct: pending.direct.clone(),
            grants,
        };
        let previous = state.trust.get(&pending.endpoint_id).cloned();
        let previous_window = state.pairing_window.take();
        if let Err(error) = state.trust.pair(Device {
            endpoint_id: pending.endpoint_id,
            display_id: pending.display_id.clone(),
            relays: pending.relays.clone(),
            direct: pending.direct.clone(),
            grants,
        }) {
            state.pairing_window = previous_window;
            let _ = pending.decision.send(false);
            return Err(error);
        }
        if let Err(error) = persist(&self.shared, &state) {
            match previous {
                Some(device) => {
                    let _ = state.trust.pair(device);
                }
                None => {
                    state.trust.remove(&pending.endpoint_id);
                }
            }
            state.pairing_window = previous_window;
            let _ = pending.decision.send(false);
            return Err(error);
        }
        drop(state);
        let _ = pending.decision.send(true);
        self.shared.events.publish(CoreEvent::DevicePaired {
            endpoint_id: info.endpoint_id.clone(),
        });
        self.shared.events.publish(CoreEvent::DeviceAuthorized {
            endpoint_id: info.endpoint_id.clone(),
        });
        Ok(info)
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

async fn bind_endpoint(
    secret: SecretKey,
    relay_mode: RelayMode,
    listen: Listen,
    clear_lookup: bool,
) -> Result<Endpoint, Error> {
    let mut builder = Endpoint::builder(presets::N0)
        .secret_key(secret)
        .alpns(vec![ALPN.to_vec()])
        .relay_mode(relay_mode);
    if clear_lookup {
        builder = builder.clear_address_lookup();
    }
    if listen == Listen::Loopback {
        builder = builder
            .clear_ip_transports()
            .bind_addr("127.0.0.1:0")
            .map_err(Error::network_debug)?;
    }
    builder.bind().await.map_err(Error::network_debug)
}

async fn accept_loop(shared: Arc<Shared>) {
    loop {
        if lock(&shared.state).shut_down {
            break;
        }
        let incoming = shared.endpoint.accept().await;
        let Some(incoming) = incoming else {
            break;
        };
        let shared = Arc::clone(&shared);
        tokio::spawn(async move {
            match incoming.await {
                Ok(connection) => host_connection(shared, connection).await,
                Err(error) => {
                    let _ = error;
                }
            }
        });
    }
}

async fn host_connection(shared: Arc<Shared>, connection: iroh::endpoint::Connection) {
    let peer = connection.remote_id();
    let (stop, mut stop_rx) = tokio::sync::watch::channel(false);
    let mut owned_session = None;
    loop {
        if *stop_rx.borrow() || lock(&shared.state).shut_down {
            break;
        }
        tokio::select! {
            changed = stop_rx.changed() => {
                if changed.is_err() || *stop_rx.borrow() {
                    break;
                }
            }
            incoming = connection.accept_bi() => {
                let Ok((send, recv)) = incoming else {
                    break;
                };
                match handle_host_request(
                    &shared,
                    &connection,
                    peer,
                    send,
                    recv,
                    &stop,
                    &mut stop_rx,
                    &mut owned_session,
                )
                .await
                {
                    HostTurn::PeerShouldClose | HostTurn::LocalStop => {
                        // The controller reads the reject or close frame, then drops
                        // the connection. Closing first discards that frame.
                        let _ =
                            tokio::time::timeout(Duration::from_secs(2), connection.closed())
                                .await;
                    }
                    HostTurn::Done => {}
                }
                break;
            }
            _ = connection.closed() => break,
        }
    }
    if let Some(session_id) = owned_session {
        finish_incoming(&shared, session_id);
    }
    connection.close(1u8.into(), b"closed");
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum HostTurn {
    /// A reject or close frame was written. Wait until the controller reads it.
    PeerShouldClose,
    /// The host ended the session and wrote a close frame.
    LocalStop,
    /// The controlling session has ended and held keys were released.
    Done,
}

const CONNECT_WAIT: Duration = Duration::from_secs(75);
const CONTROL_APPROVAL_WAIT: Duration = Duration::from_secs(60);

enum ControlAnswer {
    Approved,
    Rejected(Error),
}

/// Parks an ungranted Hello while the host types the controller's digits.
///
/// Returns [`ControlAnswer::Rejected`] with the original unauthorized error
/// when this host did not export a file with the allow switch on. That path
/// does not wait.
async fn wait_for_control_approval(
    shared: &Shared,
    connection: &iroh::endpoint::Connection,
    peer: EndpointId,
    stop_rx: &mut tokio::sync::watch::Receiver<bool>,
) -> ControlAnswer {
    let (relays, direct) = peer_addresses(&shared.endpoint, peer).await;
    let (id, receiver) = {
        let mut state = lock(&shared.state);
        if state.shut_down {
            return ControlAnswer::Rejected(Error::ShutDown);
        }
        if state.control_request.is_some() {
            return ControlAnswer::Rejected(Error::Busy);
        }
        let now_ms = match invitation::unix_ms(SystemTime::now()) {
            Ok(now_ms) => now_ms,
            Err(error) => return ControlAnswer::Rejected(error),
        };
        let window_open = state
            .pairing_window
            .as_ref()
            .is_some_and(|window| window.allow_control && now_ms < window.expires_at_ms);
        if !window_open {
            if state
                .pairing_window
                .as_ref()
                .is_some_and(|window| now_ms >= window.expires_at_ms)
            {
                state.pairing_window = None;
            }
            return ControlAnswer::Rejected(Error::Unauthorized);
        }
        let id = state.next_control_request;
        state.next_control_request = state.next_control_request.saturating_add(1);
        let check = pairing::control_digits(peer.as_bytes());
        let (sender, receiver) = tokio::sync::oneshot::channel();
        state.control_request = Some(PendingControl {
            id,
            endpoint_id: *peer.as_bytes(),
            display_id: peer.to_string(),
            check: check.clone(),
            relays,
            direct,
            decision: sender,
        });
        shared.events.publish(CoreEvent::ControlRequested {
            endpoint_id: peer.to_string(),
            check,
        });
        (id, receiver)
    };
    let approved = tokio::select! {
        decision = receiver => decision.unwrap_or(false),
        _ = tokio::time::sleep(CONTROL_APPROVAL_WAIT) => false,
        _ = stop_rx.changed() => false,
        _ = connection.closed() => false,
    };
    if approved {
        return ControlAnswer::Approved;
    }
    let mut state = lock(&shared.state);
    if state
        .control_request
        .as_ref()
        .is_some_and(|pending| pending.id == id)
    {
        state.control_request = None;
    }
    ControlAnswer::Rejected(Error::Unauthorized)
}

async fn peer_addresses(endpoint: &Endpoint, peer: EndpointId) -> (Vec<String>, Vec<SocketAddr>) {
    let Some(info) = endpoint.remote_info(peer).await else {
        return (Vec::new(), Vec::new());
    };
    let mut relays = Vec::new();
    let mut direct = Vec::new();
    for addr in info.addrs() {
        match addr.addr() {
            TransportAddr::Relay(url) => {
                if relays.len() >= crate::relay::MAX_CUSTOM_RELAYS {
                    continue;
                }
                let Ok(parsed) = crate::relay::parse_relay_url(url.as_str()) else {
                    continue;
                };
                let text = parsed.to_string();
                if !relays.contains(&text) {
                    relays.push(text);
                }
            }
            TransportAddr::Ip(socket) => direct.push(*socket),
            _ => {}
        }
    }
    let direct = invitation::canonicalize_addrs(&direct).unwrap_or_default();
    (relays, direct)
}

async fn handle_host_request(
    shared: &Arc<Shared>,
    connection: &iroh::endpoint::Connection,
    peer: EndpointId,
    mut send: iroh::endpoint::SendStream,
    mut recv: iroh::endpoint::RecvStream,
    stop: &tokio::sync::watch::Sender<bool>,
    stop_rx: &mut tokio::sync::watch::Receiver<bool>,
    owned_session: &mut Option<u64>,
) -> HostTurn {
    let message = match read_frame(&mut recv).await {
        Ok(message) => message,
        Err(_) => {
            let _ = write_frame(
                &mut send,
                &ControlMessage::Reject(error_code(&Error::Malformed)),
            )
            .await;
            return HostTurn::PeerShouldClose;
        }
    };
    match message {
        ControlMessage::Hello { version } => {
            let decision = {
                let state = lock(&shared.state);
                session::admit(
                    state.hosting,
                    state
                        .incoming
                        .as_ref()
                        .is_some_and(|session| !finished(&session.phase)),
                    state.trust.control_granted(peer.as_bytes()),
                    version,
                )
            };
            let decision = match decision {
                // The file already told the controller who this host is. The
                // digits typed here tell the host who the controller is.
                // Without an open allow switch the reject stays immediate.
                Err(Error::Unauthorized) => {
                    match wait_for_control_approval(shared, connection, peer, stop_rx).await {
                        ControlAnswer::Approved => {
                            let state = lock(&shared.state);
                            session::admit(
                                state.hosting,
                                state
                                    .incoming
                                    .as_ref()
                                    .is_some_and(|session| !finished(&session.phase)),
                                state.trust.control_granted(peer.as_bytes()),
                                version,
                            )
                        }
                        ControlAnswer::Rejected(error) => Err(error),
                    }
                }
                other => other,
            };
            match decision {
                Err(error) => {
                    let _ =
                        write_frame(&mut send, &ControlMessage::Reject(error_code(&error))).await;
                    HostTurn::PeerShouldClose
                }
                Ok(()) => {
                    // The previous session, if this peer just dropped, releases
                    // held keys before this Hello is admitted.
                    let _gate = shared.session_gate.lock().await;
                    let decision = {
                        let state = lock(&shared.state);
                        session::admit(
                            state.hosting,
                            state
                                .incoming
                                .as_ref()
                                .is_some_and(|session| !finished(&session.phase)),
                            state.trust.control_granted(peer.as_bytes()),
                            version,
                        )
                    };
                    if let Err(error) = decision {
                        let _ = write_frame(&mut send, &ControlMessage::Reject(error_code(&error)))
                            .await;
                        return HostTurn::PeerShouldClose;
                    }
                    match register_incoming(shared, peer, stop) {
                        Ok(info) => {
                            *owned_session = Some(info.id);
                            remember_link(shared, true, connection.clone());
                            let wrote = write_frame(
                                &mut send,
                                &ControlMessage::Accept {
                                    session_id: info.id,
                                    generation: info.generation,
                                    monitor_width: info.monitor_width,
                                    monitor_height: info.monitor_height,
                                },
                            )
                            .await;
                            if wrote.is_err() || write_addresses(shared, &mut send).await.is_err() {
                                finish_incoming(shared, info.id);
                                return HostTurn::PeerShouldClose;
                            }
                            let outbox = {
                                let state = lock(&shared.state);
                                state
                                    .incoming
                                    .as_ref()
                                    .and_then(|session| session.video.clone())
                            };
                            let video = outbox.map(|outbox| {
                                let connection = connection.clone();
                                let mut stop_rx = stop_rx.clone();
                                tokio::spawn(async move {
                                    host_video_loop(connection, outbox, &mut stop_rx).await;
                                })
                            });
                            let (clipboard, transfer) = spawn_host_streams(
                                shared,
                                iroh::endpoint::Connection::clone(connection),
                                info.id,
                                *peer.as_bytes(),
                                tokio::sync::watch::Receiver::clone(stop_rx),
                            );
                            let local_stop = host_input_loop(
                                shared, info.id, peer, &mut send, &mut recv, stop_rx,
                            )
                            .await;
                            transfer.abort();
                            clipboard.abort();
                            if let Some(video) = video {
                                video.abort();
                            }
                            if local_stop {
                                let _ = write_frame(&mut send, &ControlMessage::Close).await;
                                let _ = send.finish();
                            }
                            finish_incoming(shared, info.id);
                            if local_stop {
                                HostTurn::LocalStop
                            } else {
                                HostTurn::Done
                            }
                        }
                        Err(error) => {
                            let _ =
                                write_frame(&mut send, &ControlMessage::Reject(error_code(&error)))
                                    .await;
                            HostTurn::PeerShouldClose
                        }
                    }
                }
            }
        }
        _ => {
            let _ = write_frame(
                &mut send,
                &ControlMessage::Reject(error_code(&Error::Malformed)),
            )
            .await;
            HostTurn::PeerShouldClose
        }
    }
}

fn check_live_input(shared: &Shared, session_id: u64, generation: u64) -> Result<(), Error> {
    let state = lock(&shared.state);
    let active = state.incoming.as_ref().and_then(|session| {
        (session.phase == SessionPhase::Active).then_some((session.id, session.generation))
    });
    session::check_input(active, session_id, generation)
}

async fn host_input_loop(
    shared: &Shared,
    session_id: u64,
    peer: EndpointId,
    send: &mut iroh::endpoint::SendStream,
    recv: &mut iroh::endpoint::RecvStream,
    stop_rx: &mut tokio::sync::watch::Receiver<bool>,
) -> bool {
    let mut held = HeldInput::default();
    let mut input_open = true;
    let local_stop = loop {
        if *stop_rx.borrow() || lock(&shared.state).shut_down {
            break true;
        }
        tokio::select! {
            changed = stop_rx.changed() => {
                if changed.is_err() || *stop_rx.borrow() {
                    break true;
                }
            }
            frame = read_session_frame(recv) => {
                let message = match frame {
                    Ok(message) => message,
                    Err(Error::Malformed) => {
                        let _ = write_frame(send, &ControlMessage::Reject(error_code(&Error::Malformed))).await;
                        break false;
                    }
                    Err(_) => break false,
                };
                match message {
                    ControlMessage::Addresses { relays, direct } => {
                        let _ = remember_peer_addresses(
                            shared,
                            peer.as_bytes(),
                            Announcement { relays, direct },
                        );
                    }
                    ControlMessage::Close => break false,
                    ControlMessage::Keyframe {
                        session_id: event_session,
                        generation,
                    } => {
                        if let Err(error) = check_live_input(shared, event_session, generation) {
                            let _ = write_frame(send, &ControlMessage::Reject(error_code(&error))).await;
                            break false;
                        }
                        shared
                            .events
                            .publish(CoreEvent::KeyframeRequested { id: session_id });
                    }
                    ControlMessage::Input {
                        session_id: event_session,
                        generation,
                        event,
                    } => {
                        if let Err(error) = check_live_input(shared, event_session, generation) {
                            let _ = write_frame(send, &ControlMessage::Reject(error_code(&error))).await;
                            break false;
                        }
                        if !input_open {
                            continue;
                        }
                        let applied = {
                            let mut desktop = lock(&shared.desktop);
                            match desktop.as_mut() {
                                None => Err(Error::Unsupported("no desktop adapter is installed")),
                                Some(desktop) => desktop.apply(event),
                            }
                        };
                        match applied {
                            Ok(()) if input_open => held.observe(event),
                            Ok(()) => {}
                            Err(error) => {
                                note_input_failure(shared, session_id, &error);
                                if input_open && matches!(error, Error::Unsupported(_)) {
                                    input_open = false;
                                    let _ = write_frame(send, &ControlMessage::Reject(error_code(&error))).await;
                                }
                            }
                        }
                    }
                    ControlMessage::Text {
                        session_id: event_session,
                        generation,
                        text,
                    } => {
                        if let Err(error) = check_live_input(shared, event_session, generation) {
                            let _ = write_frame(send, &ControlMessage::Reject(error_code(&error))).await;
                            break false;
                        }
                        if !input_open {
                            continue;
                        }
                        if let Err(error) = apply_key_text(shared, &text.0) {
                            note_input_failure(shared, session_id, &error);
                            if input_open && matches!(error, Error::Unsupported(_)) {
                                input_open = false;
                                let _ = write_frame(send, &ControlMessage::Reject(error_code(&error))).await;
                            }
                        }
                    }
                    _ => {
                        let _ = write_frame(send, &ControlMessage::Reject(error_code(&Error::Malformed))).await;
                        break false;
                    }
                }
            }
        }
    };
    release_held(shared, session_id, &held);
    local_stop
}

fn note_input_failure(shared: &Shared, session_id: u64, error: &Error) {
    shared.events.publish(CoreEvent::InputFailed {
        id: session_id,
        message: input::InputFault::from_error(error).message(),
    });
}

fn apply_key_text(shared: &Shared, text: &str) -> Result<(), Error> {
    if text.is_empty() {
        return Ok(());
    }
    if let Some(message) = input::key_text_problem(text) {
        return Err(Error::InputFailed(message));
    }
    let mut desktop = lock(&shared.desktop);
    match desktop.as_mut() {
        None => Err(Error::Unsupported("no desktop adapter is installed")),
        Some(desktop) => desktop.type_text(text),
    }
}

fn release_held(shared: &Shared, session_id: u64, held: &HeldInput) {
    let mut desktop = lock(&shared.desktop);
    let Some(desktop) = desktop.as_mut() else {
        return;
    };
    for event in held.release_events() {
        if desktop.apply(event).is_ok() {
            continue;
        }
        shared.events.publish(CoreEvent::InputFailed {
            id: session_id,
            message: "a held key or button could not be released",
        });
    }
}

fn register_incoming(
    shared: &Shared,
    peer: EndpointId,
    stop: &tokio::sync::watch::Sender<bool>,
) -> Result<SessionInfo, Error> {
    let (monitor_width, monitor_height) = monitor_size(shared);
    let mut state = lock(&shared.state);
    if state.shut_down {
        return Err(Error::ShutDown);
    }
    if state
        .incoming
        .as_ref()
        .is_some_and(|session| !finished(&session.phase))
    {
        return Err(Error::Busy);
    }
    state.generation = state.generation.saturating_add(1);
    let id = state.next_session_id;
    state.next_session_id = state.next_session_id.saturating_add(1);
    let generation = state.generation;
    let peer_id = peer.to_string();
    let info = SessionInfo {
        id,
        generation,
        role: SessionRole::Host,
        phase: session::phase_after_admit(),
        peer_id: peer_id.clone(),
        quality: None,
        monitor_width,
        monitor_height,
    };
    state.incoming = Some(LiveSession {
        id,
        generation,
        role: SessionRole::Host,
        phase: session::phase_after_admit(),
        peer_id: peer_id.clone(),
        peer_key: *peer.as_bytes(),
        quality: None,
        monitor_width,
        monitor_height,
        stop: stop.clone(),
        outbox: None,
        video: Some(Arc::new(VideoOutbox::new())),
        link: None,
        clipboard: None,
        transfer: None,
    });
    drop(state);
    for phase in [
        SessionPhase::Authenticated,
        SessionPhase::Authorized,
        SessionPhase::Negotiating,
        SessionPhase::Active,
    ] {
        shared.events.publish(CoreEvent::SessionChanged {
            id,
            generation,
            role: SessionRole::Host,
            phase,
            peer_id: peer_id.clone(),
        });
    }
    Ok(info)
}

fn finish_incoming(shared: &Shared, session_id: u64) {
    let mut state = lock(&shared.state);
    let active = match state.incoming.as_ref() {
        Some(session) if session.id == session_id && !finished(&session.phase) => {
            session.phase == SessionPhase::Active
        }
        _ => return,
    };
    if active {
        state.generation = state.generation.saturating_add(1);
    }
    let session = state.incoming.as_mut().expect("checked");
    let info = session_info(session);
    session.phase = SessionPhase::Closed;
    drop(state);
    shared.events.publish(CoreEvent::SessionChanged {
        id: info.id,
        generation: info.generation,
        role: SessionRole::Host,
        phase: SessionPhase::Closed,
        peer_id: info.peer_id,
    });
}

async fn run_controller(
    shared: Arc<Shared>,
    peer_key: [u8; 32],
    mut stop_rx: tokio::sync::watch::Receiver<bool>,
    outbox: Arc<InputOutbox>,
    ready_tx: std::sync::mpsc::SyncSender<Result<SessionInfo, Error>>,
) {
    let mut ready = Some(ready_tx);
    let mut drops = 0u32;
    loop {
        if *stop_rx.borrow() || lock(&shared.state).shut_down {
            finish_outgoing(&shared);
            if let Some(ready) = ready.take() {
                let _ = ready.send(Err(Error::NoSession));
            }
            return;
        }
        if drops > 0 {
            mark_reconnecting(&shared);
            outbox.clear();
            tokio::select! {
                _ = tokio::time::sleep(reconnect::PAUSE) => {}
                _ = stop_rx.changed() => {}
            }
            if *stop_rx.borrow() || lock(&shared.state).shut_down {
                finish_outgoing(&shared);
                return;
            }
        }
        let device = match load_device(&shared, &peer_key) {
            Ok(device) => device,
            Err(error) => {
                fail_outgoing(&shared, &error);
                if let Some(ready) = ready.take() {
                    let _ = ready.send(Err(error));
                }
                return;
            }
        };
        match establish(&shared, device, &mut stop_rx, &outbox, &mut ready).await {
            Ok((lasted, LinkEnd::Dropped)) => {
                if lasted {
                    drops = 1;
                } else {
                    drops += 1;
                }
                if drops > reconnect::ATTEMPTS {
                    fail_outgoing(&shared, &Error::Timeout);
                    return;
                }
            }
            Ok((_, LinkEnd::LocalStop | LinkEnd::RemoteClose)) => {
                finish_outgoing(&shared);
                return;
            }
            Err(error) if ready.is_none() && reconnect::retries(&error) => {
                drops += 1;
                if drops > reconnect::ATTEMPTS {
                    fail_outgoing(&shared, &error);
                    return;
                }
            }
            Err(error) => {
                fail_outgoing(&shared, &error);
                if let Some(ready) = ready.take() {
                    let _ = ready.send(Err(error));
                }
                return;
            }
        }
    }
}

async fn establish(
    shared: &Arc<Shared>,
    device: Device,
    stop_rx: &mut tokio::sync::watch::Receiver<bool>,
    outbox: &Arc<InputOutbox>,
    ready: &mut Option<std::sync::mpsc::SyncSender<Result<SessionInfo, Error>>>,
) -> Result<(bool, LinkEnd), Error> {
    let addr = device_addr(&device)?;
    let connect = shared.endpoint.connect(addr, ALPN);
    let connection = tokio::time::timeout(Duration::from_secs(10), connect)
        .await
        .map_err(|_| Error::Timeout)?
        .map_err(Error::network_debug)?;
    if *stop_rx.borrow() {
        return Err(Error::NoSession);
    }
    let (mut send, mut recv) = connection.open_bi().await.map_err(Error::network_debug)?;
    write_frame(
        &mut send,
        &ControlMessage::Hello {
            version: PROTOCOL_VERSION,
        },
    )
    .await?;
    let response = read_frame(&mut recv).await?;
    let info = match response {
        ControlMessage::Accept {
            session_id,
            generation,
            monitor_width,
            monitor_height,
        } => {
            let peer_id = device.display_id.clone();
            {
                let mut state = lock(&shared.state);
                if let Some(session) = state.outgoing.as_mut() {
                    session.id = session_id;
                    session.generation = generation;
                    session.phase = SessionPhase::Active;
                    session.monitor_width = monitor_width;
                    session.monitor_height = monitor_height;
                    session.link = Some(connection.clone());
                }
            }
            for phase in [
                SessionPhase::Authenticated,
                SessionPhase::Authorized,
                SessionPhase::Negotiating,
                SessionPhase::Active,
            ] {
                shared.events.publish(CoreEvent::SessionChanged {
                    id: session_id,
                    generation,
                    role: SessionRole::Controller,
                    phase,
                    peer_id: peer_id.clone(),
                });
            }
            SessionInfo {
                id: session_id,
                generation,
                role: SessionRole::Controller,
                phase: SessionPhase::Active,
                peer_id,
                quality: None,
                monitor_width,
                monitor_height,
            }
        }
        ControlMessage::Reject(code) => return Err(code.to_error()),
        _ => return Err(Error::Malformed),
    };
    if write_addresses(shared, &mut send).await.is_err() {
        return Err(Error::network("the address announcement was not written"));
    }
    let clipboard = match open_controller_clipboard(shared, &connection, info.id, stop_rx).await {
        Ok(clipboard) => clipboard,
        Err(error) => return Err(error),
    };
    let transfer = match open_controller_transfer(shared, &connection, info.id, stop_rx).await {
        Ok(transfer) => transfer,
        Err(error) => {
            clipboard.abort();
            return Err(error);
        }
    };
    if let Some(ready) = ready.take() {
        let _ = ready.send(Ok(info.clone()));
    }
    let inbox = video_inbox(shared);
    inbox.reset();
    let keyframe = Arc::new(KeyframeFlag::new());
    let session_id = info.id;
    let generation = info.generation;
    let mut video_stop = stop_rx.clone();
    let video_connection = connection.clone();
    let video_inbox = Arc::clone(&inbox);
    let video_keyframe = Arc::clone(&keyframe);
    let reader = tokio::spawn(async move {
        video_recv_loop(
            video_connection,
            video_inbox,
            video_keyframe,
            &mut video_stop,
        )
        .await;
    });
    let started = std::time::Instant::now();
    let end = command_loop(
        connection,
        send,
        recv,
        Arc::clone(outbox),
        session_id,
        generation,
        device.endpoint_id,
        keyframe,
        shared,
        stop_rx,
    )
    .await;
    reader.abort();
    transfer.abort();
    clipboard.abort();
    Ok((started.elapsed() > reconnect::PAUSE, end))
}

async fn command_loop(
    connection: iroh::endpoint::Connection,
    mut send: iroh::endpoint::SendStream,
    mut recv: iroh::endpoint::RecvStream,
    outbox: Arc<InputOutbox>,
    session_id: u64,
    generation: u64,
    peer_key: [u8; 32],
    keyframe: Arc<KeyframeFlag>,
    shared: &Shared,
    stop_rx: &mut tokio::sync::watch::Receiver<bool>,
) -> LinkEnd {
    let (frames_tx, mut frames_rx) = tokio::sync::mpsc::channel(8);
    tokio::spawn(async move {
        loop {
            match read_session_frame(&mut recv).await {
                Ok(message) => {
                    if frames_tx.send(Ok(message)).await.is_err() {
                        break;
                    }
                }
                Err(error) => {
                    let _ = frames_tx.send(Err(error)).await;
                    break;
                }
            }
        }
    });
    let mut remote_close = false;
    loop {
        if *stop_rx.borrow() {
            break;
        }
        if keyframe.take() {
            let wrote = write_frame(
                &mut send,
                &ControlMessage::Keyframe {
                    session_id,
                    generation,
                },
            )
            .await;
            if wrote.is_err() {
                break;
            }
        }
        let notified = outbox.notified();
        tokio::pin!(notified);
        let batch = outbox.drain();
        if batch.is_empty() {
            let key_ready = keyframe.notified();
            tokio::pin!(key_ready);
            tokio::select! {
                _ = &mut notified => {}
                _ = &mut key_ready => {}
                changed = stop_rx.changed() => {
                    if changed.is_err() || *stop_rx.borrow() {
                        break;
                    }
                }
                frame = frames_rx.recv() => {
                    match frame {
                        Some(Ok(ControlMessage::Addresses { relays, direct })) => {
                            let _ = remember_peer_addresses(
                                shared,
                                &peer_key,
                                Announcement { relays, direct },
                            );
                        }
                        Some(Ok(ControlMessage::Close)) => {
                            remote_close = true;
                            break;
                        }
                        Some(Ok(ControlMessage::Reject(code))) => {
                            let error = code.to_error();
                            outbox.fail(&error);
                            shared.events.publish(CoreEvent::InputFailed {
                                id: session_id,
                                message: crate::input::InputFault::from_error(&error).message(),
                            });
                            if !matches!(error, Error::Unsupported(_)) {
                                break;
                            }
                        }
                        Some(Ok(_)) | Some(Err(_)) | None => break,
                    }
                }
                _ = connection.closed() => break,
            }
        } else {
            let mut failed = false;
            for item in batch {
                let message = match item {
                    Outbound::Event(event) => ControlMessage::Input {
                        session_id,
                        generation,
                        event,
                    },
                    Outbound::Text(text) => ControlMessage::Text {
                        session_id,
                        generation,
                        text: KeyText(text),
                    },
                };
                let wrote = write_frame(&mut send, &message).await;
                if wrote.is_err() {
                    failed = true;
                    break;
                }
            }
            if failed {
                break;
            }
        }
    }
    connection.close(1u8.into(), b"closed");
    if *stop_rx.borrow() {
        LinkEnd::LocalStop
    } else if remote_close {
        LinkEnd::RemoteClose
    } else {
        LinkEnd::Dropped
    }
}

fn finish_outgoing(shared: &Shared) {
    let mut state = lock(&shared.state);
    let Some(session) = state.outgoing.as_mut() else {
        return;
    };
    if finished(&session.phase) {
        return;
    }
    let info = session_info(session);
    session.phase = SessionPhase::Closed;
    drop(state);
    *lock(&shared.clipboard_inbox) = None;
    shared.events.publish(CoreEvent::SessionChanged {
        id: info.id,
        generation: info.generation,
        role: SessionRole::Controller,
        phase: SessionPhase::Closed,
        peer_id: info.peer_id,
    });
}

fn fail_outgoing(shared: &Shared, error: &Error) {
    let mut state = lock(&shared.state);
    let Some(session) = state.outgoing.as_mut() else {
        return;
    };
    let info = session_info(session);
    session.phase = SessionPhase::Failed(error.to_string());
    drop(state);
    *lock(&shared.clipboard_inbox) = None;
    shared.events.publish(CoreEvent::SessionChanged {
        id: info.id,
        generation: info.generation,
        role: SessionRole::Controller,
        phase: SessionPhase::Failed(error.to_string()),
        peer_id: info.peer_id,
    });
}

async fn write_frame(
    send: &mut iroh::endpoint::SendStream,
    message: &ControlMessage,
) -> Result<(), Error> {
    let limit = match message {
        ControlMessage::Addresses { .. } => MAX_ADDRESS_FRAME,
        _ => MAX_CONTROL_FRAME,
    };
    let payload = message.encode();
    if payload.is_empty() || payload.len() > limit {
        return Err(Error::Malformed);
    }
    let len = u16::try_from(payload.len()).map_err(|_| Error::Malformed)?;
    send.write_all(&len.to_be_bytes())
        .await
        .map_err(Error::network_debug)?;
    send.write_all(&payload)
        .await
        .map_err(Error::network_debug)?;
    Ok(())
}

async fn read_frame(recv: &mut iroh::endpoint::RecvStream) -> Result<ControlMessage, Error> {
    read_frame_within(recv, Some(Duration::from_secs(10))).await
}

/// Reads a frame on a session that is already up. Silence is not a timeout.
async fn read_session_frame(
    recv: &mut iroh::endpoint::RecvStream,
) -> Result<ControlMessage, Error> {
    read_frame_within(recv, None).await
}

async fn read_frame_within(
    recv: &mut iroh::endpoint::RecvStream,
    timeout: Option<Duration>,
) -> Result<ControlMessage, Error> {
    let mut len_buf = [0u8; 2];
    let length = recv.read_exact(&mut len_buf);
    let read = match timeout {
        Some(timeout) => tokio::time::timeout(timeout, length)
            .await
            .map_err(|_| Error::Timeout)?
            .map_err(Error::network_debug)?,
        None => length.await.map_err(Error::network_debug)?,
    };
    let _ = read;
    let len = usize::from(u16::from_be_bytes(len_buf));
    if len == 0 || len > MAX_ADDRESS_FRAME {
        return Err(Error::Malformed);
    }
    let mut payload = vec![0u8; len];
    tokio::time::timeout(Duration::from_secs(10), recv.read_exact(&mut payload))
        .await
        .map_err(|_| Error::Timeout)?
        .map_err(Error::network_debug)?;
    ControlMessage::decode(&payload)
}

fn load_device(shared: &Shared, peer_key: &[u8; 32]) -> Result<Device, Error> {
    lock(&shared.state)
        .trust
        .get(peer_key)
        .cloned()
        .ok_or(Error::NotPaired)
}

fn mark_reconnecting(shared: &Shared) {
    let mut state = lock(&shared.state);
    let Some(session) = state.outgoing.as_mut() else {
        return;
    };
    if finished(&session.phase) || matches!(session.phase, SessionPhase::Stopping) {
        return;
    }
    session.phase = SessionPhase::Reconnecting;
    let info = session_info(session);
    drop(state);
    *lock(&shared.clipboard_inbox) = None;
    shared.events.publish(CoreEvent::SessionChanged {
        id: info.id,
        generation: info.generation,
        role: info.role,
        phase: SessionPhase::Reconnecting,
        peer_id: info.peer_id,
    });
}

fn remember_link(shared: &Shared, incoming: bool, link: iroh::endpoint::Connection) {
    let mut state = lock(&shared.state);
    let session = if incoming {
        state.incoming.as_mut()
    } else {
        state.outgoing.as_mut()
    };
    if let Some(session) = session {
        session.link = Some(link);
    }
}

fn remember_peer_addresses(
    shared: &Shared,
    peer_key: &[u8; 32],
    announcement: Announcement,
) -> Result<(), Error> {
    let mut state = lock(&shared.state);
    let Some(mut device) = state.trust.get(peer_key).cloned() else {
        return Ok(());
    };
    if crate::address::unchanged(&device, &announcement) {
        return Ok(());
    }
    let previous = device.clone();
    crate::address::install(&mut device, &announcement)?;
    let endpoint_id = device.display_id.clone();
    state.trust.pair(device)?;
    if let Err(error) = persist(shared, &state) {
        let _ = state.trust.pair(previous);
        return Err(error);
    }
    drop(state);
    shared
        .events
        .publish(CoreEvent::AddressesUpdated { endpoint_id });
    Ok(())
}

async fn write_addresses(
    shared: &Shared,
    send: &mut iroh::endpoint::SendStream,
) -> Result<(), Error> {
    let relays = shared.relay.relay_urls()?;
    let direct = invitation::canonicalize_addrs(&direct_addresses(&shared.endpoint))?;
    let announcement = crate::address::canonical(&relays, &direct)?;
    write_frame(
        send,
        &ControlMessage::Addresses {
            relays: announcement.relays,
            direct: announcement.direct,
        },
    )
    .await
}

/// Clipboard is accepted before transfer. Two `accept_bi` calls at once can
/// take each other's stream, so the transfer accept starts only after the
/// clipboard accept has returned.
fn spawn_host_streams(
    shared: &Arc<Shared>,
    connection: iroh::endpoint::Connection,
    session_id: u64,
    peer: [u8; 32],
    stop_rx: tokio::sync::watch::Receiver<bool>,
) -> (tokio::task::JoinHandle<()>, tokio::task::JoinHandle<()>) {
    let (clipboard_tx, clipboard_rx) = tokio::sync::mpsc::channel(1);
    let (transfer_tx, transfer_rx) = tokio::sync::mpsc::channel(1);
    {
        let mut state = lock(&shared.state);
        if let Some(session) = state.incoming.as_mut() {
            if session.id == session_id {
                session.clipboard = Some(clipboard_tx);
                session.transfer = Some(transfer_tx);
            }
        }
    }
    let (gate_tx, gate_rx) = tokio::sync::oneshot::channel();
    let clipboard_connection = iroh::endpoint::Connection::clone(&connection);
    let transfer_connection = connection;
    let mut clipboard_stop = tokio::sync::watch::Receiver::clone(&stop_rx);
    let mut transfer_stop = stop_rx;
    let shared_apply = Arc::clone(shared);
    let shared_allow = Arc::clone(shared);
    let clipboard_task = tokio::spawn(async move {
        let opened = clipboard::accept_bi(&clipboard_connection, &mut clipboard_stop).await;
        let _ = gate_tx.send(opened.is_ok());
        if let Ok((send, recv)) = opened {
            clipboard::exchange(
                send,
                recv,
                clipboard_rx,
                clipboard_stop,
                move |text| apply_clipboard(&shared_apply, &peer, text),
                move || {
                    if lock(&shared_allow.state).trust.clipboard_granted(&peer) {
                        Ok(())
                    } else {
                        Err(Error::ClipboardDenied)
                    }
                },
                false,
            )
            .await;
        }
    });
    let shared_inspect = Arc::clone(shared);
    let shared_write = Arc::clone(shared);
    let shared_allow_files = Arc::clone(shared);
    let transfer_task = tokio::spawn(async move {
        if !matches!(gate_rx.await, Ok(true)) {
            return;
        }
        let Ok((send, recv)) = transfer::accept_bi(&transfer_connection, &mut transfer_stop).await
        else {
            return;
        };
        transfer::exchange(
            send,
            recv,
            transfer_rx,
            transfer_stop,
            move |name, size, _replace| {
                transfer::check_name(name)?;
                transfer::checked_size(size)?;
                if !lock(&shared_inspect.state).trust.transfer_granted(&peer) {
                    return Err(Error::TransferDenied);
                }
                if lock(&shared_inspect.transfer_dir).is_none() {
                    return Err(Error::Unsupported("no transfer directory is installed"));
                }
                Ok(())
            },
            move |name, bytes, replace| {
                apply_transfer(&shared_write, &peer, name, bytes, replace)?;
                shared_write
                    .events
                    .publish(CoreEvent::TransferCompleted { id: session_id });
                Ok(())
            },
            move || {
                if lock(&shared_allow_files.state)
                    .trust
                    .transfer_granted(&peer)
                {
                    Ok(())
                } else {
                    Err(Error::TransferDenied)
                }
            },
        )
        .await;
    });
    (clipboard_task, transfer_task)
}

async fn open_controller_clipboard(
    shared: &Arc<Shared>,
    connection: &iroh::endpoint::Connection,
    session_id: u64,
    stop_rx: &tokio::sync::watch::Receiver<bool>,
) -> Result<tokio::task::JoinHandle<()>, Error> {
    let (tx, rx) = tokio::sync::mpsc::channel(1);
    let (opened_tx, opened_rx) = tokio::sync::oneshot::channel();
    let shared_text = Arc::clone(shared);
    let connection = iroh::endpoint::Connection::clone(connection);
    let stop_rx = tokio::sync::watch::Receiver::clone(stop_rx);
    let task = tokio::spawn(async move {
        clipboard::attach(
            connection,
            true,
            rx,
            stop_rx,
            Some(opened_tx),
            move |text| {
                *lock(&shared_text.clipboard_inbox) = Some(text.to_owned());
                shared_text
                    .events
                    .publish(CoreEvent::ClipboardReceived { id: session_id });
                Ok(())
            },
            || Ok(()),
            true,
        )
        .await;
    });
    match tokio::time::timeout(Duration::from_secs(10), opened_rx).await {
        Ok(Ok(Ok(()))) => {}
        Ok(Ok(Err(error))) => {
            task.abort();
            return Err(error);
        }
        _ => {
            task.abort();
            return Err(Error::network("the clipboard stream did not open"));
        }
    }
    {
        let mut state = lock(&shared.state);
        if let Some(session) = state.outgoing.as_mut() {
            if session.id == session_id {
                session.clipboard = Some(tx);
            }
        }
    }
    *lock(&shared.clipboard_inbox) = None;
    Ok(task)
}

async fn open_controller_transfer(
    shared: &Arc<Shared>,
    connection: &iroh::endpoint::Connection,
    session_id: u64,
    stop_rx: &tokio::sync::watch::Receiver<bool>,
) -> Result<tokio::task::JoinHandle<()>, Error> {
    let (tx, rx) = tokio::sync::mpsc::channel(1);
    let (opened_tx, opened_rx) = tokio::sync::oneshot::channel();
    let shared_inspect = Arc::clone(shared);
    let shared_write = Arc::clone(shared);
    let connection = iroh::endpoint::Connection::clone(connection);
    let stop_rx = tokio::sync::watch::Receiver::clone(stop_rx);
    let task = tokio::spawn(async move {
        transfer::attach(
            connection,
            true,
            rx,
            stop_rx,
            Some(opened_tx),
            move |name, size, _replace| {
                transfer::check_name(name)?;
                transfer::checked_size(size)?;
                if lock(&shared_inspect.transfer_dir).is_none() {
                    return Err(Error::Unsupported("no transfer directory is installed"));
                }
                Ok(())
            },
            move |name, bytes, replace| {
                let root = lock(&shared_write.transfer_dir)
                    .clone()
                    .ok_or(Error::Unsupported("no transfer directory is installed"))?;
                transfer::write_file(&root, name, bytes, replace)?;
                shared_write
                    .events
                    .publish(CoreEvent::TransferCompleted { id: session_id });
                Ok(())
            },
            || Ok(()),
        )
        .await;
    });
    match tokio::time::timeout(Duration::from_secs(10), opened_rx).await {
        Ok(Ok(Ok(()))) => {}
        Ok(Ok(Err(error))) => {
            task.abort();
            return Err(error);
        }
        _ => {
            task.abort();
            return Err(Error::network("the transfer stream did not open"));
        }
    }
    {
        let mut state = lock(&shared.state);
        if let Some(session) = state.outgoing.as_mut() {
            if session.id == session_id {
                session.transfer = Some(tx);
            }
        }
    }
    Ok(task)
}

fn clipboard_sender(
    state: &State,
    incoming: bool,
    session_id: u64,
    generation: Option<u64>,
) -> Result<ClipboardSender, Error> {
    let session = if incoming {
        state.incoming.as_ref()
    } else {
        state.outgoing.as_ref()
    }
    .ok_or(Error::NoSession)?;
    if session.phase != SessionPhase::Active || session.id != session_id {
        return Err(Error::NoSession);
    }
    if let Some(generation) = generation {
        if session.generation != generation {
            return Err(Error::StaleSession);
        }
    }
    session
        .clipboard
        .clone()
        .ok_or(Error::Unsupported("clipboard stream is not open"))
}

fn clipboard_round_trip(tx: ClipboardSender, text: &str) -> Result<(), Error> {
    let (reply_tx, reply_rx) = std::sync::mpsc::sync_channel(1);
    match tx.try_send(ClipboardRequest {
        text: text.to_owned(),
        reply: reply_tx,
    }) {
        Ok(()) => {}
        Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => return Err(Error::Busy),
        Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => {
            return Err(Error::Unsupported("clipboard stream is closed"));
        }
    }
    match reply_rx.recv_timeout(Duration::from_secs(10)) {
        Ok(result) => result,
        Err(_) => Err(Error::Timeout),
    }
}

fn apply_clipboard(shared: &Shared, peer: &[u8; 32], text: &str) -> Result<(), Error> {
    if !lock(&shared.state).trust.clipboard_granted(peer) {
        return Err(Error::ClipboardDenied);
    }
    let mut slot = lock(&shared.clipboard);
    let Some(adapter) = slot.as_mut() else {
        return Err(Error::Unsupported("no clipboard adapter is installed"));
    };
    adapter.set_text(text)
}

fn transfer_sender(
    state: &State,
    incoming: bool,
    session_id: u64,
    generation: Option<u64>,
) -> Result<TransferSender, Error> {
    let session = if incoming {
        state.incoming.as_ref()
    } else {
        state.outgoing.as_ref()
    }
    .ok_or(Error::NoSession)?;
    if session.phase != SessionPhase::Active || session.id != session_id {
        return Err(Error::NoSession);
    }
    if let Some(generation) = generation {
        if session.generation != generation {
            return Err(Error::StaleSession);
        }
    }
    session
        .transfer
        .clone()
        .ok_or(Error::Unsupported("transfer stream is not open"))
}

fn transfer_round_trip(
    tx: TransferSender,
    name: &str,
    bytes: &[u8],
    replace: bool,
) -> Result<(), Error> {
    let (reply_tx, reply_rx) = std::sync::mpsc::sync_channel(1);
    match tx.try_send(TransferRequest {
        name: name.to_owned(),
        bytes: bytes.to_vec(),
        replace,
        reply: reply_tx,
    }) {
        Ok(()) => {}
        Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => return Err(Error::Busy),
        Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => {
            return Err(Error::Unsupported("transfer stream is closed"));
        }
    }
    match reply_rx.recv_timeout(Duration::from_secs(10)) {
        Ok(result) => result,
        Err(_) => Err(Error::Timeout),
    }
}

fn apply_transfer(
    shared: &Shared,
    peer: &[u8; 32],
    name: &str,
    bytes: &[u8],
    replace: bool,
) -> Result<(), Error> {
    if !lock(&shared.state).trust.transfer_granted(peer) {
        return Err(Error::TransferDenied);
    }
    let root = lock(&shared.transfer_dir)
        .clone()
        .ok_or(Error::Unsupported("no transfer directory is installed"))?;
    transfer::write_file(&root, name, bytes, replace)
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

fn video_inbox(shared: &Shared) -> Arc<VideoInbox> {
    let mut slot = lock(&shared.video_inbox);
    if let Some(inbox) = slot.as_ref() {
        return Arc::clone(inbox);
    }
    let inbox = Arc::new(VideoInbox::new());
    *slot = Some(Arc::clone(&inbox));
    inbox
}

async fn host_video_loop(
    connection: iroh::endpoint::Connection,
    outbox: Arc<VideoOutbox>,
    stop_rx: &mut tokio::sync::watch::Receiver<bool>,
) {
    let mut send = tokio::select! {
        opened = connection.open_uni() => {
            match opened {
                Ok(send) => send,
                Err(_) => return,
            }
        }
        _ = connection.closed() => return,
        _ = stop_rx.changed() => return,
    };
    loop {
        if *stop_rx.borrow() {
            return;
        }
        let notified = outbox.notified();
        tokio::pin!(notified);
        let frames = outbox.drain();
        if frames.is_empty() {
            tokio::select! {
                _ = &mut notified => {}
                _ = connection.closed() => return,
                _ = stop_rx.changed() => return,
            }
        } else {
            for frame in frames {
                if write_video_frame(&mut send, &frame).await.is_err() {
                    return;
                }
            }
        }
    }
}

async fn video_recv_loop(
    connection: iroh::endpoint::Connection,
    inbox: Arc<VideoInbox>,
    keyframe: Arc<KeyframeFlag>,
    stop_rx: &mut tokio::sync::watch::Receiver<bool>,
) {
    let mut recv = tokio::select! {
        incoming = connection.accept_uni() => {
            match incoming {
                Ok(recv) => recv,
                Err(_) => return,
            }
        }
        _ = connection.closed() => return,
        _ = stop_rx.changed() => return,
    };
    loop {
        if *stop_rx.borrow() {
            return;
        }
        let read = tokio::select! {
            read = read_video_frame(&mut recv) => read,
            _ = connection.closed() => return,
            _ = stop_rx.changed() => return,
        };
        match read {
            Ok(frame) => {
                if matches!(inbox.push(frame), VideoPush::KeyframeNeeded) {
                    keyframe.request();
                }
            }
            // The body was consumed. A codec this install does not know must not end input.
            Err(Error::Unsupported(_)) => {}
            Err(_) => return,
        }
    }
}

async fn write_video_frame(
    send: &mut iroh::endpoint::SendStream,
    frame: &EncodedFrame,
) -> Result<(), Error> {
    let body = encode_video_body(frame);
    crate::video::check_video_len(body.len())?;
    let len = u32::try_from(body.len()).map_err(|_| Error::Malformed)?;
    send.write_all(&len.to_be_bytes())
        .await
        .map_err(Error::network_debug)?;
    send.write_all(&body).await.map_err(Error::network_debug)?;
    Ok(())
}

async fn read_video_frame(recv: &mut iroh::endpoint::RecvStream) -> Result<EncodedFrame, Error> {
    let mut len_buf = [0u8; 4];
    recv.read_exact(&mut len_buf)
        .await
        .map_err(Error::network_debug)?;
    let len = usize::try_from(u32::from_be_bytes(len_buf)).map_err(|_| Error::Malformed)?;
    crate::video::check_video_len(len)?;
    let mut body = vec![0u8; len];
    recv.read_exact(&mut body)
        .await
        .map_err(Error::network_debug)?;
    decode_video_body(&body)
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
