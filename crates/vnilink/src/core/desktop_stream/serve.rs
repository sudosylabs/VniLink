// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! The host's side of the Desktop stream: accepting it and running its tasks.

use super::*;

pub(super) struct HostedDesktopGuard {
    shared: Arc<Shared>,
    id: u64,
    generation: u64,
    /// Revocation removes the runtime from the service record. Keep the actual
    /// applied input alive until teardown has released it.
    desktop: Arc<DesktopRuntime>,
}

impl Drop for HostedDesktopGuard {
    fn drop(&mut self) {
        let mut state = lock(&self.shared.state);
        release_desktop_input(&self.shared, &self.desktop);
        if state.desktop_control == Some(self.desktop.stamp) {
            state.desktop_control = None;
        }
        if state
            .hosted_services
            .get(&self.id)
            .is_some_and(|r| r.info.generation == self.generation)
        {
            if let Some(mut record) = state.hosted_services.remove(&self.id) {
                super::super::admission::service_ended(
                    &mut state,
                    &record.info.peer_id,
                    std::time::Instant::now(),
                );
                // Backstop for a serve loop that ended without an explicit
                // close: explicit paths set the cause first and `change`
                // keeps the first one. A live parent means the desktop
                // backend or loop itself died.
                let cause = if state.shut_down {
                    CloseCause::Shutdown
                } else {
                    match state.incoming.get(&record.info.connection_id) {
                        // A live connection on an open link: the backend or
                        // loop itself died. A link already closed is a lost
                        // parent, whatever the session's phase says yet.
                        Some(session) if session.connected() => CloseCause::DeviceLoss,
                        _ => CloseCause::ParentLoss,
                    }
                };
                record.change(self.generation, ServicePhase::Closed(cause));
                self.shared.events.publish(CoreEvent::HostedServiceChanged {
                    service: record.info,
                });
            }
        }
    }
}

pub(super) struct StreamCount<'a>(&'a std::sync::atomic::AtomicUsize);

impl Drop for StreamCount<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, std::sync::atomic::Ordering::AcqRel);
    }
}

pub(super) fn release_desktop_input(shared: &Shared, desktop: &DesktopRuntime) {
    desktop
        .host_input
        .release(shared, desktop.info.service.connection_id);
}

pub(in crate::core) async fn serve(
    shared: Arc<Shared>,
    connection_id: u64,
    peer: [u8; 32],
    mut send: SendStream,
    mut recv: RecvStream,
    body: &[u8],
    mut parent_stop: tokio::sync::watch::Receiver<bool>,
) -> Result<(), Error> {
    if body.len() < 27 || body[0] != VERSION || body[25] & !(OPEN_CURSOR | OPEN_STILLS) != 0 {
        write(&mut send, &[VERSION, 5]).await?;
        return Ok(());
    }
    let parent = SessionStamp::new(
        u64::from_be_bytes(body[1..9].try_into().unwrap()),
        u64::from_be_bytes(body[9..17].try_into().unwrap()),
    );
    let remote_id = u64::from_be_bytes(body[17..25].try_into().unwrap());
    let ControlMessage::Hello {
        version,
        codec,
        quality,
        decode,
        bt601,
        ..
    } = ControlMessage::decode(&body[26..])?
    else {
        return Err(Error::Malformed);
    };
    if remote_id == 0 || version != crate::protocol::PROTOCOL_VERSION {
        write(&mut send, &[VERSION, 5]).await?;
        return Ok(());
    }
    // One connection admits a bounded set of simultaneous monitor owners;
    // each carries its own runtime while the lease records are counted at
    // Open. Other connections admit concurrently; `desktop_streams` keeps
    // adapter swaps out until every stream, including teardown, has finished.
    let outcome = {
        let mut state = lock(&shared.state);
        let session = live_session(&state, true, connection_id)?;
        if session.peer_key != peer || session.wire_stamp != parent {
            Err(3)
        } else if !state.trust.service_granted(&peer, ServiceKind::Desktop) {
            Err(6)
        } else if !super::super::admission::approved(
            &state,
            &session.peer_id,
            std::time::Instant::now(),
        ) {
            Err(super::super::admission::NOT_ADMITTED)
        } else if shared
            .desktop_owners
            .load(std::sync::atomic::Ordering::Acquire)
            >= 16
            || state.hosted_services.len() >= 16
            || state
                .hosted_services
                .values()
                .filter(|r| {
                    r.info.connection_id == connection_id
                        && r.info.kind == ServiceKind::Desktop
                        && r.desktop.is_some()
                })
                .count()
                >= MAX_DESKTOP_OWNERS_PER_CONNECTION
        {
            Err(4)
        } else if !decode || !lock(&shared.video_caps).can_encode() {
            Err(1)
        } else {
            let monitor = monitor_size(&shared);
            let caps = lock(&shared.video_caps).clone();
            if monitor.0 == 0 || monitor.1 == 0 {
                Err(1)
            } else if let Some(agreed) = crate::video_caps::negotiate(codec, &caps) {
                let codec = agreed.codec;
                let matrix = crate::VideoCodec::from_wire_id(codec)
                    .map_or(crate::YuvMatrix::Bt709, |agreed| {
                        crate::YuvMatrix::of_codec(bt601, agreed)
                    });
                let peer_id = session.peer_id.clone();
                let id = state.next_service_id;
                state.next_service_id = id.checked_add(1).ok_or(Error::Busy)?;
                let info = ServiceInfo {
                    id,
                    connection_id,
                    generation: parent.generation,
                    peer_id,
                    kind: ServiceKind::Desktop,
                    phase: ServicePhase::Active,
                };
                let (separate, stills) =
                    lock(&shared.capture).as_ref().map_or((false, false), |c| {
                        (
                            body[25] & OPEN_CURSOR != 0 && c.separate_cursor(),
                            body[25] & OPEN_STILLS != 0 && c.stills(),
                        )
                    });
                let desktop = DesktopRuntime::new(
                    DesktopInfo {
                        service: info.clone(),
                        monitor_width: monitor.0,
                        monitor_height: monitor.1,
                        codec,
                        quality: crate::session::start_quality(&quality, monitor, &caps, codec),
                        separate_cursor: separate,
                        stills,
                        matrix,
                    },
                    SessionStamp::new(id, parent.generation),
                    quality.max_bitrate_kbps,
                );
                shared
                    .desktop_owners
                    .fetch_add(1, std::sync::atomic::Ordering::AcqRel);
                desktop
                    .host_input
                    .keep_slot(Arc::new(super::input_cleanup::AdmissionSlot(
                        shared.desktop_owners.clone(),
                    )));
                desktop.set_recording_allowed(
                    state.trust.get(&peer).is_some() && state.trust.sharing.recording,
                );
                let stop = tokio::sync::watch::channel(false).0;
                let receiver = stop.subscribe();
                // Opening requests control once; a busy host admits a viewer.
                if !state.trust.granted(&peer, crate::session::Grant::Control) {
                    desktop
                        .control_state
                        .send_replace((DesktopControlState::Viewing, 0));
                } else if state.input_holder().is_none() {
                    state.desktop_control = Some(desktop.stamp);
                    desktop
                        .control_state
                        .send_replace((DesktopControlState::Controlling, 1));
                } else {
                    desktop
                        .control_state
                        .send_replace((DesktopControlState::Contended, 0));
                }
                state.hosted_services.insert(
                    id,
                    super::services::ServiceRecord {
                        info: info.clone(),
                        remote_id: Some(remote_id),
                        stream_open: false,
                        stop,
                        desktop: Some(desktop.clone()),
                        span: shared.span.clone(),
                    },
                );
                shared
                    .events
                    .publish(CoreEvent::HostedServiceChanged { service: info });
                Ok((desktop, receiver))
            } else {
                Err(1)
            }
        }
    };
    let (desktop, mut service_stop) = match outcome {
        Ok(value) => value,
        Err(status) => {
            write(&mut send, &[VERSION, status]).await?;
            return Ok(());
        }
    };
    shared
        .desktop_streams
        .fetch_add(1, std::sync::atomic::Ordering::AcqRel);
    let _streams = StreamCount(&shared.desktop_streams);
    let _guard = HostedDesktopGuard {
        shared: shared.clone(),
        id: desktop.info.service.id,
        generation: parent.generation,
        desktop: desktop.clone(),
    };
    let link = lock(&shared.state)
        .incoming
        .get(&connection_id)
        .and_then(|session| session.link.clone());
    if let Some(link) = link {
        super::desktop_input::ensure_router(&shared, link, connection_id, parent.generation);
    }
    let control = *desktop.control_state.borrow();
    let mut response = vec![VERSION, 0, control.0.encode()];
    response.extend_from_slice(&control.1.to_be_bytes());
    let recording = *desktop.recording_allowed.borrow();
    response.push(u8::from(recording.0));
    response.extend_from_slice(&recording.1.to_be_bytes());
    response.extend_from_slice(
        &ControlMessage::Accept {
            stamp: desktop.stamp,
            monitor_width: desktop.info.monitor_width,
            monitor_height: desktop.info.monitor_height,
            codec: desktop.info.codec
                | if desktop.info.separate_cursor {
                    crate::protocol::SEPARATE_CURSOR
                } else {
                    0
                }
                | if desktop.info.stills {
                    crate::protocol::STILLS
                } else {
                    0
                },
            quality: desktop.info.quality,
            hostname: None,
            os: crate::OperatingSystem::local(),
        }
        .encode(),
    );
    write(&mut send, &response).await?;
    if !*parent_stop.borrow() && !*service_stop.borrow() {
        let stream_ended = tokio::select! {
            biased;
            _ = parent_stop.changed() => false, _ = service_stop.changed() => false,
            _ = host_receive(&shared, &desktop, &mut recv) => true,
            _ = host_send(&shared, &desktop, &mut send) => true,
        };
        // The stream ended while its connection lives: the viewer finished
        // its side or stopped reading ours, so it closed the desktop. A lost
        // connection is the backstop's to name, and a cause this computer
        // set first, as a failed capture, stays.
        if stream_ended {
            let mut state = lock(&shared.state);
            let connected = state
                .incoming
                .get(&desktop.info.service.connection_id)
                .is_some_and(LiveSession::connected);
            if let Some(record) = state
                .hosted_services
                .get_mut(&desktop.info.service.id)
                .filter(|record| connected && record.info.generation == parent.generation)
            {
                record.change(
                    parent.generation,
                    ServicePhase::Closed(CloseCause::PeerClose),
                );
            }
        }
    }
    let parent_stopped = *parent_stop.borrow();
    // A parent stopped because its link was lost: nobody is left to tell.
    let link_lost = parent_stopped
        && !lock(&shared.state)
            .incoming
            .get(&desktop.info.service.connection_id)
            .is_some_and(LiveSession::link_open);
    if link_lost {
        desktop.ended(&shared, true, crate::DesktopEnd::LinkLost);
    } else if let Some(end) = host_end(&shared, &desktop, parent_stopped) {
        desktop.ended(&shared, true, crate::DesktopEnd::Host(end));
        crate::ignored!(
            crate::trace::STREAM,
            "the viewer may be gone before it hears why the desktop ended",
            tokio::time::timeout(END_PATIENCE, write(&mut send, &[END, end.encode()])).await
        );
    }
    desktop.close();
    crate::ignored!(
        crate::trace::STREAM,
        "a closing stream that cannot finish is closed by the link",
        send.finish()
    );
    Ok(())
}

/// What a host tells its viewer as its stream stops: why its record
/// closed, or that its user stopped the connection. Nothing when the
/// stream itself failed, as when the viewer left or the link broke.
pub(super) fn host_end(
    shared: &Shared,
    desktop: &DesktopRuntime,
    parent_stopped: bool,
) -> Option<crate::StreamEnd> {
    let state = lock(&shared.state);
    let service = &desktop.info.service;
    let phase = state
        .hosted_services
        .get(&service.id)
        .filter(|record| record.info.generation == service.generation)
        .map(|record| record.info.phase.clone());
    match phase {
        Some(ServicePhase::Closed(cause)) => crate::StreamEnd::for_close(cause),
        _ if state.shut_down => Some(crate::StreamEnd::Shutdown),
        _ if parent_stopped => Some(crate::StreamEnd::Disconnected),
        _ => None,
    }
}
