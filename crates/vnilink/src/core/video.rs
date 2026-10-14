//! Encoded frames for one control session.

use super::*;

use std::sync::Arc;

use crate::error::Error;
use crate::events::CoreEvent;
use crate::protocol::Quality;
use crate::session::SessionPhase;
use crate::video::{
    decode_video_body, encode_video_body, EncodedFrame, HostCapture, KeyframeFlag, VideoInbox,
    VideoOutbox, VideoPush, VideoSubscription,
};

/// One consumer of the shared physical capture. Legacy desktop sessions and
/// neutral Desktop owners are separate records; a files-only or stopped
/// desktop is absent. Each target owns its encoder, quality and queue.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VideoTarget {
    /// True when `id` is a hosted Desktop service. False for a legacy session.
    pub service: bool,
    pub id: u64,
    pub generation: u64,
    pub quality: Quality,
    pub separate_cursor: bool,
    pub codec: u8,
    /// Display this consumer selected. Zero means it has not chosen one and
    /// still follows whichever capture the supervisor opened.
    pub display_id: u64,
    /// Topology revision the selection was accepted under. Zero when the
    /// consumer selected nothing or the display is no longer published.
    pub revision: u64,
}

/// Consumers and the resolved source for consumers without a selection.
#[derive(Debug, Clone)]
pub struct VideoRouting {
    pub targets: Vec<VideoTarget>,
    pub default_source: Option<u64>,
}
impl VideoRouting {
    pub fn serves_selection(&self, selected: &[u64], source: u64) -> bool {
        serves_source(
            selected.first().copied().unwrap_or(0),
            source,
            self.default_source,
        )
    }
}
impl VideoTarget {
    pub fn serves_source(&self, source: u64, default: Option<u64>) -> bool {
        serves_source(self.display_id, source, default)
    }
}
fn serves_source(selected: u64, source: u64, default: Option<u64>) -> bool {
    source == 0
        || if selected == 0 {
            default.is_none_or(|id| id == source)
        } else {
            selected == source
        }
}

impl Core {
    /// Fence physical capture startup to the consumers present when it began.
    /// One departing viewer does not cancel its siblings. A newly attached
    /// viewer cannot adopt a cancelled startup; the supervisor opens afresh.
    pub fn desktop_capture_startup(
        &self,
        display_id: u64,
        stop: Arc<std::sync::atomic::AtomicBool>,
        timeout: std::time::Duration,
    ) -> Result<crate::BackendStartup, Error> {
        let state = self.alive_state()?;
        let routing = video_routing_locked(&self.shared, &state);
        let mut owners = Vec::new();
        for target in routing
            .targets
            .into_iter()
            .filter(|target| target.serves_source(display_id, routing.default_source))
        {
            if target.service {
                if let Some(owner) = state
                    .hosted_services
                    .get(&target.id)
                    .filter(|owner| owner.info.generation == target.generation)
                {
                    owners.push(owner.stop.subscribe());
                }
            } else if let Some(owner) = state
                .incoming
                .get(&target.id)
                .filter(|owner| owner.generation == target.generation)
            {
                owners.push(owner.desktop_stop.subscribe());
            }
        }
        let startup = crate::BackendStartup::for_consumers(stop, owners, timeout);
        startup.check()?;
        Ok(startup)
    }
    /// Capture workers stop this generation's encoder without stopping the
    /// peer connection. Avoid a full installation snapshot on every frame.
    pub fn host_desktop_active(&self, id: u64, generation: u64) -> Result<bool, Error> {
        let state = self.alive_state()?;
        Ok(state.incoming.get(&id).is_some_and(|s| {
            s.generation == generation
                && s.phase == SessionPhase::Active
                && !s.files_only
                && !*s.desktop_stop.borrow()
                && s.quality.is_some()
        }))
    }

    /// Receives encoded frames for the controlling session. One subscriber at a time.
    ///
    /// Frames are not core events. A slow subscriber does not block input. Drop the
    /// subscription to free the slot.
    pub fn subscribe_video(&self) -> Result<VideoSubscription, Error> {
        let selected = {
            let state = self.alive_state()?;
            if state
                .outgoing
                .values()
                .filter(|s| !finished(&s.phase))
                .count()
                > 1
            {
                return Err(Error::Busy);
            }
            state.single_outgoing().map(|s| (s.id, s.generation))
        };
        if let Some((id, generation)) = selected {
            return self.subscribe_session_video(id, generation);
        }
        // Legacy clients can subscribe before connect(). New tabs use the
        // explicit API below and never share this reservation/inbox.
        let mut slot = lock(&self.shared.video_subscriber);
        if slot.upgrade().is_some() {
            return Err(Error::Busy);
        }
        let inbox = video_inbox(&self.shared);
        let alive = Arc::new(());
        *slot = Arc::downgrade(&alive);
        Ok(VideoSubscription::new(inbox, alive))
    }

    /// One independent subscriber per active controller generation.
    pub fn subscribe_session_video(
        &self,
        id: u64,
        generation: u64,
    ) -> Result<VideoSubscription, Error> {
        {
            let state = self.alive_state()?;
            if state
                .outgoing
                .get(&id)
                .is_some_and(|s| s.files_only || *s.desktop_stop.borrow())
            {
                return Err(Error::Unsupported("video on a service connection"));
            }
        }
        let controller = self.controller_state(id, generation)?;
        let mut slot = lock(&controller.subscriber);
        if slot.upgrade().is_some() {
            return Err(Error::Busy);
        }
        let alive = Arc::new(());
        *slot = Arc::downgrade(&alive);
        Ok(VideoSubscription::new(controller.inbox.clone(), alive))
    }

    /// Reports the monitor this host captures.
    ///
    /// The platform captures and encodes, then passes bytes to [`Core::publish_video`].
    /// An idle neutral connection may configure capture before Desktop Open.
    /// Refused while Desktop is active or completing teardown.
    pub fn set_capture(&self, capture: Box<dyn HostCapture>) -> Result<(), Error> {
        let state = self.alive_state()?;
        if state
            .incoming
            .values()
            .any(|session| !finished(&session.phase) && !session.services_only)
            || self
                .shared
                .desktop_streams
                .load(std::sync::atomic::Ordering::Acquire)
                != 0
        {
            return Err(Error::Busy);
        }
        let _gate = self
            .shared
            .desktop_gate
            .try_lock()
            .map_err(|_| Error::Busy)?;
        *lock(&self.shared.capture) = Some(capture);
        Ok(())
    }

    /// Registers the platform's probe-validated video codec abilities.
    ///
    /// The library never probes hardware itself. The native layer calls this
    /// after initializing its encoder and decoder so `capabilities()`,
    /// admission and negotiation report an explicit unsupported state
    /// instead of admitting a session that can never carry video. Every set
    /// bit must mean a backend processed a sample; a failed probe registers
    /// cleared bits, never assumed ones.
    pub fn set_video_codecs(&self, caps: crate::video_caps::VideoCodecCaps) -> Result<(), Error> {
        self.ensure_alive()?;
        caps.validate()?;
        *lock(&self.shared.video_caps) = caps;
        Ok(())
    }

    /// Currently registered video codec abilities.
    pub fn video_codecs(&self) -> Result<crate::video_caps::VideoCodecCaps, Error> {
        self.ensure_alive()?;
        Ok(lock(&self.shared.video_caps).clone())
    }

    /// Live capture consumers. The list is a snapshot: a later open, close or
    /// revoke changes the next call. Order is service flag, then id.
    pub fn video_targets(&self) -> Result<Vec<VideoTarget>, Error> {
        let state = self.alive_state()?;
        Ok(video_targets_locked(&self.shared, &state))
    }

    /// Frame, cursor and failure attribution share this atomic routing snapshot.
    pub fn video_routing(&self) -> Result<VideoRouting, Error> {
        let state = self.alive_state()?;
        Ok(video_routing_locked(&self.shared, &state))
    }

    /// Agreed H.264 codec id for one hosted session (`CODEC_H264` or
    /// `CODEC_H264_HIGH`). The serve path opens the matching encoder
    /// profile with it.
    pub fn host_codec(&self, session_id: u64) -> Result<u8, Error> {
        let state = self.alive_state()?;
        let session = state.incoming.get(&session_id).ok_or(Error::NoSession)?;
        if session.phase != SessionPhase::Active {
            return Err(Error::NoSession);
        }
        Ok(session.codec)
    }

    /// Queues one encoded frame for the active hosted session.
    ///
    /// A full queue drops predicted frames until the next keyframe and still returns
    /// `Ok`. The frame bytes are not logged.
    pub fn publish_video(&self, session_id: u64, mut frame: EncodedFrame) -> Result<(), Error> {
        self.ensure_alive()?;
        frame.check()?;
        let outbox = {
            let state = self.alive_state()?;
            let session = state.incoming.get(&session_id).ok_or(Error::NoSession)?;
            if session.phase != SessionPhase::Active || *session.desktop_stop.borrow() {
                return Err(Error::NoSession);
            }
            if Some(frame.codec) != crate::VideoCodec::from_wire_id(session.codec) {
                return Err(Error::InvalidSetting(
                    "frame codec differs from negotiated codec",
                ));
            }
            session.video.clone().ok_or(Error::NoSession)?
        };
        // Same backstop as the service path: a zero revision reads the live
        // topology. The display id is never invented here.
        if frame.revision == 0 {
            frame.revision = lock(&self.shared.topology).revision;
        }
        if matches!(outbox.push(frame), VideoPush::KeyframeNeeded) {
            self.shared
                .events
                .publish(CoreEvent::KeyframeRequested { id: session_id });
        }
        Ok(())
    }

    /// Queued frames and cumulative drops for the active hosted session.
    ///
    /// Poll alongside `session_paths` RTT/relay to drive adaptive quality:
    /// rising `queued` or `dropped` means the encoder outruns the path.
    pub fn video_backpressure(&self, session_id: u64) -> Result<(usize, u64), Error> {
        self.ensure_alive()?;
        let state = self.alive_state()?;
        let session = state.incoming.get(&session_id).ok_or(Error::NoSession)?;
        if session.phase != SessionPhase::Active {
            return Err(Error::NoSession);
        }
        let outbox = session.video.clone().ok_or(Error::NoSession)?;
        Ok((outbox.queued(), outbox.dropped()))
    }

    /// Starting limits for outgoing sessions and newly opened Desktop owners.
    /// The application persists this preference and restores it before dialing.
    pub fn desktop_quality_defaults(&self) -> Result<Quality, Error> {
        Ok(self.alive_state()?.desktop_quality_default)
    }

    /// Codec for new outgoing Desktop owners; None negotiates automatically.
    pub fn desktop_codec_default(&self) -> Result<Option<crate::VideoCodec>, Error> {
        Ok(self.alive_state()?.desktop_codec_default)
    }

    /// Saves a preference without changing existing owners or requiring a decoder now.
    pub fn set_desktop_codec_default(&self, codec: Option<crate::VideoCodec>) -> Result<(), Error> {
        self.alive_state()?.desktop_codec_default = codec;
        Ok(())
    }

    pub fn set_desktop_quality_defaults(&self, quality: Quality) -> Result<(), Error> {
        let quality = Quality::new(
            quality.max_bitrate_kbps,
            quality.max_frame_rate,
            quality.max_width,
            quality.max_height,
        )?;
        self.alive_state()?.desktop_quality_default = quality;
        Ok(())
    }

    /// Changes the encoder limits for one live session.
    ///
    /// The role selects which side changes: a host call applies to the
    /// incoming session, a controller call queues the change for the host
    /// and applies it locally. Session ids alone cannot select the side:
    /// both roles number their sessions from the same counter, so an
    /// incoming and an outgoing session routinely share one id. A manual
    /// controller choice remains selected until the session reconnects.
    pub fn update_session_quality(
        &self,
        session_id: u64,
        role: SessionRole,
        quality: Quality,
    ) -> Result<(), Error> {
        self.apply_session_quality(session_id, role, None, quality)
            .map(|_| ())
    }

    /// Applies UI edits only while the selected session generation is still live.
    pub fn update_session_quality_for_generation(
        &self,
        session_id: u64,
        generation: u64,
        role: SessionRole,
        quality: Quality,
    ) -> Result<(), Error> {
        self.apply_session_quality(session_id, role, Some(generation), quality)
            .map(|_| ())
    }

    fn apply_session_quality(
        &self,
        session_id: u64,
        role: SessionRole,
        generation: Option<u64>,
        quality: Quality,
    ) -> Result<bool, Error> {
        let mut state = self.alive_state()?;
        if let Some(expected) = generation {
            let actual = match role {
                SessionRole::Host => state
                    .incoming
                    .get(&session_id)
                    .map(|session| session.generation),
                SessionRole::Controller => state
                    .outgoing
                    .get(&session_id)
                    .map(|session| session.generation),
            }
            .ok_or(Error::NoSession)?;
            if actual != expected {
                return Err(Error::StaleSession);
            }
        }
        match role {
            SessionRole::Host => {
                let session = state
                    .incoming
                    .get_mut(&session_id)
                    .filter(|session| {
                        session.phase == SessionPhase::Active
                            && !session.files_only
                            && !*session.desktop_stop.borrow()
                    })
                    .ok_or(Error::NoSession)?;
                session.quality = Some(quality);
                drop(state);
                self.shared.events.publish(CoreEvent::QualityUpdated {
                    id: session_id,
                    quality,
                });
                Ok(true)
            }
            SessionRole::Controller => {
                let outbox = state
                    .outgoing
                    .get(&session_id)
                    .filter(|session| {
                        session.id == session_id
                            && session.phase == SessionPhase::Active
                            && !session.files_only
                            && !*session.desktop_stop.borrow()
                    })
                    .and_then(|session| session.outbox.clone())
                    .ok_or(Error::NoSession)?;
                let session = state.outgoing.get_mut(&session_id).expect("checked");
                let controller = session.controller.as_ref().ok_or(Error::NoSession)?;
                let owner = (session.id, session.generation);
                let mut auto = lock(&controller.auto);
                // Only change the local limits after the command is queued.
                outbox.push_quality(quality)?;
                auto.manual_session = Some(owner);
                drop(auto);
                session.quality = Some(quality);
                drop(state);
                self.shared.events.publish(CoreEvent::QualityUpdated {
                    id: session_id,
                    quality,
                });
                Ok(true)
            }
        }
    }
}

pub(super) fn video_inbox(shared: &Shared) -> Arc<VideoInbox> {
    let mut slot = lock(&shared.video_inbox);
    if let Some(inbox) = slot.as_ref() {
        return Arc::clone(inbox);
    }
    let inbox = Arc::new(VideoInbox::new());
    *slot = Some(Arc::clone(&inbox));
    inbox
}

pub(super) async fn host_video_loop(
    connection: iroh::endpoint::Connection,
    outbox: Arc<VideoOutbox>,
    stop_rx: &mut tokio::sync::watch::Receiver<bool>,
) {
    // The reliable stream opens lazily: datagram sessions may never need it.
    let mut send: Option<iroh::endpoint::SendStream> = None;
    let mut seq: u32 = 0;
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
            continue;
        }
        // Unreliable datagrams have no head-of-line blocking: one lost packet
        // stalls only its own frame, and the keyframe path already recovers
        // drops. The stream stays as fallback where datagrams are disabled.
        if let Some(max) = connection.max_datagram_size() {
            if max >= crate::video::FRAG_HEADER + 64 {
                let mut datagrams = Vec::new();
                for frame in &frames {
                    let body = encode_video_body(frame);
                    for frag in crate::video::fragment(&body, seq, max) {
                        datagrams.push(bytes::Bytes::from(frag));
                    }
                    seq = seq.wrapping_add(1);
                }
                // Loss-tolerant by design: older queued datagrams may be
                // dropped to make room, and expiry plus keyframe requests
                // recover the decoder.
                let _ = connection.send_many_datagrams(&datagrams);
                continue;
            }
        }
        if send.is_none() {
            let opened = tokio::select! {
                opened = connection.open_uni() => opened,
                _ = connection.closed() => return,
                _ = stop_rx.changed() => return,
            };
            match opened {
                Ok(stream) => send = Some(stream),
                Err(_) => return,
            }
        }
        let stream = send.as_mut().expect("opened");
        let mut failed = false;
        for frame in frames {
            if write_video_frame(stream, &frame).await.is_err() {
                failed = true;
                break;
            }
        }
        if failed {
            return;
        }
    }
}

pub(super) async fn video_recv_loop(
    connection: iroh::endpoint::Connection,
    inbox: Arc<VideoInbox>,
    keyframe: Arc<KeyframeFlag>,
    stop_rx: &mut tokio::sync::watch::Receiver<bool>,
    cursor: Option<(crate::protocol::SessionStamp, super::cursor::CursorInbox)>,
    codec: u8,
) {
    // Both transports stay live for the session: the host picks datagrams
    // when its path allows them and the stream otherwise, switching per
    // batch if the path changes. Each frame travels exactly one way.
    let mut reassembler = crate::video::Reassembler::new();
    let mut streams = tokio::task::JoinSet::new();
    loop {
        if *stop_rx.borrow() {
            return;
        }
        tokio::select! {
            _ = connection.closed() => return,
            _ = stop_rx.changed() => return,
            datagram = connection.read_datagram() => {
                let Ok(bytes) = datagram else {
                    return;
                };
                let Some(body) = reassembler.push(&bytes) else {
                    continue;
                };
                match decode_video_body(&body) {
                    Ok(frame) => {
                        if Some(frame.codec)!=crate::VideoCodec::from_wire_id(codec) {continue;}
                        if matches!(inbox.push(frame), VideoPush::KeyframeNeeded) {
                            keyframe.request();
                        }
                    }
                    // The body was consumed. A codec this install does not
                    // know must not end input.
                    Err(Error::Unsupported(_)) => {}
                    Err(_) => {}
                }
            }
            incoming = connection.accept_uni() => {
                let Ok(mut stream) = incoming else { return; };
                // At most one video stream and one cursor stream. Unknown or
                // stalled streams cannot accumulate tasks or block datagrams.
                if streams.len() >= 2 { let _ = stream.stop(0u8.into()); continue; }
                let inbox = Arc::clone(&inbox);
                let keyframe = Arc::clone(&keyframe);
                let cursor = cursor.clone();
                streams.spawn(async move {
                    let mut prefix = [0;4];
                    if !matches!(tokio::time::timeout(Duration::from_secs(2), stream.read_exact(&mut prefix)).await, Ok(Ok(()))) { return; }
                    if prefix == crate::cursor::MAGIC {
                        if let Some((stamp, inbox)) = cursor {
                            super::cursor::receive_cursor(stream, stamp, inbox).await;
                        }
                        return;
                    }
                    loop {
                        let len = u32::from_be_bytes(prefix) as usize;
                        if crate::video::check_video_len(len).is_err() { return; }
                        let mut body = vec![0;len];
                        if stream.read_exact(&mut body).await.is_err() { return; }
                        if let Ok(frame) = decode_video_body(&body) {
                            if Some(frame.codec)!=crate::VideoCodec::from_wire_id(codec) {return;}
                            if matches!(inbox.push(frame), VideoPush::KeyframeNeeded) { keyframe.request(); }
                        }
                        if stream.read_exact(&mut prefix).await.is_err() { return; }
                    }
                });
            }
            _ = streams.join_next(), if !streams.is_empty() => {}

        }
    }
}

pub(super) async fn write_video_frame(
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

pub(super) async fn read_video_frame(
    recv: &mut iroh::endpoint::RecvStream,
) -> Result<EncodedFrame, Error> {
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

fn video_targets_locked(shared: &Shared, state: &State) -> Vec<VideoTarget> {
    let topology = lock(&shared.topology).clone();
    let revision_of = |display_id: u64| {
        topology
            .displays
            .iter()
            .find(|display| display.id == display_id)
            .map(|display| display.revision)
            .unwrap_or(0)
    };
    let mut targets = Vec::new();
    for record in state.hosted_services.values() {
        if record.info.kind != crate::ServiceKind::Desktop
            || record.info.phase != crate::ServicePhase::Active
            || *record.stop.borrow()
        {
            continue;
        }
        let Some(desktop) = &record.desktop else {
            continue;
        };
        let info = desktop.metadata();
        let display_id = lock(&desktop.selected).first().copied().unwrap_or(0);
        targets.push(VideoTarget {
            service: true,
            id: info.service.id,
            generation: info.service.generation,
            quality: info.quality,
            separate_cursor: info.separate_cursor,
            codec: info.codec,
            display_id,
            revision: revision_of(display_id),
        });
    }
    for session in state.incoming.values() {
        if session.phase != SessionPhase::Active
            || session.files_only
            || session.services_only
            || *session.desktop_stop.borrow()
            || session.desktop_owner.is_some()
        {
            continue;
        }
        let Some(quality) = session.quality else {
            continue;
        };
        let display_id = session.selected_displays.first().copied().unwrap_or(0);
        targets.push(VideoTarget {
            service: false,
            id: session.id,
            generation: session.generation,
            quality,
            separate_cursor: session.separate_cursor,
            codec: session.codec,
            display_id,
            revision: revision_of(display_id),
        });
    }
    targets.sort_by_key(|target| (target.service, target.id));
    targets
}

pub(super) fn video_routing_locked(shared: &Shared, state: &State) -> VideoRouting {
    let targets = video_targets_locked(shared, state);
    let default_source = targets
        .iter()
        .filter_map(|t| (t.display_id != 0).then_some(t.display_id))
        .min()
        .or_else(|| lock(&shared.topology).displays.first().map(|d| d.id));
    VideoRouting {
        targets,
        default_source,
    }
}
