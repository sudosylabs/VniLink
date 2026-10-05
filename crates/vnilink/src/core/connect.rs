//! Authorize, connect, disconnect, and the controller dial loop.

use super::*;

use super::clipboard::open_controller_clipboard;
use super::files::open_controller_transfer;
use super::link::{remember_peer_addresses, write_addresses};
use super::video::{video_inbox, video_recv_loop};
use super::wire::{read_frame, read_session_frame, write_frame};

use std::sync::Arc;
use std::time::Duration;

use crate::address::Announcement;
use crate::error::Error;
use crate::events::CoreEvent;
use crate::input::{InputOutbox, Outbound};
use crate::protocol::{ControlMessage, KeyText, SessionStamp, ALPN, PROTOCOL_VERSION};
use crate::reconnect::{self, LinkEnd};
use crate::session::{Grants, SessionInfo, SessionPhase, SessionRole};
use crate::trust::Device;
use crate::video::KeyframeFlag;

impl Core {
    pub fn authorize(&self, endpoint_id: &str, grants: Grants) -> Result<(), Error> {
        let key = parse_endpoint_id(endpoint_id)?;
        {
            let mut state = self.alive_state()?;
            store_grants(&self.shared, &mut state, &key, grants)?;
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
            store_grants(&self.shared, &mut state, &key, Grants::default())?;
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

    /// Forgets a paired device entirely.
    ///
    /// Unlike [`Core::revoke`], which keeps the device with empty grants, the
    /// device is removed: later dials fail with [`Error::NotPaired`], and an
    /// unknown device reports that without touching any session. A live
    /// session with that peer ends on either role and releases held input
    /// like any local stop; the controller does not redial. This is local
    /// only and is not propagated to the peer. A failed save restores the
    /// device and reports the store error.
    pub fn unpair(&self, endpoint_id: &str) -> Result<(), Error> {
        let key = parse_endpoint_id(endpoint_id)?;
        let mut incoming_stop = None;
        let mut outgoing_stop = None;
        {
            let mut state = self.alive_state()?;
            forget_device(&self.shared, &mut state, &key)?;
            if state
                .incoming
                .as_ref()
                .is_some_and(|session| session.peer_key == key && !finished(&session.phase))
            {
                state.generation = state.generation.saturating_add(1);
                if let Some(session) = state.incoming.as_mut() {
                    session.phase = SessionPhase::Stopping;
                    incoming_stop = Some(session.stop.clone());
                }
            }
            if state
                .outgoing
                .as_ref()
                .is_some_and(|session| session.peer_key == key && !finished(&session.phase))
            {
                if let Some(session) = state.outgoing.as_mut() {
                    session.phase = SessionPhase::Stopping;
                    outgoing_stop = Some(session.stop.clone());
                }
            }
        }
        if let Some(stop) = incoming_stop {
            let _ = stop.send(true);
        }
        if let Some(stop) = outgoing_stop {
            let _ = stop.send(true);
        }
        self.shared.events.publish(CoreEvent::DeviceUnpaired {
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
        // The controller must decode what the host encodes. Fail here with an
        // explicit unsupported error instead of connecting to a black screen.
        if !self
            .shared
            .h264_decode
            .load(std::sync::atomic::Ordering::Relaxed)
        {
            return Err(Error::Unsupported("no H.264 decoder is available"));
        }
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
                quality: Some(crate::session::controller_offer()),
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
}

pub(super) async fn run_controller(
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

pub(super) async fn establish(
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
    let offer = crate::session::controller_offer();
    let decode = shared
        .h264_decode
        .load(std::sync::atomic::Ordering::Relaxed);
    write_frame(
        &mut send,
        &ControlMessage::Hello {
            version: PROTOCOL_VERSION,
            codec: crate::protocol::CODEC_H264,
            quality: offer,
            decode,
            hostname: crate::hostname::local_hostname(),
        },
    )
    .await?;
    let response = read_frame(&mut recv).await?;
    let info = match response {
        ControlMessage::Accept {
            stamp,
            monitor_width,
            monitor_height,
            codec,
            quality,
            hostname: accept_hostname,
        } => {
            if codec != crate::protocol::CODEC_H264 {
                return Err(Error::IncompatibleVersion);
            }
            let peer_id = device.display_id.clone();
            // The host admitted us: keep its display name for this
            // controller's device list. Persist failures stay local; the
            // next dial refreshes the name.
            let _ =
                super::link::remember_peer_hostname(shared, &device.endpoint_id, accept_hostname);
            {
                let mut state = lock(&shared.state);
                if let Some(session) = state.outgoing.as_mut() {
                    session.id = stamp.id;
                    session.generation = stamp.generation;
                    session.phase = SessionPhase::Active;
                    session.monitor_width = monitor_width;
                    session.monitor_height = monitor_height;
                    session.link = Some(connection.clone());
                    session.quality = Some(quality);
                }
            }
            crate::session::announce_progression(
                &shared.events,
                stamp.id,
                stamp.generation,
                SessionRole::Controller,
                &peer_id,
            );
            SessionInfo {
                id: stamp.id,
                generation: stamp.generation,
                role: SessionRole::Controller,
                phase: SessionPhase::Active,
                peer_id,
                quality: Some(quality),
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
        SessionStamp::new(session_id, generation),
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

pub(super) async fn command_loop(
    connection: iroh::endpoint::Connection,
    mut send: iroh::endpoint::SendStream,
    mut recv: iroh::endpoint::RecvStream,
    outbox: Arc<InputOutbox>,
    stamp: SessionStamp,
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
            let wrote = write_frame(&mut send, &ControlMessage::Keyframe { stamp }).await;
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
                        Some(Ok(ControlMessage::Quality {
                            stamp: msg_stamp,
                            quality,
                        })) => {
                            if msg_stamp != stamp {
                                break;
                            }
                            {
                                let mut state = lock(&shared.state);
                                if let Some(session) = state.outgoing.as_mut() {
                                    if session.id == stamp.id
                                        && session.phase == SessionPhase::Active
                                    {
                                        session.quality = Some(quality);
                                    }
                                }
                            }
                            shared.events.publish(CoreEvent::QualityUpdated {
                                id: stamp.id,
                                quality,
                            });
                        }
                        Some(Ok(ControlMessage::Reject(code))) => {
                            let error = code.to_error();
                            outbox.fail(&error);
                            shared.events.publish(CoreEvent::InputFailed {
                                id: stamp.id,
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
                    Outbound::Event(event) => ControlMessage::Input { stamp, event },
                    Outbound::Text(text) => ControlMessage::Text {
                        stamp,
                        text: KeyText(text),
                    },
                    Outbound::Chord(text) => ControlMessage::Chord {
                        stamp,
                        text: KeyText(text),
                    },
                    Outbound::Quality(quality) => ControlMessage::Quality { stamp, quality },
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

pub(super) fn finish_outgoing(shared: &Shared) {
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
    settle_outgoing(shared, &info, SessionPhase::Closed);
}

pub(super) fn fail_outgoing(shared: &Shared, error: &Error) {
    let mut state = lock(&shared.state);
    let Some(session) = state.outgoing.as_mut() else {
        return;
    };
    let info = session_info(session);
    session.phase = SessionPhase::Failed(error.to_string());
    drop(state);
    settle_outgoing(shared, &info, SessionPhase::Failed(error.to_string()));
}

pub(super) fn mark_reconnecting(shared: &Shared) {
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
    settle_outgoing(shared, &info, SessionPhase::Reconnecting);
}

/// Clears the clipboard inbox and emits the terminal phase. The guards and
/// the phase assignment stay with each caller: only a finished session
/// closes, while a failure records even over one.
fn settle_outgoing(shared: &Shared, info: &SessionInfo, phase: SessionPhase) {
    *lock(&shared.clipboard_inbox) = None;
    shared.events.publish(CoreEvent::SessionChanged {
        id: info.id,
        generation: info.generation,
        role: info.role,
        phase,
        peer_id: info.peer_id.clone(),
    });
}
