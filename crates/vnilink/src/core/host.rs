//! Hosting, the accept loop, and the side streams that start after Accept.

use super::*;

use super::blob::{is_blob_connection, serve_blob_connection, BLOB_ALPN};
use super::clipboard::{
    apply_clipboard, apply_clipboard_files, clipboard_selected, deliver_paste_ready,
    spawn_paste_preparation,
};
use super::keys::host_input_loop;
use super::link::{remember_link, write_addresses};
use super::pairing::{wait_for_control_approval, ControlAnswer};
use super::video::host_video_loop;
use super::wire::{read_frame, write_frame};
use crate::clipboard::ClipboardPayload;

use std::sync::Arc;
use std::time::Duration;

use iroh::endpoint::presets;
use iroh::{Endpoint, EndpointId, RelayMode, SecretKey};

use crate::error::Error;
use crate::events::CoreEvent;
use crate::protocol::{error_code, ControlMessage, Quality, SessionStamp, ALPN};
use crate::session::{self, Grant, SessionInfo, SessionPhase, SessionRole};
use crate::video::VideoOutbox;

/// Live hosted peer slots. Each hosted connection owns its session record,
/// service owners, video/clipboard streams and input loop; the host
/// clipboard sync selects exactly one session (see `clipboard_owner`).
/// Four bounds concurrent task trees and per-connection queues while covering
/// two viewers plus Files/Terminal work. Capture is shared: each consumer has
/// its own encoder and bounded queue (`video_targets`). Native single-viewer
/// callers still address one session via `Snapshot::incoming`; multi-session
/// callers use `hosted_sessions()`.
const MAX_LIVE_INCOMING: usize = 4;

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

    /// Disables hosting and fences every live hosted peer in one state
    /// operation: no hosted session stays usable and no new Hello is
    /// admitted afterwards. Owner tasks observe their parent stop and
    /// close asynchronously; outgoing (controller-side) sessions are
    /// untouched.
    pub fn stop_hosting(&self) -> Result<(), Error> {
        let stops = {
            let mut state = self.alive_state()?;
            state.hosting = false;
            state.generation = state.generation.saturating_add(1);
            state
                .incoming
                .values_mut()
                .filter(|session| !finished(&session.phase))
                .map(|session| {
                    session.phase = SessionPhase::Stopping;
                    session.stop.clone()
                })
                .collect::<Vec<_>>()
        };
        for stop in stops {
            let _ = stop.send(true);
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
        self.stop_incoming()
    }

    /// Ends one hosted peer. Other live peers, their services and their
    /// capture consumers stay up. Hosting itself stays enabled.
    pub fn stop_hosted_peer(&self, id: u64) -> Result<(), Error> {
        let stop = {
            let mut state = self.alive_state()?;
            let session = state.incoming.get_mut(&id).ok_or(Error::NoSession)?;
            if finished(&session.phase) {
                return Err(Error::NoSession);
            }
            session.phase = SessionPhase::Stopping;
            session.stop.clone()
        };
        let _ = stop.send(true);
        Ok(())
    }

    pub(super) fn stop_incoming(&self) -> Result<(), Error> {
        let mut state = self.alive_state()?;
        let stop = {
            let session = state.incoming_live_mut().ok_or(Error::NoSession)?;
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
        .alpns(vec![ALPN.to_vec(), BLOB_ALPN.to_vec()])
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
                Ok(connection) => {
                    if is_blob_connection(&connection) {
                        serve_blob_connection(shared, connection).await;
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

/// One admission check for every Hello: before approval, after approval, and
/// after the session gate. The rule lives in [`session::admit`]; this only
/// reads the state it needs.
fn admission(
    state: &State,
    peer: EndpointId,
    version: u16,
    files_only: bool,
    services_only: bool,
    video_unsupported: bool,
    codec_supported: bool,
    peer_decode_unsupported: bool,
) -> Result<(), Error> {
    if services_only {
        // One link per paired peer; a newer one replaces the older.
        let other_links = state
            .incoming
            .values()
            .filter(|s| !finished(&s.phase) && s.services_only && s.peer_key != *peer.as_bytes())
            .count();
        return session::admit_services(
            state.hosting,
            other_links >= crate::trust::MAX_DEVICES,
            state.trust.get(peer.as_bytes()).is_some(),
            version,
        );
    }
    session::admit(
        state.hosting,
        state.incoming_admission_count() >= MAX_LIVE_INCOMING,
        state.trust.granted(peer.as_bytes(), Grant::View),
        state.trust.granted(peer.as_bytes(), Grant::Transfer),
        files_only,
        version,
        video_unsupported,
        codec_supported,
        peer_decode_unsupported,
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
            let _ = write_frame(
                &mut send,
                &ControlMessage::Reject(error_code(&Error::Malformed)),
            )
            .await;
            return HostTurn::PeerShouldClose;
        }
    };
    match message {
        ControlMessage::Hello {
            version,
            codec,
            quality,
            decode,
            files_only,
            services_only,
            separate_cursor: cursor_receiver,
            hostname: hello_hostname,
            os: hello_os,
        } => {
            // Hello carries a decode bit, not a profile bit; every decoder we
            // ship handles both profiles, so High is a host-encoder decision.
            let caps = lock(&shared.video_caps).clone();
            let agreed_codec = crate::video_caps::negotiate(codec, &caps);
            let codec_supported = agreed_codec.is_some();
            // The controller names its decoder in Hello. A peer that cannot
            // decode gets an explicit reject, not a session with no
            // displayable video.
            let peer_decode_unsupported = !decode && lock(&shared.capture).is_some();
            let decision = admission(
                &lock(&shared.state),
                peer,
                version,
                files_only,
                services_only,
                super::video_unsupported(&shared),
                codec_supported,
                peer_decode_unsupported,
            );
            let decision = match decision {
                // The file already told the controller who this host is. The
                // digits typed here tell the host who the controller is.
                // Without an open allow switch the reject stays immediate.
                Err(Error::Unauthorized | Error::NotPaired)
                    if lock(&shared.state).trust.get(peer.as_bytes()).is_none() =>
                {
                    match wait_for_control_approval(shared, connection, peer, stop_rx).await {
                        ControlAnswer::Approved => admission(
                            &lock(&shared.state),
                            peer,
                            version,
                            files_only,
                            services_only,
                            super::video_unsupported(&shared),
                            codec_supported,
                            // Re-read: the platform could report an encoder
                            // while the approval was parked.
                            !decode && lock(&shared.capture).is_some(),
                        ),
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
                    let decision = admission(
                        &lock(&shared.state),
                        peer,
                        version,
                        files_only,
                        services_only,
                        super::video_unsupported(&shared),
                        codec_supported,
                        !decode && lock(&shared.capture).is_some(),
                    );
                    if let Err(error) = decision {
                        let _ = write_frame(&mut send, &ControlMessage::Reject(error_code(&error)))
                            .await;
                        return HostTurn::PeerShouldClose;
                    }
                    // Agree the encoder limits before admitting: the offer
                    // clamped to this host's monitor and codec caps.
                    let monitor = monitor_size(shared);
                    let caps = lock(&shared.video_caps).clone();
                    let agreed = session::agree_quality(&quality, monitor, &caps);
                    let codec_id = agreed_codec
                        .map(|agreed| agreed.codec)
                        .unwrap_or(crate::protocol::CODEC_H264);
                    let separate_cursor = !files_only
                        && cursor_receiver
                        && lock(&shared.capture)
                            .as_ref()
                            .is_some_and(|capture| capture.separate_cursor());
                    match register_incoming(
                        shared,
                        peer,
                        stop,
                        Some(agreed),
                        codec_id,
                        separate_cursor,
                        files_only,
                        services_only,
                    ) {
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
                            let _ = super::link::remember_peer_hostname(
                                shared,
                                peer.as_bytes(),
                                hello_hostname,
                            );
                            let _ =
                                super::link::remember_peer_os(shared, peer.as_bytes(), hello_os);
                            let desktop_stop = lock(&shared.state)
                                .incoming
                                .get(&info.id)
                                .expect("admitted session")
                                .desktop_stop
                                .subscribe();
                            let cursor_task = separate_cursor.then(|| {
                                let mut desktop_stop = desktop_stop.clone();
                                let connection = connection.clone();
                                let cursor = lock(&shared.state)
                                    .incoming
                                    .get(&info.id)
                                    .expect("admitted session")
                                    .cursor_outbox
                                    .subscribe();
                                let stop = stop_rx.clone();
                                tokio::spawn(async move {
                                    if *desktop_stop.borrow() {
                                        return;
                                    }
                                    tokio::select! {
                                            biased;
                                            _ = desktop_stop.changed() => {},
                                            _ = super::cursor::send_cursor(
                                        connection,
                                        SessionStamp::new(info.id, info.generation),
                                        cursor,
                                        stop,
                                    ) => {},
                                        }
                                })
                            });
                            // Install reverse side-stream senders before Accept can
                            // make the controller's connect call return. In particular,
                            // a host file send immediately after connect must not race
                            // this registration.
                            let (clipboard, files) = spawn_host_streams(
                                shared,
                                iroh::endpoint::Connection::clone(connection),
                                info.id,
                                *peer.as_bytes(),
                                tokio::sync::watch::Receiver::clone(stop_rx),
                            );
                            let wrote = write_frame(
                                &mut send,
                                &ControlMessage::Accept {
                                    stamp: SessionStamp::new(info.id, info.generation),
                                    monitor_width: info.monitor_width,
                                    monitor_height: info.monitor_height,
                                    codec: codec_id
                                        | if separate_cursor {
                                            crate::protocol::SEPARATE_CURSOR
                                        } else {
                                            0
                                        },
                                    quality: info.quality.unwrap_or(agreed),
                                    hostname: crate::hostname::local_hostname(),
                                    os: crate::OperatingSystem::local(),
                                },
                            )
                            .await;
                            if wrote.is_err() || write_addresses(shared, &mut send).await.is_err() {
                                if let Some(task) = cursor_task {
                                    task.abort();
                                }
                                if let Some(task) = clipboard {
                                    task.abort();
                                }
                                files.abort();
                                finish_incoming(shared, info.id);
                                return HostTurn::PeerShouldClose;
                            }
                            let outbox = {
                                let state = lock(&shared.state);
                                state
                                    .incoming
                                    .get(&info.id)
                                    .and_then(|session| session.video.clone())
                            };
                            let video = outbox.map(|outbox| {
                                let connection = connection.clone();
                                let mut stop_rx = stop_rx.clone();
                                let mut desktop_stop = desktop_stop.clone();
                                tokio::spawn(async move {
                                    if *desktop_stop.borrow() {
                                        return;
                                    }
                                    tokio::select! {
                                        biased;
                                        _ = desktop_stop.changed() => {},
                                        _ = host_video_loop(connection, outbox, &mut stop_rx) => {},
                                    }
                                })
                            });
                            let local_stop = host_input_loop(
                                shared, info.id, peer, &mut send, &mut recv, stop_rx,
                            )
                            .await;
                            if let Some(task) = clipboard {
                                task.abort();
                            }
                            files.abort();
                            if let Some(task) = cursor_task {
                                task.abort();
                            }
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
    quality: Option<Quality>,
    codec: u8,
    separate_cursor: bool,
    files_only: bool,
    services_only: bool,
) -> Result<SessionInfo, Error> {
    let (monitor_width, monitor_height) = if services_only {
        (0, 0)
    } else {
        monitor_size(shared)
    };
    let mut state = lock(&shared.state);
    if state.shut_down {
        return Err(Error::ShutDown);
    }
    // Finished records are retained for status reads; drop them here so the
    // map holds only live sessions plus the latest finished one per id.
    state.incoming.retain(|_, s| !finished(&s.phase));
    if services_only {
        // A peer that dials again has lost its old link; end it.
        for old in state
            .incoming
            .values()
            .filter(|s| s.services_only && s.peer_key == *peer.as_bytes())
        {
            old.stop.send_replace(true);
        }
    } else if state.incoming_admission_count() >= MAX_LIVE_INCOMING {
        return Err(Error::Busy);
    }
    state.generation = state.generation.saturating_add(1);
    let id = state.next_session_id;
    state.next_session_id = state.next_session_id.saturating_add(1);
    let generation = state.generation;
    let peer_id = peer.to_string();
    let quality = if services_only { None } else { quality };
    let info = SessionInfo {
        id,
        generation,
        role: SessionRole::Host,
        phase: session::phase_after_admit(),
        peer_id: peer_id.clone(),
        quality,
        monitor_width,
        monitor_height,
    };
    // Links hold no admission slot: they are outside the live-session cap.
    let incoming_slot = (!services_only).then(|| {
        state
            .incoming_slots
            .fetch_add(1, std::sync::atomic::Ordering::AcqRel);
        Arc::new(super::input_cleanup::AdmissionSlot(
            state.incoming_slots.clone(),
        ))
    });
    state.incoming.insert(
        id,
        LiveSession {
            wire_stamp: crate::protocol::SessionStamp::new(id, generation),
            controller: None,
            id,
            generation,
            role: SessionRole::Host,
            phase: session::phase_after_admit(),
            peer_id: peer_id.clone(),
            peer_key: *peer.as_bytes(),
            quality,
            codec,
            separate_cursor: separate_cursor && !files_only,
            files_only,
            services_only,
            desktop_stop: tokio::sync::watch::channel(false).0,
            host_held: Arc::new(Mutex::new(crate::input::HeldInput::default())),
            incoming_slot,
            cursor_outbox: tokio::sync::watch::channel(None).0,
            input_epoch: Arc::new(AtomicU64::new(1)),
            desktop_owner: None,
            monitor_width,
            monitor_height,
            peer_topology: Vec::new(),
            selected_displays: Vec::new(),
            display_fault: None,
            stop: stop.clone(),
            outbox: None,
            video: if files_only {
                None
            } else {
                Some(Arc::new(VideoOutbox::new()))
            },
            link: None,
            clipboard: None,
            clipboard_files: None,
            published_files: None,
            published_rev: std::sync::atomic::AtomicU64::new(0),
            remote_availability: None,
        },
    );
    // Admission selects the new session for host clipboard sync.
    state.clipboard_owner = Some(id);
    drop(state);
    session::announce_progression(&shared.events, id, generation, SessionRole::Host, &peer_id);
    if services_only {
        // The peer just proved it is reachable; our own link to it dials now.
        super::presence::wake_link(shared, peer.as_bytes());
    }
    Ok(info)
}

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
    session.phase = SessionPhase::Closed;
    session.cursor_outbox.send_replace(None);
    session.incoming_slot = None;
    // Hand clipboard selection to the lowest remaining live hosted session,
    // if any, so sync never points at a closed session.
    if state.clipboard_owner == Some(session_id) {
        state.clipboard_owner = state
            .incoming
            .values()
            .filter(|s| !finished(&s.phase))
            .map(|s| s.id)
            .min();
    }
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

/// Clipboard is accepted before the filesystem loop. Two `accept_bi`
/// calls at once can take each other's stream, so each accept starts only
/// after the previous one has returned. Filesystem requests open their own
/// short streams afterwards and are accepted in a loop, so no gate is
/// needed past this point.
pub(super) fn spawn_host_streams(
    shared: &Arc<Shared>,
    connection: iroh::endpoint::Connection,
    session_id: u64,
    peer: [u8; 32],
    stop_rx: tokio::sync::watch::Receiver<bool>,
) -> (
    Option<tokio::task::JoinHandle<()>>,
    tokio::task::JoinHandle<()>,
) {
    if lock(&shared.state)
        .incoming
        .get(&session_id)
        .is_some_and(|s| s.services_only)
    {
        let shared = shared.clone();
        let aux = tokio::spawn(async move {
            super::fs::serve_aux_host(shared, connection, session_id, peer, stop_rx).await;
        });
        return (None, aux);
    }
    let (clipboard_tx, clipboard_rx) = tokio::sync::mpsc::channel(1);
    {
        let mut state = lock(&shared.state);
        if let Some(session) = state.incoming.get_mut(&session_id) {
            session.clipboard = Some(clipboard_tx);
        }
    }
    let (gate_tx, gate_rx) = tokio::sync::oneshot::channel();
    let clipboard_connection = iroh::endpoint::Connection::clone(&connection);
    let fs_connection = connection;
    let fs_stop = tokio::sync::watch::Receiver::clone(&stop_rx);
    let mut clipboard_stop = tokio::sync::watch::Receiver::clone(&stop_rx);
    let shared_apply = Arc::clone(shared);
    let shared_allow = Arc::clone(shared);
    let clipboard_task = tokio::spawn(async move {
        let opened = crate::sidestream::accept_bi(&clipboard_connection, &mut clipboard_stop).await;
        let _ = gate_tx.send(opened.is_ok());
        if let Ok((send, recv)) = opened {
            crate::clipboard::exchange(
                send,
                recv,
                clipboard_rx,
                clipboard_stop,
                move |payload| match payload {
                    ClipboardPayload::Text(text) => {
                        if !clipboard_selected(&lock(&shared_apply.state), session_id) {
                            return Err(Error::Busy);
                        }
                        apply_clipboard(&shared_apply, &peer, text)
                    }
                    ClipboardPayload::Files {
                        revision,
                        selection,
                    } => {
                        if !clipboard_selected(&lock(&shared_apply.state), session_id) {
                            return Err(Error::Busy);
                        }
                        apply_clipboard_files(
                            &shared_apply,
                            &peer,
                            session_id,
                            *revision,
                            selection,
                        )
                    }
                    ClipboardPayload::Prepare {
                        nonce,
                        revision,
                        selection,
                    } => spawn_paste_preparation(
                        &shared_apply,
                        true,
                        session_id,
                        peer,
                        *nonce,
                        *revision,
                        selection,
                    ),
                    ClipboardPayload::FilesReady { nonce, outcome } => {
                        deliver_paste_ready(&shared_apply, true, session_id, *nonce, outcome);
                        Ok(())
                    }
                },
                move || {
                    if lock(&shared_allow.state)
                        .trust
                        .granted(&peer, Grant::Clipboard)
                    {
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
    let shared_fs = Arc::clone(shared);
    let fs_task = tokio::spawn(async move {
        if !matches!(gate_rx.await, Ok(true)) {
            return;
        }
        super::fs::serve_aux_host(shared_fs, fs_connection, session_id, peer, fs_stop).await;
    });
    (Some(clipboard_task), fs_task)
}
