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

impl Core {
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

    /// Records the platform's H.264 codec availability.
    ///
    /// The library never probes hardware itself. The native host calls this
    /// after asking its adapters (VA-API on Linux) so `capabilities()` and
    /// host admission report an explicit unsupported state instead of
    /// admitting a session that can never carry video.
    pub fn set_h264_support(&self, encode: bool, decode: bool) -> Result<(), Error> {
        self.ensure_alive()?;
        self.shared
            .h264_encode
            .store(encode, std::sync::atomic::Ordering::Relaxed);
        self.shared
            .h264_decode
            .store(decode, std::sync::atomic::Ordering::Relaxed);
        Ok(())
    }

    /// Records whether this host encodes H.264 High profile. The native host
    /// calls this after probing (VA-API High entrypoint on Linux,
    /// VideoToolbox on macOS). Defaults to false: without it the session
    /// negotiates Baseline. Every decoder we ship handles both profiles, so
    /// only the host side needs this flag.
    pub fn set_h264_high(&self, high: bool) -> Result<(), Error> {
        self.ensure_alive()?;
        self.shared
            .h264_high_encode
            .store(high, std::sync::atomic::Ordering::Relaxed);
        Ok(())
    }

    /// Agreed H.264 codec id for one hosted session (`CODEC_H264` or
    /// `CODEC_H264_HIGH`). The serve path opens the matching encoder
    /// profile with it.
    pub fn host_codec(&self, session_id: u64) -> Result<u8, Error> {
        let state = self.alive_state()?;
        let session = state.incoming.as_ref().ok_or(Error::NoSession)?;
        if session.id != session_id || session.phase != SessionPhase::Active {
            return Err(Error::NoSession);
        }
        Ok(session.codec)
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

    /// Queued frames and cumulative drops for the active hosted session.
    ///
    /// Poll alongside `session_paths` RTT/relay to drive adaptive quality:
    /// rising `queued` or `dropped` means the encoder outruns the path.
    pub fn video_backpressure(&self, session_id: u64) -> Result<(usize, u64), Error> {
        self.ensure_alive()?;
        let state = self.alive_state()?;
        let session = state.incoming.as_ref().ok_or(Error::NoSession)?;
        if session.id != session_id || session.phase != SessionPhase::Active {
            return Err(Error::NoSession);
        }
        let outbox = session.video.clone().ok_or(Error::NoSession)?;
        Ok((outbox.queued(), outbox.dropped()))
    }

    /// One adaptive step for the controlling session. Reads the selected
    /// path RTT/relay from the live connection and the inbox drop counter,
    /// then applies `suggest_quality` with a 2s cooldown. Returns the applied
    /// quality, or `None` to hold. Call every ~2s from the viewer; manual
    /// `update_session_quality` holds the manual preset for that generation.
    pub fn auto_quality_tick(&self, session_id: u64) -> Result<Option<Quality>, Error> {
        self.ensure_alive()?;
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        let (current, generation, link, inbox) = {
            let state = self.alive_state()?;
            let session = state.outgoing.as_ref().ok_or(Error::NoSession)?;
            if session.id != session_id || session.phase != SessionPhase::Active {
                return Err(Error::NoSession);
            }
            let current = session
                .quality
                .unwrap_or_else(crate::session::controller_offer);
            (
                current,
                session.generation,
                session.link.clone(),
                video_inbox(&self.shared),
            )
        };
        let Some(link) = link else {
            return Ok(None);
        };
        let mut rtt_ms = u64::MAX;
        let mut relay = false;
        for path in link.paths().iter() {
            if path.is_selected() {
                rtt_ms = u64::try_from(path.rtt().as_millis()).unwrap_or(u64::MAX);
                relay = path.is_relay();
                break;
            }
        }
        if rtt_ms == u64::MAX {
            return Ok(None);
        }
        let queued = inbox.len();
        let dropped = inbox.dropped();
        let delta = {
            let mut auto = lock(&self.shared.auto);
            if auto.manual_session == Some((session_id, generation)) {
                return Ok(None);
            }
            let delta = dropped.saturating_sub(auto.last_dropped);
            auto.last_dropped = dropped;
            if now_ms.saturating_sub(auto.last_change_ms) < 2000 {
                return Ok(None);
            }
            delta
        };
        let Some(next) = crate::session::suggest_quality(&current, rtt_ms, relay, queued, delta)
        else {
            return Ok(None);
        };
        if !self.apply_session_quality(
            session_id,
            SessionRole::Controller,
            next,
            Some(generation),
        )? {
            return Ok(None);
        }
        lock(&self.shared.auto).last_change_ms = now_ms;
        Ok(Some(next))
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
        self.apply_session_quality(session_id, role, quality, None)
            .map(|_| ())
    }

    fn apply_session_quality(
        &self,
        session_id: u64,
        role: SessionRole,
        quality: Quality,
        automatic_generation: Option<u64>,
    ) -> Result<bool, Error> {
        let mut state = self.alive_state()?;
        match role {
            SessionRole::Host => {
                let session = state
                    .incoming
                    .as_mut()
                    .filter(|session| {
                        session.id == session_id && session.phase == SessionPhase::Active
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
                    .as_ref()
                    .filter(|session| {
                        session.id == session_id && session.phase == SessionPhase::Active
                    })
                    .and_then(|session| session.outbox.clone())
                    .ok_or(Error::NoSession)?;
                let session = state.outgoing.as_mut().expect("checked");
                let owner = (session.id, session.generation);
                let mut auto = lock(&self.shared.auto);
                if let Some(generation) = automatic_generation {
                    // A simultaneous manual selection or reconnect takes priority.
                    if generation != session.generation || auto.manual_session == Some(owner) {
                        return Ok(false);
                    }
                }
                // Only change the local limits after the command is queued.
                outbox.push_quality(quality)?;
                if automatic_generation.is_none() {
                    auto.manual_session = Some(owner);
                }
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
