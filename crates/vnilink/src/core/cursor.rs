//! The cursor stream is reliable, bounded and separate from H.264 transport.
use super::*;
use crate::cursor::{self, CursorState};
use crate::protocol::SessionStamp;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

pub(super) type CursorInbox = Arc<Mutex<Option<(SessionStamp, Arc<CursorState>)>>>;

impl Core {
    /// Native viewers enable this only when they render cursor metadata.
    pub fn set_cursor_receiver(&self, enabled: bool) -> Result<(), Error> {
        self.ensure_alive()?;
        self.shared
            .cursor_receiver
            .store(enabled, Ordering::Release);
        Ok(())
    }
    /// True only after this direction negotiated independent cursor rendering.
    pub fn separate_cursor(&self, id: u64, role: SessionRole) -> Result<bool, Error> {
        let state = self.alive_state()?;
        let session = match role {
            SessionRole::Host => state.incoming.as_ref(),
            SessionRole::Controller => state.outgoing.get(&id),
        };
        let session = session
            .filter(|s| s.id == id && s.phase == SessionPhase::Active)
            .ok_or(Error::NoSession)?;
        Ok(session.separate_cursor)
    }
    /// Copies no image pixels. The platform owns one immutable cursor snapshot.
    pub fn publish_cursor(
        &self,
        id: u64,
        generation: u64,
        cursor: CursorState,
    ) -> Result<(), Error> {
        let state = self.alive_state()?;
        let session = state
            .incoming
            .as_ref()
            .filter(|s| {
                s.id == id
                    && s.generation == generation
                    && s.phase == SessionPhase::Active
                    && s.separate_cursor
            })
            .ok_or(Error::NoSession)?;
        self.shared.cursor_outbox.send_replace(Some((
            SessionStamp::new(session.id, session.generation),
            Arc::new(cursor),
        )));
        Ok(())
    }
    /// A snapshot for this exact outgoing generation; stale cursor data stays hidden.
    pub fn cursor(&self, id: u64, generation: u64) -> Result<Option<Arc<CursorState>>, Error> {
        let state = self.alive_state()?;
        let session = state
            .outgoing
            .get(&id)
            .filter(|s| {
                s.id == id
                    && s.generation == generation
                    && s.phase == SessionPhase::Active
                    && s.separate_cursor
            })
            .ok_or(Error::NoSession)?;
        let stamp = session.wire_stamp;
        let controller = session.controller.as_ref().ok_or(Error::NoSession)?;
        let result = lock(&controller.cursor)
            .as_ref()
            .filter(|(owner, _)| *owner == stamp)
            .map(|(_, cursor)| Arc::clone(cursor));
        Ok(result)
    }
}

pub(super) async fn send_cursor(
    connection: iroh::endpoint::Connection,
    stamp: SessionStamp,
    mut updates: tokio::sync::watch::Receiver<Option<(SessionStamp, Arc<CursorState>)>>,
    mut stop: tokio::sync::watch::Receiver<bool>,
) {
    let Ok(mut stream) = connection.open_uni().await else {
        return;
    };
    let mut header = cursor::MAGIC.to_vec();
    header.extend_from_slice(&stamp.id.to_be_bytes());
    header.extend_from_slice(&stamp.generation.to_be_bytes());
    if stream.write_all(&header).await.is_err() {
        return;
    }
    let mut previous: Option<Arc<CursorState>> = None;
    loop {
        if *stop.borrow() {
            return;
        }
        let current = updates.borrow_and_update().clone();
        if let Some((owner, current)) = current.filter(|(owner, _)| *owner == stamp) {
            let _ = owner;
            if previous.as_deref() != Some(current.as_ref()) {
                let changed =
                    previous.as_ref().and_then(|s| s.image.as_ref()) != current.image.as_ref();
                let body = cursor::encode(&current, changed);
                let write = async {
                    stream.write_all(&(body.len() as u32).to_be_bytes()).await?;
                    stream.write_all(&body).await
                };
                tokio::select! {
                    result = tokio::time::timeout(Duration::from_secs(2), write) => {
                        if !matches!(result, Ok(Ok(()))) { return; }
                    }
                    _ = stop.changed() => return,
                    _ = connection.closed() => return,
                }
                previous = Some(current);
            }
        }
        tokio::select! {
            result = updates.changed() => if result.is_err() { return; },
            _ = stop.changed() => return,
            _ = connection.closed() => return,
        }
    }
}

pub(super) async fn receive_cursor(
    mut stream: iroh::endpoint::RecvStream,
    stamp: SessionStamp,
    inbox: CursorInbox,
) {
    let mut owner = [0; 16];
    if !matches!(
        tokio::time::timeout(Duration::from_secs(2), stream.read_exact(&mut owner)).await,
        Ok(Ok(()))
    ) {
        return;
    }
    if owner[..8] != stamp.id.to_be_bytes() || owner[8..] != stamp.generation.to_be_bytes() {
        return;
    }
    let mut image = None;
    loop {
        let mut len = [0; 4];
        if stream.read_exact(&mut len).await.is_err() {
            return;
        }
        let len = u32::from_be_bytes(len) as usize;
        if !(10..=cursor::MAX_BODY).contains(&len) {
            return;
        }
        let mut body = vec![0; len];
        if !matches!(
            tokio::time::timeout(Duration::from_secs(2), stream.read_exact(&mut body)).await,
            Ok(Ok(()))
        ) {
            return;
        }
        let Ok(mut update) = cursor::decode(&body) else {
            return;
        };
        if update.image.is_some() {
            image = update.image.clone();
        }
        update.image = image.clone();
        *lock(&inbox) = Some((stamp, Arc::new(update)));
    }
}
