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
        let _engine = self.engine();
        self.ensure_alive()?;
        self.shared
            .cursor_receiver
            .store(enabled, Ordering::Release);
        Ok(())
    }
    /// True only after this direction negotiated independent cursor rendering.
    pub fn separate_cursor(&self, id: u64, role: SessionRole) -> Result<bool, Error> {
        let _engine = self.engine();
        let state = self.alive_state()?;
        let session = match role {
            SessionRole::Host => state.incoming.get(&id),
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
            .get(&id)
            .filter(|s| {
                s.id == id
                    && s.generation == generation
                    && s.phase == SessionPhase::Active
                    && s.separate_cursor
                    && !*s.desktop_stop.borrow()
            })
            .ok_or(Error::NoSession)?;
        session.cursor_outbox.send_replace(Some((
            SessionStamp::new(session.id, session.generation),
            Arc::new(cursor),
        )));
        Ok(())
    }
    /// A snapshot for this exact outgoing generation; stale cursor data stays hidden.
    pub fn cursor(&self, id: u64, generation: u64) -> Result<Option<Arc<CursorState>>, Error> {
        let _engine = self.engine();
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

#[cfg(test)]
mod review_v11 {
    use super::*;
    struct Capture;
    impl crate::HostCapture for Capture {
        fn monitor(&self) -> (u16, u16) {
            (64, 64)
        }
        fn separate_cursor(&self) -> bool {
            true
        }
    }
    #[test]
    fn legacy_two_peer_cursor_publish_preserves_each_consumers_latest_sample() {
        let host = crate::tests::support::start_direct();
        host.set_capture(Box::new(Capture)).unwrap();
        host.start_hosting().unwrap();
        let first = crate::tests::support::start_direct();
        let second = crate::tests::support::start_direct();
        for viewer in [&first, &second] {
            viewer.set_cursor_receiver(true).unwrap();
            viewer
                .accept_invitation(&host.create_invitation(Duration::from_secs(60)).unwrap())
                .unwrap();
            host.accept_invitation(&viewer.create_invitation(Duration::from_secs(60)).unwrap())
                .unwrap();
            host.set_desktop_sharing(crate::Grants {
                view: true,
                control: true,
                clipboard: false,
                transfer: false,
            })
            .unwrap();
            viewer.connect(&host.endpoint_id()).unwrap();
        }
        let owners = host.hosted_sessions().unwrap();
        assert_eq!(owners.len(), 2);
        let a = SessionStamp::new(owners[0].id, owners[0].generation);
        let b = SessionStamp::new(owners[1].id, owners[1].generation);
        let mut reader_a = lock(&host.shared.state)
            .incoming
            .get(&a.id)
            .unwrap()
            .cursor_outbox
            .subscribe();
        let mut reader_b = lock(&host.shared.state)
            .incoming
            .get(&b.id)
            .unwrap()
            .cursor_outbox
            .subscribe();
        let sample = CursorState {
            position: (12, 34),
            visible: true,
            image: None,
        };
        host.publish_cursor(a.id, a.generation, sample.clone())
            .unwrap();
        host.publish_cursor(
            b.id,
            b.generation,
            CursorState {
                position: (56, 78),
                ..sample.clone()
            },
        )
        .unwrap();
        // Exact per-owner filter used by production send_cursor. Readers
        // are scheduled only after both native publications in this probe.
        let delivered_a = reader_a
            .borrow_and_update()
            .clone()
            .filter(|(owner, _)| *owner == a);
        let delivered_b = reader_b
            .borrow_and_update()
            .clone()
            .filter(|(owner, _)| *owner == b);
        assert_eq!(delivered_a.as_ref().unwrap().1.position, (12, 34));
        assert_eq!(delivered_b.as_ref().unwrap().1.position, (56, 78));
        let first_parent = first.snapshot().unwrap().outgoing.unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while first
            .cursor(first_parent.id, first_parent.generation)
            .unwrap()
            .is_none()
        {
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(2));
        }
        assert_eq!(
            first
                .cursor(first_parent.id, first_parent.generation)
                .unwrap()
                .unwrap()
                .position,
            (12, 34)
        );
        host.stop_hosted_peer(a.id).unwrap();
        while !finished(&first.controller_session(first_parent.id).unwrap().phase) {
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(host
            .publish_cursor(a.id, a.generation, sample.clone())
            .is_err());
        let replaced = first.connect(&host.endpoint_id()).unwrap();
        let new = host
            .hosted_sessions()
            .unwrap()
            .into_iter()
            .find(|s| s.peer_id == first.endpoint_id())
            .unwrap();
        assert_ne!(new.generation, a.generation);
        assert!(lock(&host.shared.state)
            .incoming
            .get(&new.id)
            .unwrap()
            .cursor_outbox
            .borrow()
            .is_none());
        host.publish_cursor(
            new.id,
            new.generation,
            CursorState {
                position: (90, 91),
                ..sample
            },
        )
        .unwrap();
        assert!(reader_a.borrow_and_update().is_none());
        assert_eq!(
            reader_b.borrow_and_update().as_ref().unwrap().1.position,
            (56, 78)
        );
        while first
            .cursor(replaced.id, replaced.generation)
            .unwrap()
            .is_none()
        {
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(2));
        }
        assert_eq!(
            first
                .cursor(replaced.id, replaced.generation)
                .unwrap()
                .unwrap()
                .position,
            (90, 91)
        );
        host.shutdown();
        first.shutdown();
        second.shutdown();
        assert!(
            delivered_a.is_some(),
            "peer A sample overwritten by peer B before its sender reads"
        );
    }
}
