//! Hosting, the accept loop, and the side streams that start after Accept.
// trace: records every ignored failure

use super::*;

use super::keys::host_command_loop;
use super::link::{remember_link, write_addresses};
use super::pairing::{wait_for_control_approval, ControlAnswer};
use super::transfer::{is_files_connection, serve_files_connection};
use super::wire::{read_frame, write_frame};
use crate::transfer::FILES_ALPN;

use std::sync::Arc;
use std::time::Duration;

use iroh::endpoint::{presets, QuicTransportConfig, VarInt};
use iroh::{Endpoint, EndpointId, RelayMode, SecretKey};

use crate::error::Error;
use crate::events::CoreEvent;
use crate::protocol::{error_code, ControlMessage, SessionStamp, ALPN};
use crate::session::{self, SessionInfo, SessionPhase, SessionRole};

impl Core {
    pub fn start_hosting(&self) -> Result<(), Error> {
        let _engine = self.engine();
        let mut state = self.alive_state()?;
        state.hosting = true;
        drop(state);
        self.shared
            .events
            .publish(CoreEvent::HostingChanged { hosting: true });
        Ok(())
    }

    /// Disables hosting and fences every live hosted peer in one state
    /// operation: no hosted session stays usable and no new Hello is
    /// admitted afterwards. Owner tasks observe their parent stop and
    /// close asynchronously; outgoing (controller-side) sessions are
    /// untouched.
    pub fn stop_hosting(&self) -> Result<(), Error> {
        let _engine = self.engine();
        let stops = {
            let mut state = self.alive_state()?;
            state.hosting = false;
            state.generation = state.generation.saturating_add(1);
            state
                .incoming
                .values_mut()
                .filter(|session| !finished(&session.phase))
                .map(|session| {
                    session.enter(SessionPhase::Stopping);
                    session.stop.clone()
                })
                .collect::<Vec<_>>()
        };
        for stop in stops {
            crate::ignored!(
                crate::trace::SESSION,
                "work that already ended needs no stop",
                stop.send(true)
            );
        }
        self.shared
            .events
            .publish(CoreEvent::HostingChanged { hosting: false });
        Ok(())
    }

    /// Ends the hosted session and leaves hosting enabled.
    ///
    /// With more than one live peer this stops an arbitrary one. Call
    /// [`Core::stop_hosted_peer`] when the session id is known.
    pub fn stop_hosted_session(&self) -> Result<(), Error> {
        let _engine = self.engine();
        self.stop_incoming()
    }

    /// Ends one hosted peer. Other live peers, their services and their
    /// capture consumers stay up. Hosting itself stays enabled.
    pub fn stop_hosted_peer(&self, id: u64) -> Result<(), Error> {
        let _engine = self.engine();
        let stop = {
            let mut state = self.alive_state()?;
            let session = state.incoming.get_mut(&id).ok_or(Error::NoSession)?;
            if finished(&session.phase) {
                return Err(Error::NoSession);
            }
            session.enter(SessionPhase::Stopping);
            session.stop.clone()
        };
        crate::ignored!(
            crate::trace::SESSION,
            "work that already ended needs no stop",
            stop.send(true)
        );
        Ok(())
    }

    pub(super) fn stop_incoming(&self) -> Result<(), Error> {
        let mut state = self.alive_state()?;
        let stop = {
            let session = state.incoming_live_mut().ok_or(Error::NoSession)?;
            if finished(&session.phase) {
                return Err(Error::NoSession);
            }
            session.enter(SessionPhase::Stopping);
            session.stop.clone()
        };
        state.generation = state.generation.saturating_add(1);
        drop(state);
        crate::ignored!(
            crate::trace::SESSION,
            "work that already ended needs no stop",
            stop.send(true)
        );
        Ok(())
    }
}

/// Bytes one stream may have in flight unacknowledged. noq's default
/// (1.25 MB) caps a single stream at about 12.5 MB/s at 100 ms; this is
/// enough for a gigabit link at 130 ms.
const STREAM_RECEIVE_WINDOW: u32 = 16 * 1024 * 1024;
/// Bytes all streams of one connection may have in flight, which bounds
/// the receive memory a peer can claim.
pub(super) const CONNECTION_RECEIVE_WINDOW: u32 = 64 * 1024 * 1024;
/// Bytes sent and not yet acknowledged, across the connection.
const SEND_WINDOW: u64 = 64 * 1024 * 1024;
/// Datagram bytes the transport may hold before sending. noq's default,
/// stated so the desktop sender can tell how much video is waiting.
pub(super) const DATAGRAM_SEND_BUFFER: usize = 1024 * 1024;

/// Flow-control windows sized for bulk transfers on long paths. Congestion
/// control stays noq's default, Cubic: its BBRv3 is experimental, and a
/// change is made only on measurement. Datagram buffering keeps noq's
/// default size; the desktop sender bounds its own backlog within it.
fn transport_config() -> QuicTransportConfig {
    transport_config_receiving(VarInt::from_u32(CONNECTION_RECEIVE_WINDOW))
}

/// The transport a files connection is dialed with: the same, but starting
/// from a small receive window, so a sender cannot have the whole window in
/// flight before its path is known. The receiver widens it from there.
pub(super) fn files_transport_config() -> QuicTransportConfig {
    transport_config_receiving(
        VarInt::from_u64(crate::transfer::INITIAL_WINDOW).unwrap_or(VarInt::MAX),
    )
}

fn transport_config_receiving(receive_window: VarInt) -> QuicTransportConfig {
    QuicTransportConfig::builder()
        .stream_receive_window(VarInt::from_u32(STREAM_RECEIVE_WINDOW))
        .receive_window(receive_window)
        .send_window(SEND_WINDOW)
        .datagram_send_buffer_size(DATAGRAM_SEND_BUFFER)
        .build()
}

pub(super) async fn bind_endpoint(
    secret: SecretKey,
    relay_mode: RelayMode,
    listen: Listen,
    clear_lookup: bool,
) -> Result<Endpoint, Error> {
    let relayed = !matches!(relay_mode, RelayMode::Disabled);
    let mut builder = Endpoint::builder(presets::N0)
        .secret_key(secret)
        .alpns(vec![ALPN.to_vec(), FILES_ALPN.to_vec()])
        .relay_mode(relay_mode)
        .transport_config(transport_config());
    if clear_lookup {
        builder = builder.clear_address_lookup();
    }
    if listen == Listen::Loopback {
        builder = builder
            .clear_ip_transports()
            .bind_addr("127.0.0.1:0")
            .map_err(Error::network_debug)?;
    }
    #[cfg(any(test, feature = "test-probes"))]
    if listen == Listen::RelayOnly {
        builder = builder.clear_ip_transports();
    }
    let endpoint = builder.bind().await.map_err(Error::network_debug)?;
    tracing::info!(
        target: crate::trace::RELAY,
        relay = relayed,
        listen = ?listen,
        "endpoint bound"
    );
    Ok(endpoint)
}

pub(super) async fn accept_loop(shared: Arc<Shared>) {
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
                Ok(connection) => {
                    super::transfer::peer_reached(&shared, connection.remote_id().as_bytes());
                    // Only a `test-probes` build has the agent's protocol.
                    #[cfg(feature = "test-probes")]
                    if super::agent::is_agent_connection(&connection) {
                        super::agent::serve_agent_connection(shared, connection).await;
                        return;
                    }
                    if is_files_connection(&connection) {
                        serve_files_connection(shared, connection).await;
                    } else {
                        host_connection(shared, connection).await;
                    }
                }
                Err(error) => {
                    let _ = error;
                }
            }
        });
    }
}

pub(super) async fn host_connection(shared: Arc<Shared>, connection: iroh::endpoint::Connection) {
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
                        crate::ignored!(
                            crate::trace::SESSION,
                            "a closing connection is closed at best",
                            tokio::time::timeout(Duration::from_secs(2), connection.closed())
                                    .await
                        );
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
    close_link(&connection, lock(&shared.state).shut_down, 1, b"closed");
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum HostTurn {
    /// A reject or close frame was written. Wait until the controller reads it.
    PeerShouldClose,
    /// The host ended the session and wrote a close frame.
    LocalStop,
    /// The session has ended.
    Done,
}

/// One admission check for every Hello: before approval, after approval, and
/// after the session gate. The rule lives in [`session::admit_services`];
/// this only reads the state it needs.
fn admission(state: &State, peer: EndpointId, version: u16) -> Result<(), Error> {
    // One link per paired peer; a newer one replaces the older.
    let other_links = state
        .incoming
        .values()
        .filter(|s| !finished(&s.phase) && s.peer_key != *peer.as_bytes())
        .count();
    session::admit_services(
        state.hosting,
        other_links >= crate::trust::MAX_DEVICES,
        state.trust.get(peer.as_bytes()).is_some(),
        version,
    )
}

pub(super) async fn handle_host_request(
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
            crate::ignored!(
                crate::trace::SESSION,
                "the peer may be gone before it hears why",
                write_frame(
                    &mut send,
                    &ControlMessage::Reject(error_code(&Error::Malformed)),
                )
                .await
            );
            return HostTurn::PeerShouldClose;
        }
    };
    match message {
        // A link carries no picture: a Desktop service opens its own stream
        // on it.
        ControlMessage::Hello {
            version,
            hostname: hello_hostname,
            os: hello_os,
            ..
        } => {
            let decision = admission(&lock(&shared.state), peer, version);
            let decision = match decision {
                // The file already told the controller who this host is. The
                // digits typed here tell the host who the controller is.
                // Without an open allow switch the reject stays immediate.
                Err(Error::Unauthorized | Error::NotPaired)
                    if lock(&shared.state).trust.get(peer.as_bytes()).is_none() =>
                {
                    match wait_for_control_approval(shared, connection, peer, stop_rx).await {
                        ControlAnswer::Approved => admission(&lock(&shared.state), peer, version),
                        ControlAnswer::Rejected(error) => Err(error),
                    }
                }
                other => other,
            };
            match decision {
                Err(error) => {
                    crate::ignored!(
                        crate::trace::SESSION,
                        "the peer may be gone before it hears why",
                        write_frame(&mut send, &ControlMessage::Reject(error_code(&error))).await
                    );
                    HostTurn::PeerShouldClose
                }
                Ok(()) => {
                    // Admission counts live sessions: one Hello passes the
                    // checks and registers before the next is decided.
                    let _gate = shared.session_gate.lock().await;
                    let decision = admission(&lock(&shared.state), peer, version);
                    if let Err(error) = decision {
                        crate::ignored!(
                            crate::trace::SESSION,
                            "the peer may be gone before it hears why",
                            write_frame(&mut send, &ControlMessage::Reject(error_code(&error)))
                                .await
                        );
                        return HostTurn::PeerShouldClose;
                    }
                    match register_incoming(shared, peer, stop) {
                        Ok(info) => {
                            *owned_session = Some(info.id);
                            remember_link(shared, info.id, connection.clone());
                            // Admission is complete: release the gate so a
                            // concurrent peer's Hello is not serialized behind
                            // this session's lifetime. Everything below runs
                            // per connection without shared admission state.
                            drop(_gate);
                            // The peer is admitted: keep its display name for
                            // this host's device list. Persist failures stay
                            // local; the next dial refreshes the name.
                            crate::ignored!(
                                crate::trace::SESSION,
                                "a peer's name and addresses are refreshed at the next connection",
                                super::link::remember_peer_hostname(
                                    shared,
                                    peer.as_bytes(),
                                    hello_hostname,
                                )
                            );
                            crate::ignored!(
                                crate::trace::SESSION,
                                "a peer's name and addresses are refreshed at the next connection",
                                super::link::remember_peer_os(shared, peer.as_bytes(), hello_os)
                            );
                            // Serve the link's streams before Accept can make the
                            // controller's connect return, so a service it opens at
                            // once is not refused.
                            let mut streams = spawn_host_streams(
                                shared,
                                iroh::endpoint::Connection::clone(connection),
                                info.id,
                                *peer.as_bytes(),
                                tokio::sync::watch::Receiver::clone(stop_rx),
                            );
                            // The desktop fields of the Accept, which a
                            // Desktop service's own stream fills, stay at
                            // their defaults.
                            let wrote = write_frame(
                                &mut send,
                                &ControlMessage::Accept {
                                    stamp: SessionStamp::new(info.id, info.generation),
                                    monitor_width: 0,
                                    monitor_height: 0,
                                    codec: 0,
                                    quality: session::controller_offer(),
                                    hostname: shared.local_hostname(),
                                    os: crate::OperatingSystem::local(),
                                },
                            )
                            .await;
                            if wrote.is_err() || write_addresses(shared, &mut send).await.is_err() {
                                streams.abort();
                                finish_incoming(shared, info.id);
                                return HostTurn::PeerShouldClose;
                            }
                            let local_stop = host_command_loop(
                                shared, info.id, peer, &mut send, &mut recv, stop_rx,
                            )
                            .await;
                            // Service streams end on the session's stop and
                            // tell their peers why before its close below;
                            // one that has not ended by then is cut off.
                            match tokio::time::timeout(STREAMS_ENDING, &mut streams).await {
                                Ok(ended) => {
                                    crate::ignored!(
                                        crate::trace::SESSION,
                                        "a stream task that failed has ended all the same",
                                        ended
                                    );
                                }
                                Err(_) => streams.abort(),
                            }
                            if local_stop {
                                crate::ignored!(
                                    crate::trace::SESSION,
                                    "the peer may be gone before it hears why",
                                    write_frame(&mut send, &ControlMessage::Close).await
                                );
                                crate::ignored!(
                                    crate::trace::SESSION,
                                    "a stream that cannot finish is closed with its link",
                                    send.finish()
                                );
                            }
                            finish_incoming(shared, info.id);
                            if local_stop {
                                HostTurn::LocalStop
                            } else {
                                HostTurn::Done
                            }
                        }
                        Err(error) => {
                            crate::ignored!(
                                crate::trace::SESSION,
                                "the peer may be gone before it hears why",
                                write_frame(&mut send, &ControlMessage::Reject(error_code(&error)))
                                    .await
                            );
                            HostTurn::PeerShouldClose
                        }
                    }
                }
            }
        }
        _ => {
            crate::ignored!(
                crate::trace::SESSION,
                "the peer may be gone before it hears why",
                write_frame(
                    &mut send,
                    &ControlMessage::Reject(error_code(&Error::Malformed)),
                )
                .await
            );
            HostTurn::PeerShouldClose
        }
    }
}

pub(super) fn register_incoming(
    shared: &Shared,
    peer: EndpointId,
    stop: &tokio::sync::watch::Sender<bool>,
) -> Result<SessionInfo, Error> {
    let mut state = lock(&shared.state);
    if state.shut_down {
        return Err(Error::ShutDown);
    }
    // Finished records are retained for status reads; drop them here so the
    // map holds only live sessions plus the latest finished one per id.
    state.incoming.retain(|_, s| !finished(&s.phase));
    // A peer that dials again has lost its old link; end it.
    for old in state
        .incoming
        .values()
        .filter(|s| s.peer_key == *peer.as_bytes())
    {
        old.stop.send_replace(true);
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
    };
    state.incoming.insert(
        id,
        LiveSession {
            wire_stamp: crate::protocol::SessionStamp::new(id, generation),
            id,
            generation,
            role: SessionRole::Host,
            phase: session::phase_after_admit(),
            peer_id: peer_id.clone(),
            peer_key: *peer.as_bytes(),
            peer_topology: Vec::new(),
            stop: stop.clone(),
            link: None,
            remote_availability: None,
        },
    );
    drop(state);
    session::announce_progression(&shared.events, id, generation, SessionRole::Host, &peer_id);
    // The peer just proved it is reachable; our own link to it dials now.
    super::presence::wake_link(shared, peer.as_bytes());
    Ok(info)
}

/// How long a stopping session's service streams have to end on their own,
/// telling their peers why, before they are cut off.
pub(super) const STREAMS_ENDING: Duration = Duration::from_secs(1);

pub(super) fn finish_incoming(shared: &Shared, session_id: u64) {
    let mut state = lock(&shared.state);
    let active = match state.incoming.get(&session_id) {
        Some(session) if !finished(&session.phase) => session.phase == SessionPhase::Active,
        _ => return,
    };
    if active {
        state.generation = state.generation.saturating_add(1);
    }
    let session = state.incoming.get_mut(&session_id).expect("checked");
    let info = session_info(session);
    session.enter(SessionPhase::Closed);
    super::services::synchronize(&mut state, &shared.events);
    drop(state);
    shared.events.publish(CoreEvent::SessionChanged {
        id: info.id,
        generation: info.generation,
        role: SessionRole::Host,
        phase: SessionPhase::Closed,
        peer_id: info.peer_id,
    });
}

/// Serves the link's streams: services, Desktop streams and job control,
/// each request on its own stream, until the session stops.
pub(super) fn spawn_host_streams(
    shared: &Arc<Shared>,
    connection: iroh::endpoint::Connection,
    session_id: u64,
    peer: [u8; 32],
    stop_rx: tokio::sync::watch::Receiver<bool>,
) -> tokio::task::JoinHandle<()> {
    let shared = shared.clone();
    tokio::spawn(async move {
        super::fs::serve_aux_host(shared, connection, session_id, peer, stop_rx).await;
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A stream may carry far more than noq's default 1.25 MB before its
    /// reader reads: the bulk window, not a timer, paces a long path.
    #[test]
    fn one_stream_sends_beyond_the_default_window_unread() {
        const PAYLOAD: usize = 8 * 1024 * 1024;
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            let bind = || {
                bind_endpoint(
                    SecretKey::generate(),
                    RelayMode::Disabled,
                    Listen::Loopback,
                    true,
                )
            };
            let host = bind().await.unwrap();
            let dialer = bind().await.unwrap();
            let address = host.addr();
            let accepting = tokio::spawn(async move {
                let connection = host.accept().await.unwrap().await.unwrap();
                // Accepted but never read until the writer is done.
                let (_send, recv) = connection.accept_bi().await.unwrap();
                (host, connection, recv)
            });
            let connection = dialer.connect(address, ALPN).await.unwrap();
            let (mut send, _recv) = connection.open_bi().await.unwrap();
            let written =
                tokio::time::timeout(Duration::from_secs(10), send.write_all(&vec![7u8; PAYLOAD]))
                    .await;
            assert!(
                matches!(written, Ok(Ok(()))),
                "an unread stream must take {PAYLOAD} bytes"
            );
            let (_host, _connection, mut recv) = accepting.await.unwrap();
            send.finish().unwrap();
            let received = recv.read_to_end(PAYLOAD).await.unwrap();
            assert_eq!(received.len(), PAYLOAD);
        });
    }

    /// An engine offers the test agent's protocol only while its agent
    /// listens, and one built without `test-probes` has no agent at all: a
    /// dial naming that protocol fails its handshake.
    #[test]
    fn an_engine_without_an_agent_refuses_its_protocol() {
        const AGENT_PROTOCOL: &[u8] = b"vnilink-agent/1";
        #[cfg(feature = "test-probes")]
        assert_eq!(AGENT_PROTOCOL, super::super::agent::AGENT_ALPN);
        let host = crate::test_support::loopback_core();
        let address = host.shared.endpoint.addr();
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            let dialer = bind_endpoint(
                SecretKey::generate(),
                RelayMode::Disabled,
                Listen::Loopback,
                true,
            )
            .await
            .unwrap();
            let dialed = tokio::time::timeout(
                Duration::from_secs(10),
                dialer.connect(address, AGENT_PROTOCOL),
            )
            .await
            .expect("the handshake ends");
            assert!(dialed.is_err(), "the agent's protocol is refused");
        });
    }
}
