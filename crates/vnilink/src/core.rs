use std::net::SocketAddr;
use std::str::FromStr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use iroh::endpoint::presets;
use iroh::{Endpoint, EndpointAddr, EndpointId, PublicKey, RelayMode, SecretKey};

use crate::error::Error;
use crate::events::{CoreEvent, EventHub, EventSubscription};
use crate::input::{HeldInput, HostDesktop, InputEvent, InputOutbox};
use crate::invitation::{self, ReplayCache};
use crate::os_input::OsInput;
use crate::protocol::{
    error_code, Capabilities, ControlMessage, Quality, ALPN, MAX_CONTROL_FRAME, PROTOCOL_VERSION,
};
use crate::relay::RelaySettings;
use crate::session::{self, Grants, SessionInfo, SessionPhase, SessionRole};
use crate::trust::{Device, Trust};

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
    /// Restored installation identity. `None` generates a new one for this process.
    /// Platform credential storage is not connected yet, so a new process mints a new id
    /// unless the host passes the bytes it saved.
    pub identity: Option<[u8; 32]>,
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
}

struct State {
    hosting: bool,
    trust: Trust,
    replay: ReplayCache,
    incoming: Option<LiveSession>,
    outgoing: Option<LiveSession>,
    next_session_id: u64,
    generation: u64,
    shut_down: bool,
}

struct Shared {
    endpoint: Endpoint,
    secret: SecretKey,
    relay: RelaySettings,
    events: EventHub,
    state: Mutex<State>,
    desktop: Mutex<Option<Box<dyn HostDesktop>>>,
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

    pub(crate) fn boot(
        config: CoreConfig,
        relay_mode: RelayMode,
        direct_only: bool,
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
            direct_only || !config.relay.is_public(),
        ))?;
        let shared = Arc::new(Shared {
            endpoint,
            secret,
            relay: config.relay,
            events: EventHub::new(event_capacity),
            state: Mutex::new(State {
                hosting: false,
                trust: Trust::default(),
                replay: ReplayCache::new(),
                incoming: None,
                outgoing: None,
                next_session_id: 1,
                generation: 0,
                shut_down: false,
            }),
            desktop: Mutex::new(None),
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
            signal_stop(state.incoming.as_mut());
            signal_stop(state.outgoing.as_mut());
        }
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
    pub fn export_identity(&self) -> [u8; 32] {
        self.shared.secret.to_bytes()
    }

    pub fn subscribe(&self) -> Result<EventSubscription, Error> {
        self.ensure_alive()?;
        self.shared.events.subscribe()
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
            capabilities: desktop_capabilities(&self.shared.desktop),
        })
    }

    pub fn capabilities(&self) -> Capabilities {
        desktop_capabilities(&self.shared.desktop)
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
            state.replay.remember(nonce, opened.expires_at_ms, now_ms)?;
            state.trust.pair(Device {
                endpoint_id: opened.endpoint_id,
                display_id: display.clone(),
                relays: opened.relays,
                direct: opened.direct,
                grants: Grants::default(),
            })?;
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
            state.trust.authorize(&key, grants)?;
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
            state.trust.revoke(&key)?;
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
        self.runtime.spawn(async move {
            match controller_handshake(shared.clone(), device, stop_rx, outbox).await {
                Ok(info) => {
                    let _ = ready_tx.send(Ok(info));
                }
                Err(error) => {
                    fail_outgoing(&shared, &error);
                    let _ = ready_tx.send(Err(error));
                }
            }
        });
        match ready_rx.recv_timeout(Duration::from_secs(15)) {
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
        outbox.push(event)
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
    let peer_key = *peer.as_bytes();
    let (stop, mut stop_rx) = tokio::sync::watch::channel(false);
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
                match handle_host_request(&shared, peer, send, recv, &stop, &mut stop_rx).await {
                    HostTurn::PeerShouldClose => {
                        // The controller reads the reject and then drops the connection.
                        // Closing first discards that frame.
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
    finish_incoming(&shared, &peer_key);
    connection.close(1u8.into(), b"closed");
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum HostTurn {
    /// A reject frame was written. Wait until the controller reads it.
    PeerShouldClose,
    /// The controlling session has ended and held keys were released.
    Done,
}

async fn handle_host_request(
    shared: &Shared,
    peer: EndpointId,
    mut send: iroh::endpoint::SendStream,
    mut recv: iroh::endpoint::RecvStream,
    stop: &tokio::sync::watch::Sender<bool>,
    stop_rx: &mut tokio::sync::watch::Receiver<bool>,
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
            match decision {
                Err(error) => {
                    let _ =
                        write_frame(&mut send, &ControlMessage::Reject(error_code(&error))).await;
                    HostTurn::PeerShouldClose
                }
                Ok(()) => match register_incoming(shared, peer, stop) {
                    Ok(info) => {
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
                        if wrote.is_ok() {
                            host_input_loop(shared, info.id, &mut send, &mut recv, stop_rx).await;
                            HostTurn::Done
                        } else {
                            HostTurn::PeerShouldClose
                        }
                    }
                    Err(error) => {
                        let _ = write_frame(&mut send, &ControlMessage::Reject(error_code(&error)))
                            .await;
                        HostTurn::PeerShouldClose
                    }
                },
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

async fn host_input_loop(
    shared: &Shared,
    session_id: u64,
    send: &mut iroh::endpoint::SendStream,
    recv: &mut iroh::endpoint::RecvStream,
    stop_rx: &mut tokio::sync::watch::Receiver<bool>,
) {
    let mut held = HeldInput::default();
    let mut input_open = true;
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
            frame = read_frame(recv) => {
                let message = match frame {
                    Ok(message) => message,
                    Err(Error::Malformed) => {
                        let _ = write_frame(send, &ControlMessage::Reject(error_code(&Error::Malformed))).await;
                        break;
                    }
                    Err(_) => break,
                };
                let ControlMessage::Input { session_id: event_session, generation, event } = message else {
                    let _ = write_frame(send, &ControlMessage::Reject(error_code(&Error::Malformed))).await;
                    break;
                };
                let decision = {
                    let state = lock(&shared.state);
                    let active = state.incoming.as_ref().and_then(|session| {
                        (session.phase == SessionPhase::Active).then_some((session.id, session.generation))
                    });
                    session::check_input(active, event_session, generation)
                };
                if let Err(error) = decision {
                    let _ = write_frame(send, &ControlMessage::Reject(error_code(&error))).await;
                    break;
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
                        shared.events.publish(CoreEvent::InputFailed {
                            id: session_id,
                            message: crate::input::InputFault::from_error(&error).message(),
                        });
                        if input_open && matches!(error, Error::Unsupported(_)) {
                            input_open = false;
                            let _ = write_frame(send, &ControlMessage::Reject(error_code(&error))).await;
                        }
                    }
                }
            }
        }
    }
    release_held(shared, session_id, &held);
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

fn finish_incoming(shared: &Shared, peer_key: &[u8; 32]) {
    let mut state = lock(&shared.state);
    let matches = state
        .incoming
        .as_ref()
        .is_some_and(|session| session.peer_key == *peer_key);
    if !matches {
        return;
    }
    let active = state.incoming.as_ref().expect("checked").phase == SessionPhase::Active;
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

async fn controller_handshake(
    shared: Arc<Shared>,
    device: Device,
    mut stop_rx: tokio::sync::watch::Receiver<bool>,
    outbox: Arc<InputOutbox>,
) -> Result<SessionInfo, Error> {
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
    let ready = info.clone();
    let session_id = info.id;
    let generation = info.generation;
    tokio::spawn(async move {
        command_loop(
            connection,
            send,
            recv,
            outbox,
            session_id,
            generation,
            &shared,
            &mut stop_rx,
        )
        .await;
        finish_outgoing(&shared);
    });
    Ok(ready)
}

async fn command_loop(
    connection: iroh::endpoint::Connection,
    mut send: iroh::endpoint::SendStream,
    mut recv: iroh::endpoint::RecvStream,
    outbox: Arc<InputOutbox>,
    session_id: u64,
    generation: u64,
    shared: &Shared,
    stop_rx: &mut tokio::sync::watch::Receiver<bool>,
) {
    let (frames_tx, mut frames_rx) = tokio::sync::mpsc::channel(8);
    tokio::spawn(async move {
        loop {
            match read_frame(&mut recv).await {
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
    loop {
        if *stop_rx.borrow() {
            break;
        }
        let notified = outbox.notified();
        tokio::pin!(notified);
        let batch = outbox.drain();
        if batch.is_empty() {
            tokio::select! {
                _ = &mut notified => {}
                changed = stop_rx.changed() => {
                    if changed.is_err() || *stop_rx.borrow() {
                        break;
                    }
                }
                frame = frames_rx.recv() => {
                    match frame {
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
            for event in batch {
                let wrote = write_frame(
                    &mut send,
                    &ControlMessage::Input {
                        session_id,
                        generation,
                        event,
                    },
                )
                .await;
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
    let payload = message.encode();
    if payload.is_empty() || payload.len() > MAX_CONTROL_FRAME {
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
    let mut len_buf = [0u8; 2];
    tokio::time::timeout(Duration::from_secs(10), recv.read_exact(&mut len_buf))
        .await
        .map_err(|_| Error::Timeout)?
        .map_err(Error::network_debug)?;
    let len = usize::from(u16::from_be_bytes(len_buf));
    if len == 0 || len > MAX_CONTROL_FRAME {
        return Err(Error::Malformed);
    }
    let mut payload = vec![0u8; len];
    tokio::time::timeout(Duration::from_secs(10), recv.read_exact(&mut payload))
        .await
        .map_err(|_| Error::Timeout)?
        .map_err(Error::network_debug)?;
    ControlMessage::decode(&payload)
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
    lock(&shared.desktop)
        .as_ref()
        .map(|desktop| desktop.monitor())
        .unwrap_or((0, 0))
}

fn desktop_capabilities(desktop: &Mutex<Option<Box<dyn HostDesktop>>>) -> Capabilities {
    let guard = lock(desktop);
    match guard.as_ref() {
        None => Capabilities::UNSUPPORTED,
        Some(desktop) => {
            let (width, height) = desktop.monitor();
            Capabilities {
                desktop_capture: false,
                keyboard: true,
                pointer: true,
                clipboard: false,
                file_transfer: false,
                audio: false,
                monitors: u8::from(width > 0 && height > 0),
            }
        }
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

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poison| poison.into_inner())
}
