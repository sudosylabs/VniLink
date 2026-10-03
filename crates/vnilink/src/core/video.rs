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

pub(super) async fn video_recv_loop(
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
