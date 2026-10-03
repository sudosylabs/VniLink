//! Hosting, the accept loop, and the side streams that start after Accept.

use super::*;

use super::clipboard::apply_clipboard;
use super::connect::{remember_link, write_addresses};
use super::files::apply_transfer;
use super::keys::host_input_loop;
use super::pairing::{wait_for_control_approval, ControlAnswer};
use super::video::host_video_loop;
use super::wire::{read_frame, write_frame};

use std::sync::Arc;
use std::time::Duration;

use iroh::endpoint::presets;
use iroh::{Endpoint, EndpointId, RelayMode, SecretKey};

use crate::error::Error;
use crate::events::CoreEvent;
use crate::protocol::{error_code, ControlMessage, ALPN};
use crate::session::{self, SessionInfo, SessionPhase, SessionRole};
use crate::transfer::{self};
use crate::video::VideoOutbox;

impl Core {
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

    pub(super) fn stop_incoming(&self) -> Result<(), Error> {
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
}

pub(super) async fn bind_endpoint(
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
                Ok(connection) => host_connection(shared, connection).await,
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
pub(super) enum HostTurn {
    /// A reject or close frame was written. Wait until the controller reads it.
    PeerShouldClose,
    /// The host ended the session and wrote a close frame.
    LocalStop,
    /// The controlling session has ended and held keys were released.
    Done,
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

pub(super) fn register_incoming(
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

pub(super) fn finish_incoming(shared: &Shared, session_id: u64) {
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

/// Clipboard is accepted before transfer. Two `accept_bi` calls at once can
/// take each other's stream, so the transfer accept starts only after the
/// clipboard accept has returned.
pub(super) fn spawn_host_streams(
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
        let opened = crate::clipboard::accept_bi(&clipboard_connection, &mut clipboard_stop).await;
        let _ = gate_tx.send(opened.is_ok());
        if let Ok((send, recv)) = opened {
            crate::clipboard::exchange(
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
