//! Authorize, connect, disconnect, and the controller dial loop.

use super::*;

use super::clipboard::open_controller_clipboard;
use super::link::{remember_peer_addresses, write_addresses};
use super::transfer::close_peer_files;
use super::video::{video_inbox, video_recv_loop};
use super::wire::{read_frame_within, read_session_frame, write_frame};

use std::sync::Arc;
use std::time::Duration;

use crate::address::Announcement;
use crate::error::Error;
use crate::events::CoreEvent;
use crate::input::{InputOutbox, Outbound};
use crate::protocol::{ControlMessage, KeyText, SessionStamp, ALPN};
use crate::reconnect::{self, LinkEnd};
use crate::session::{Grants, SessionInfo, SessionPhase, SessionRole};
use crate::trust::Device;
use crate::video::KeyframeFlag;

/// Live controller-side connections, across all peers. Mirrors the host's
/// `MAX_LIVE_INCOMING`: each outgoing session owns dial tasks, reconnect
/// state and per-peer channels, so fan-out is bounded and the fifth peer
/// gets a structured `Busy` instead of silent degradation.
const MAX_LIVE_OUTGOING: usize = 4;

/// Dropping a cancelled establish future must stop its side-stream workers.
#[derive(Default)]
struct ControllerTasks(Vec<tokio::task::AbortHandle>);
impl Drop for ControllerTasks {
    fn drop(&mut self) {
        for task in &self.0 {
            task.abort();
        }
    }
}

impl Core {
    pub fn set_desktop_sharing(&self, grants: Grants) -> Result<(), Error> {
        self.update_sharing_policy(|policy| {
            policy.view = grants.view;
            policy.control = grants.control;
            policy.clipboard = grants.clipboard;
            policy.files = grants.transfer;
        })
    }

    pub fn revoke(&self, endpoint_id: &str) -> Result<(), Error> {
        self.unpair(endpoint_id)
    }

    /// Forgets a paired device entirely.
    ///
    /// The device is removed: later dials fail with [`Error::NotPaired`], and an
    /// unknown device reports that without touching any session. A live
    /// session with that peer ends on either role and releases held input
    /// like any local stop; the controller does not redial. This is local
    /// only and is not propagated to the peer. A failed save restores the
    /// device and reports the store error.
    pub fn unpair(&self, endpoint_id: &str) -> Result<(), Error> {
        let key = parse_endpoint_id(endpoint_id)?;
        let mut incoming_stop = Vec::new();
        let mut outgoing_stop = None;
        {
            let mut state = self.alive_state()?;
            forget_device(&self.shared, &mut state, &key)?;
            let live: Vec<u64> = state
                .incoming
                .values()
                .filter(|session| session.peer_key == key && !finished(&session.phase))
                .map(|session| session.id)
                .collect();
            if !live.is_empty() {
                state.generation = state.generation.saturating_add(1);
                for id in live {
                    if let Some(session) = state.incoming.get_mut(&id) {
                        session.phase = SessionPhase::Stopping;
                        incoming_stop.push(session.stop.clone());
                    }
                }
            }
            for session in state
                .outgoing
                .values_mut()
                .filter(|s| s.peer_key == key && !finished(&s.phase))
            {
                session.phase = SessionPhase::Stopping;
                outgoing_stop = Some(session.stop.clone());
            }
            super::services::synchronize(&mut state, &self.shared.events);
        }
        for stop in incoming_stop {
            let _ = stop.send(true);
        }
        if let Some(stop) = outgoing_stop {
            let _ = stop.send(true);
        }
        close_peer_files(&self.shared, &key);
        super::jobs::interrupt_peer_jobs(&self.shared, &key);
        self.shared.events.publish(CoreEvent::DeviceUnpaired {
            endpoint_id: endpoint_id.to_owned(),
        });
        Ok(())
    }

    pub fn connect(&self, endpoint_id: &str) -> Result<SessionInfo, Error> {
        self.connect_inner(endpoint_id, false)
    }

    /// Establishes a control session for file roots/listings/transfers only,
    /// without requiring a video decoder or host screen sharing. Admission on
    /// the host needs the Transfer grant (Control not required); file
    /// operations enforce Transfer per request. Reuses the single outgoing
    /// slot: fails `Busy` while another control session is active, like
    /// `connect` (open Files from the live desktop session to reuse it).
    pub fn connect_files(&self, endpoint_id: &str) -> Result<SessionInfo, Error> {
        self.connect_inner(endpoint_id, true)
    }

    fn connect_inner(&self, endpoint_id: &str, files_only: bool) -> Result<SessionInfo, Error> {
        // Compatibility for native clients that still expose one viewer.
        let (pending, ready, stop) = self.start_controller(endpoint_id, files_only, false, true)?;
        match ready.recv_timeout(CONNECT_WAIT) {
            Ok(result) => result,
            Err(_) => {
                let _ = stop.send(true);
                let _ = self.disconnect_session(pending.id);
                Err(Error::Timeout)
            }
        }
    }

    /// Starts one independent controller connection without waiting for approval.
    /// The returned local id remains stable across reconnects. Observe its phase
    /// through controller_session/controller_sessions or SessionChanged events.
    pub fn begin_connect(&self, endpoint_id: &str) -> Result<SessionInfo, Error> {
        self.start_controller(endpoint_id, false, false, false)
            .map(|(info, _, _)| info)
    }

    pub fn begin_connect_files(&self, endpoint_id: &str) -> Result<SessionInfo, Error> {
        self.start_controller(endpoint_id, true, false, false)
            .map(|(info, _, _)| info)
    }

    /// Starts a paired, feature-neutral connection. No approval of desktop,
    /// file transfer, camera or terminal is implied by connection admission.
    /// Reuses this peer's live neutral connection, including pending admission
    /// and reconnect. This does not open or replace any service owner. A live
    /// legacy connection still returns Busy; no privileged fallback is used.
    pub fn begin_connect_services(&self, endpoint_id: &str) -> Result<SessionInfo, Error> {
        let info = self
            .start_controller(endpoint_id, true, true, false)
            .map(|(info, _, _)| info)?;
        // A link waiting out its backoff dials now.
        super::presence::wake_link(&self.shared, &parse_endpoint_id(endpoint_id)?);
        Ok(info)
    }

    fn start_controller(
        &self,
        endpoint_id: &str,
        files_only: bool,
        services_only: bool,
        legacy: bool,
    ) -> Result<
        (
            SessionInfo,
            std::sync::mpsc::Receiver<Result<SessionInfo, Error>>,
            tokio::sync::watch::Sender<bool>,
        ),
        Error,
    > {
        start_outgoing(
            &self.shared,
            self.runtime.handle(),
            endpoint_id,
            files_only,
            services_only,
            legacy,
        )
    }

    pub fn controller_sessions(&self) -> Result<Vec<SessionInfo>, Error> {
        let state = self.alive_state()?;
        let mut sessions: Vec<_> = state.outgoing.values().map(session_info).collect();
        sessions.sort_by_key(|s| s.id);
        Ok(sessions)
    }

    pub fn controller_session(&self, id: u64) -> Result<SessionInfo, Error> {
        let state = self.alive_state()?;
        state
            .outgoing
            .get(&id)
            .map(session_info)
            .ok_or(Error::NoSession)
    }

    /// Stops only this local controller handle, including a pending approval.
    pub fn disconnect_session(&self, id: u64) -> Result<(), Error> {
        let mut state = self.alive_state()?;
        let session = state.outgoing.get_mut(&id).ok_or(Error::NoSession)?;
        if finished(&session.phase) {
            return Err(Error::NoSession);
        }
        session.phase = SessionPhase::Stopping;
        let _ = session.stop.send(true);
        super::services::synchronize(&mut state, &self.shared.events);
        Ok(())
    }

    /// Releases a closed/failed tab's record and buffers. Never stops live work.
    pub fn forget_controller_session(&self, id: u64) -> Result<(), Error> {
        let mut state = self.alive_state()?;
        let session = state.outgoing.get(&id).ok_or(Error::NoSession)?;
        if !finished(&session.phase) {
            return Err(Error::Busy);
        }
        state.outgoing.remove(&id);
        Ok(())
    }

    /// Legacy disconnect is refused when more than one outgoing session is live.
    pub fn disconnect(&self) -> Result<(), Error> {
        let id = {
            let state = self.alive_state()?;
            if state.live_outgoing_count() > 1 {
                return Err(Error::Busy);
            }
            state.single_outgoing().ok_or(Error::NoSession)?.id
        };
        self.disconnect_session(id)
    }

    pub(super) fn controller_state(
        &self,
        id: u64,
        generation: u64,
    ) -> Result<Arc<ControllerState>, Error> {
        let state = self.alive_state()?;
        let session = live_session(&state, false, id)?;
        if session.generation != generation {
            return Err(Error::StaleSession);
        }
        session.controller.clone().ok_or(Error::NoSession)
    }

    #[cfg(test)]
    pub(crate) fn drop_controller_link(&self, id: u64) -> Result<(), Error> {
        let link = self
            .alive_state()?
            .outgoing
            .get(&id)
            .and_then(|s| s.link.clone())
            .ok_or(Error::NoSession)?;
        link.close(1u8.into(), b"test dropped controller");
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
                .single_outgoing()
                .and_then(|session| session.link.clone())
                .or_else(|| {
                    state
                        .incoming_live()
                        .and_then(|session| session.link.clone())
                })
                .ok_or(Error::NoSession)?
        };
        link.close(1u8.into(), b"dropped");
        Ok(())
    }
}

pub(super) fn start_outgoing(
    shared: &Arc<Shared>,
    runtime: &tokio::runtime::Handle,
    endpoint_id: &str,
    files_only: bool,
    services_only: bool,
    legacy: bool,
) -> Result<
    (
        SessionInfo,
        std::sync::mpsc::Receiver<Result<SessionInfo, Error>>,
        tokio::sync::watch::Sender<bool>,
    ),
    Error,
> {
    let key = parse_endpoint_id(endpoint_id)?;
    let (stop, stop_rx) = tokio::sync::watch::channel(false);
    let outbox = Arc::new(InputOutbox::new());
    // Gate and reserve in one critical section: concurrent connects cannot
    // overwrite each other or open two control sessions to the same peer.
    let (pending, peer_key) =
        {
            let mut state = lock(&shared.state);
            if state.shut_down {
                return Err(Error::ShutDown);
            }
            let device = state.trust.get(&key).cloned().ok_or(Error::NotPaired)?;
            // A neutral link and a legacy session to the same peer coexist;
            // each kind has at most one live record per peer.
            if let Some(existing) = state.outgoing.values().find(|s| {
                s.peer_key == key && !finished(&s.phase) && s.services_only == services_only
            }) {
                if services_only && existing.phase != SessionPhase::Stopping {
                    let info = session_info(existing);
                    let (ready, result) = std::sync::mpsc::sync_channel(1);
                    let _ = ready.send(Ok(info.clone()));
                    return Ok((info, result, existing.stop.clone()));
                }
                return Err(Error::Busy);
            }
            if legacy
                && state
                    .outgoing
                    .values()
                    .any(|s| !finished(&s.phase) && !s.services_only)
            {
                return Err(Error::Busy);
            }
            // Links are bounded by the paired devices; other sessions by the live cap.
            let live = |links: bool| {
                state
                    .outgoing
                    .values()
                    .filter(|s| !finished(&s.phase) && s.services_only == links)
                    .count()
            };
            if live(services_only)
                >= if services_only {
                    crate::trust::MAX_DEVICES
                } else {
                    MAX_LIVE_OUTGOING
                }
            {
                return Err(Error::Busy);
            }
            if !files_only && !lock(&shared.video_caps).can_decode() {
                return Err(Error::Unsupported("no H.264 decoder is available"));
            }
            let id = state.next_session_id;
            state.next_session_id = id.checked_add(1).ok_or(Error::Busy)?;
            let inbox = if legacy {
                video_inbox(shared)
            } else {
                Arc::new(crate::video::VideoInbox::new())
            };
            let controller = Arc::new(ControllerState::new(
                inbox,
                shared
                    .keyboard_mode
                    .load(std::sync::atomic::Ordering::Relaxed),
                shared
                    .swap_control_command
                    .load(std::sync::atomic::Ordering::Relaxed),
                *lock(&shared.pointer_defaults),
            ));
            if legacy {
                *lock(&controller.subscriber) = lock(&shared.video_subscriber).clone();
            }
            let session = LiveSession {
                id,
                generation: 0,
                wire_stamp: SessionStamp::new(0, 0),
                controller: Some(controller),
                role: SessionRole::Controller,
                phase: SessionPhase::Connecting,
                peer_id: device.display_id.clone(),
                peer_key: key,
                quality: if services_only {
                    None
                } else {
                    Some(state.desktop_quality_default)
                },
                codec: crate::protocol::CODEC_H264,
                separate_cursor: false,
                files_only,
                services_only,
                desktop_stop: tokio::sync::watch::channel(false).0,
                host_held: Arc::new(Mutex::new(crate::input::HeldInput::default())),
                incoming_slot: None,
                cursor_outbox: tokio::sync::watch::channel(None).0,
                input_epoch: Arc::new(AtomicU64::new(1)),
                desktop_owner: None,
                monitor_width: 0,
                monitor_height: 0,
                peer_topology: Vec::new(),
                selected_displays: Vec::new(),
                display_fault: None,
                stop: stop.clone(),
                outbox: Some(outbox.clone()),
                video: None,
                link: None,
                clipboard: None,
                clipboard_files: None,
                published_files: None,
                published_rev: std::sync::atomic::AtomicU64::new(0),
                remote_availability: None,
            };
            let info = session_info(&session);
            state.outgoing.insert(id, session);
            (info, key)
        };
    shared.events.publish(CoreEvent::SessionChanged {
        id: pending.id,
        generation: 0,
        role: SessionRole::Controller,
        phase: SessionPhase::Connecting,
        peer_id: pending.peer_id.clone(),
    });
    let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel(1);
    let shared = Arc::clone(shared);
    let id = pending.id;
    runtime.spawn(async move {
        run_controller(
            shared,
            id,
            peer_key,
            stop_rx,
            outbox,
            ready_tx,
            files_only,
            services_only,
        )
        .await;
    });
    Ok((pending, ready_rx, stop))
}

pub(super) async fn run_controller(
    shared: Arc<Shared>,
    local_id: u64,
    peer_key: [u8; 32],
    mut stop_rx: tokio::sync::watch::Receiver<bool>,
    outbox: Arc<InputOutbox>,
    ready_tx: std::sync::mpsc::SyncSender<Result<SessionInfo, Error>>,
    files_only: bool,
    services_only: bool,
) {
    let mut ready = Some(ready_tx);
    let mut drops = 0u32;
    loop {
        if *stop_rx.borrow() || lock(&shared.state).shut_down {
            finish_outgoing(&shared, local_id);
            if let Some(ready) = ready.take() {
                let _ = ready.send(Err(Error::NoSession));
            }
            return;
        }
        if drops > 0 {
            mark_reconnecting(&shared, local_id);
            outbox.clear();
            tokio::select! {
                _ = tokio::time::sleep(reconnect::PAUSE) => {}
                _ = stop_rx.changed() => {}
            }
            if *stop_rx.borrow() || lock(&shared.state).shut_down {
                finish_outgoing(&shared, local_id);
                return;
            }
        }
        let device = match load_device(&shared, &peer_key) {
            Ok(device) => device,
            Err(error) => {
                fail_outgoing(&shared, local_id, &error);
                if let Some(ready) = ready.take() {
                    let _ = ready.send(Err(error));
                }
                return;
            }
        };
        let mut dial_stop = stop_rx.clone();
        let attempt = tokio::select! {
            result = establish(&shared, local_id, device, &mut dial_stop, &outbox, &mut ready, files_only, services_only) => result,
            _ = stop_rx.changed() => Err(Error::NoSession),
        };
        if *stop_rx.borrow() || lock(&shared.state).shut_down {
            finish_outgoing(&shared, local_id);
            if let Some(ready) = ready.take() {
                let _ = ready.send(Err(Error::NoSession));
            }
            return;
        }
        match attempt {
            Ok((lasted, LinkEnd::Dropped)) => {
                if lasted {
                    drops = 1;
                } else {
                    drops += 1;
                }
                if drops > reconnect::ATTEMPTS {
                    fail_outgoing(&shared, local_id, &Error::Timeout);
                    return;
                }
            }
            Ok((_, LinkEnd::LocalStop | LinkEnd::RemoteClose)) => {
                finish_outgoing(&shared, local_id);
                return;
            }
            Err(error) if ready.is_none() && reconnect::retries(&error) => {
                drops += 1;
                if drops > reconnect::ATTEMPTS {
                    fail_outgoing(&shared, local_id, &error);
                    return;
                }
            }
            Err(error) => {
                fail_outgoing(&shared, local_id, &error);
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
    local_id: u64,
    device: Device,
    stop_rx: &mut tokio::sync::watch::Receiver<bool>,
    outbox: &Arc<InputOutbox>,
    ready: &mut Option<std::sync::mpsc::SyncSender<Result<SessionInfo, Error>>>,
    files_only: bool,
    services_only: bool,
) -> Result<(bool, LinkEnd), Error> {
    let mut tasks = ControllerTasks::default();
    // Retiring an adopted desktop is permanent for this connection. A later
    // redial must preserve Files without silently starting desktop capture.
    let retired = lock(&shared.state)
        .outgoing
        .get(&local_id)
        .is_some_and(|s| *s.desktop_stop.borrow());
    let files_only = files_only || retired;
    let services_only = services_only || retired;
    let addr = device_addr(&device)?;
    let offer = lock(&shared.state).desktop_quality_default;
    let codec_preference = lock(&shared.state).desktop_codec_default;
    // Neither non-desktop mode needs a decoder; desktop offers the decoder
    // mask, or only the codec the owner selected.
    let (decode, codec) = if files_only {
        (false, 0x40 | crate::VideoCodec::H264.offer_bit())
    } else {
        let caps = lock(&shared.video_caps).clone();
        (caps.can_decode(), caps.preferred_offer(codec_preference)?)
    };
    let cursor_receiver = !files_only
        && shared
            .cursor_receiver
            .load(std::sync::atomic::Ordering::Acquire);
    let (connection, mut send, recv, info) = {
        let connect = shared.endpoint.connect(addr.clone(), ALPN);
        let connection = tokio::time::timeout(Duration::from_secs(10), connect)
            .await
            .map_err(|_| Error::Timeout)?
            .map_err(Error::network_debug)?;
        {
            let mut state = lock(&shared.state);
            let session = state.outgoing.get_mut(&local_id).ok_or(Error::NoSession)?;
            if *stop_rx.borrow() || matches!(session.phase, SessionPhase::Stopping) {
                connection.close(0u8.into(), b"cancelled");
                return Err(Error::NoSession);
            }
            session.link = Some(connection.clone());
        }
        let (mut send, mut recv) = connection.open_bi().await.map_err(Error::network_debug)?;
        write_frame(
            &mut send,
            &ControlMessage::Hello {
                version: crate::protocol::PROTOCOL_VERSION,
                codec,
                quality: offer,
                decode,
                files_only,
                services_only,
                separate_cursor: cursor_receiver,
                hostname: crate::hostname::local_hostname(),
                os: crate::OperatingSystem::local(),
            },
        )
        .await?;
        match read_frame_within(&mut recv, Some(super::HELLO_ANSWER_WAIT)).await? {
            ControlMessage::Accept {
                stamp,
                monitor_width,
                monitor_height,
                codec: agreed,
                quality,
                hostname: accept_hostname,
                os: accept_os,
            } => {
                let separate_cursor = agreed & crate::protocol::SEPARATE_CURSOR != 0;
                let agreed = agreed & !crate::protocol::SEPARATE_CURSOR;
                let common = Error::Unsupported("no video codec both computers support");
                if !files_only
                    && codec_preference
                        .is_some_and(|c| crate::VideoCodec::from_wire_id(agreed) != Some(c))
                {
                    return Err(common);
                }
                if !files_only
                    && !lock(&shared.video_caps)
                        .supports_decode(crate::VideoCodec::from_wire_id(agreed).ok_or(common)?)
                {
                    return Err(Error::Unsupported("no video codec both computers support"));
                }
                let peer_id = device.display_id.clone();
                // The host admitted us: keep its display name for this
                // controller's device list. Persist failures stay local; the
                // next dial refreshes the name.
                let _ = super::link::remember_peer_hostname(
                    shared,
                    &device.endpoint_id,
                    accept_hostname,
                );
                let _ = super::link::remember_peer_os(shared, &device.endpoint_id, accept_os);
                {
                    let mut state = lock(&shared.state);
                    if let Some(session) = state.outgoing.get_mut(&local_id) {
                        if *stop_rx.borrow() || matches!(session.phase, SessionPhase::Stopping) {
                            return Err(Error::NoSession);
                        }
                        if *session.desktop_stop.borrow() && !services_only {
                            // Close raced an in-flight desktop Hello. Discard
                            // that admission and retry as neutral; never publish
                            // Active or resurrect media from the retired owner.
                            connection.close(0u8.into(), b"desktop retired");
                            return Err(Error::network("desktop retired during admission"));
                        }
                        if session.generation > 0 {
                            let previous = session.controller.as_ref().ok_or(Error::NoSession)?;
                            let pointer = lock(&previous.pointer).settings;
                            session.controller = Some(Arc::new(ControllerState::new(
                                Arc::new(crate::video::VideoInbox::new()),
                                previous.mode.load(std::sync::atomic::Ordering::Relaxed),
                                previous.swap.load(std::sync::atomic::Ordering::Relaxed),
                                pointer,
                            )));
                        }
                        session.wire_stamp = stamp;
                        session.files_only = files_only;
                        session.services_only = services_only;
                        session.desktop_owner = None;
                        session.generation =
                            session.generation.saturating_add(1).max(stamp.generation);
                        session.monitor_width = monitor_width;
                        session.monitor_height = monitor_height;
                        session.link = Some(connection.clone());
                        session.quality = if services_only { None } else { Some(quality) };
                        session.codec = agreed;
                        session.separate_cursor = separate_cursor;
                    }
                }
                (
                    connection,
                    send,
                    recv,
                    SessionInfo {
                        id: local_id,
                        generation: lock(&shared.state)
                            .outgoing
                            .get(&local_id)
                            .ok_or(Error::NoSession)?
                            .generation,
                        role: SessionRole::Controller,
                        phase: SessionPhase::Active,
                        peer_id,
                        quality: if services_only { None } else { Some(quality) },
                        monitor_width,
                        monitor_height,
                    },
                )
            }
            ControlMessage::Reject(code) => return Err(code.to_error()),
            _ => return Err(Error::Malformed),
        }
    };
    if write_addresses(shared, &mut send).await.is_err() {
        return Err(Error::network("the address announcement was not written"));
    }
    let clipboard = if services_only {
        None
    } else {
        Some(
            match open_controller_clipboard(shared, &connection, info.id, stop_rx).await {
                Ok(clipboard) => clipboard,
                Err(error) => return Err(error),
            },
        )
    };
    if let Some(task) = &clipboard {
        tasks.0.push(task.abort_handle());
    }
    let controller = lock(&shared.state)
        .outgoing
        .get(&local_id)
        .and_then(|s| s.controller.clone())
        .ok_or(Error::NoSession)?;
    let inbox = controller.inbox.clone();
    inbox.reset();
    // A reconnect must not expose Active with senders from the previous
    // connection. Both side streams are usable before the UI can act on it.
    {
        let mut state = lock(&shared.state);
        let stopped = state.shut_down || *stop_rx.borrow();
        let Some(session) = state.outgoing.get_mut(&local_id).filter(|s| {
            s.id == info.id
                && s.generation == info.generation
                && !matches!(s.phase, SessionPhase::Stopping)
                && !finished(&s.phase)
        }) else {
            if let Some(task) = &clipboard {
                task.abort();
            }
            return Err(Error::NoSession);
        };
        if stopped {
            if let Some(task) = &clipboard {
                task.abort();
            }
            return Err(Error::NoSession);
        }
        session.phase = SessionPhase::Active;
        super::services::synchronize(&mut state, &shared.events);
    }
    crate::session::announce_progression(
        &shared.events,
        info.id,
        info.generation,
        SessionRole::Controller,
        &info.peer_id,
    );
    if let Some(ready) = ready.take() {
        let _ = ready.send(Ok(info.clone()));
    }
    let keyframe = Arc::new(KeyframeFlag::new());
    let mut video_stop = stop_rx.clone();
    let video_connection = connection.clone();
    let video_inbox = Arc::clone(&inbox);
    let video_keyframe = Arc::clone(&keyframe);
    let mut desktop_stop = lock(&shared.state)
        .outgoing
        .get(&local_id)
        .ok_or(Error::NoSession)?
        .desktop_stop
        .subscribe();
    let cursor = {
        let state = lock(&shared.state);
        state
            .outgoing
            .get(&local_id)
            .filter(|s| s.separate_cursor)
            .map(|s| (s.wire_stamp, Arc::clone(&controller.cursor)))
    };
    let expected_codec = lock(&shared.state)
        .outgoing
        .get(&local_id)
        .ok_or(Error::NoSession)?
        .codec;
    let reader = tokio::spawn(async move {
        if files_only {
            return;
        }
        if *desktop_stop.borrow() {
            return;
        }
        tokio::select! {
        biased;
        _ = desktop_stop.changed() => {},
        _ = video_recv_loop(
            video_connection,
            video_inbox,
            video_keyframe,
            &mut video_stop,
            cursor,
            expected_codec,
        ) => {},
        }
    });
    tasks.0.push(reader.abort_handle());
    // Host-initiated job streams (status queries for host senders) are
    // accepted here; the controller never accepts anything else.
    let aux_stop = stop_rx.clone();
    let aux_connection = connection.clone();
    let aux_shared = Arc::clone(&shared);
    let aux_peer = device.endpoint_id;
    let aux = tokio::spawn(async move {
        super::fs::serve_aux_controller(aux_shared, aux_connection, aux_peer, aux_stop).await;
    });
    tasks.0.push(aux.abort_handle());
    let started = std::time::Instant::now();
    let wire_stamp = lock(&shared.state)
        .outgoing
        .get(&local_id)
        .ok_or(Error::NoSession)?
        .wire_stamp;
    let end = command_loop(
        connection,
        send,
        recv,
        Arc::clone(outbox),
        wire_stamp,
        local_id,
        device.endpoint_id,
        keyframe,
        shared,
        stop_rx,
    )
    .await;
    reader.abort();
    aux.abort();
    if let Some(task) = &clipboard {
        task.abort();
    }
    Ok((started.elapsed() > reconnect::PAUSE, end))
}

pub(super) async fn command_loop(
    connection: iroh::endpoint::Connection,
    mut send: iroh::endpoint::SendStream,
    mut recv: iroh::endpoint::RecvStream,
    outbox: Arc<InputOutbox>,
    stamp: SessionStamp,
    local_id: u64,
    peer_key: [u8; 32],
    keyframe: Arc<KeyframeFlag>,
    shared: &Shared,
    stop_rx: &mut tokio::sync::watch::Receiver<bool>,
) -> LinkEnd {
    let (frames_tx, mut frames_rx) = tokio::sync::mpsc::channel(8);
    let reader = tokio::spawn(async move {
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
    let _reader = ControllerTasks(vec![reader.abort_handle()]);
    let mut remote_close = false;
    loop {
        if *stop_rx.borrow() {
            break;
        }
        if keyframe.take()
            && !lock(&shared.state)
                .outgoing
                .get(&local_id)
                .is_none_or(|s| *s.desktop_stop.borrow())
        {
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
                        Some(Ok(ControlMessage::Availability { entries })) => {
                            let peer_id = {
                                let mut state = lock(&shared.state);
                                state.outgoing.get_mut(&local_id).filter(|s| s.services_only).map(|session| {
                                    session.remote_availability = Some(entries);
                                    session.peer_id.clone()
                                })
                            };
                            if let Some(endpoint_id) = peer_id {
                                shared
                                    .events
                                    .publish(CoreEvent::PeerAvailabilityChanged { endpoint_id });
                            }
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
                                if let Some(session) = state.outgoing.get_mut(&local_id) {
                                    if session.wire_stamp == stamp
                                        && session.phase == SessionPhase::Active
                                        && !*session.desktop_stop.borrow()
                                    {
                                        session.quality = Some(quality);
                                    }
                                }
                            }
                            shared.events.publish(CoreEvent::QualityUpdated {
                                id: local_id,
                                quality,
                            });
                        }
                        Some(Ok(ControlMessage::Reject(code))) => {
                            let error = code.to_error();
                            outbox.fail(&error);
                            shared.events.publish(CoreEvent::InputFailed {
                                id: local_id,
                                message: crate::input::InputFault::from_error(&error).message(),
                            });
                            if !matches!(error, Error::Unsupported(_)) {
                                break;
                            }
                        }
                        Some(Ok(ControlMessage::Topology {
                            stamp: msg_stamp,
                            revision,
                            displays,
                        })) => {
                            if msg_stamp != stamp {
                                break;
                            }
                            super::topology::store_peer_topology(
                                shared,
                                local_id,
                                stamp,
                                revision,
                                displays,
                            );
                        }
                        Some(Ok(_)) | Some(Err(_)) | None => break,
                    }
                }
                _ = connection.closed() => break,
            }
        } else {
            let mut failed = false;
            for item in batch {
                if lock(&shared.state)
                    .outgoing
                    .get(&local_id)
                    .is_none_or(|s| *s.desktop_stop.borrow())
                {
                    break;
                }
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
                    Outbound::Displays(displays) => {
                        ControlMessage::SelectDisplays { stamp, displays }
                    }
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

pub(super) fn finish_outgoing(shared: &Shared, local_id: u64) {
    let mut state = lock(&shared.state);
    let Some(session) = state.outgoing.get_mut(&local_id) else {
        return;
    };
    if finished(&session.phase) {
        return;
    }
    let info = session_info(session);
    let peer_key = session.peer_key;
    if let Some(link) = session.link.take() {
        link.close(0u8.into(), b"controller stopped");
    }
    session.clipboard = None;
    session.phase = SessionPhase::Closed;
    drop(state);
    settle_outgoing(shared, &info, SessionPhase::Closed, &peer_key);
}

pub(super) fn fail_outgoing(shared: &Shared, local_id: u64, error: &Error) {
    let mut state = lock(&shared.state);
    let Some(session) = state.outgoing.get_mut(&local_id) else {
        return;
    };
    let info = session_info(session);
    let peer_key = session.peer_key;
    let link_session = session.services_only;
    if let Some(link) = session.link.take() {
        link.close(0u8.into(), b"controller failed");
    }
    session.clipboard = None;
    session.phase = SessionPhase::Failed(error.to_string());
    if link_session {
        if matches!(error, Error::NotHosting) {
            state.link_refused.insert(peer_key);
        } else {
            state.link_refused.remove(&peer_key);
        }
    }
    drop(state);
    settle_outgoing(
        shared,
        &info,
        SessionPhase::Failed(error.to_string()),
        &peer_key,
    );
}

pub(super) fn mark_reconnecting(shared: &Shared, local_id: u64) {
    let mut state = lock(&shared.state);
    let Some(session) = state.outgoing.get_mut(&local_id) else {
        return;
    };
    if finished(&session.phase) || matches!(session.phase, SessionPhase::Stopping) {
        return;
    }
    session.clipboard = None;
    session.link = None;
    session.phase = SessionPhase::Reconnecting;
    let info = session_info(session);
    let peer_key = session.peer_key;
    drop(state);
    settle_outgoing(shared, &info, SessionPhase::Reconnecting, &peer_key);
}

/// Clears the clipboard inbox and emits the terminal phase. The guards and
/// the phase assignment stay with each caller: only a finished session
/// closes, while a failure records even over one.
fn settle_outgoing(shared: &Shared, info: &SessionInfo, phase: SessionPhase, peer_key: &[u8; 32]) {
    {
        let mut state = lock(&shared.state);
        super::services::synchronize(&mut state, &shared.events);
    }
    let owner = {
        let state = lock(&shared.state);
        state
            .outgoing
            .get(&info.id)
            .and_then(|s| s.controller.clone().map(|c| (s.peer_key, c)))
    };
    if let Some((_, controller)) = owner {
        *lock(&controller.clipboard) = None;
        *lock(&controller.cursor) = None;
        controller.inbox.reset();
        lock(&controller.keyboard).clear();
        *lock(&controller.held) = crate::input::HeldInput::default();
        lock(&controller.pointer).reset();
        *lock(&controller.auto) = AutoState::default();
    }
    super::jobs::interrupt_controller_jobs(shared, peer_key);
    shared.events.publish(CoreEvent::SessionChanged {
        id: info.id,
        generation: info.generation,
        role: info.role,
        phase,
        peer_id: info.peer_id.clone(),
    });
}
