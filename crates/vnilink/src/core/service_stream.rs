//! Versioned, independently owned service pipes on the existing peer link.
use super::*;
use crate::protocol::SessionStamp;
use crate::{fs, CloseCause, ServiceInfo, ServiceKind, ServiceLease, ServicePhase, ServiceSupport};
use iroh::endpoint::{Connection, RecvStream, SendStream};

pub(super) const TAG_SERVICE: u8 = 8;
pub(super) const VERSION: u8 = 1;
const OPEN: u8 = 1;
const PROBE: u8 = 0;
const MAX_FRAME: usize = super::fs::MAX_FS_FRAME;
const TIMEOUT: Duration = Duration::from_secs(5);
/// How long a Files caller waits for its answer, queue included. A slow
/// answer is that request's timeout, not the pipe's end: a transfer on the
/// same path, or a busy disk on the host, can make one listing slow.
#[cfg(not(test))]
const PATIENCE: Duration = Duration::from_secs(30);
#[cfg(test)]
const PATIENCE: Duration = Duration::from_secs(1);
/// A delete walks the whole tree on the host before it answers.
#[cfg(not(test))]
const DELETE_PATIENCE: Duration = Duration::from_secs(120);
#[cfg(test)]
const DELETE_PATIENCE: Duration = Duration::from_secs(2);
/// An exchange still unanswered after this means the host is gone. Until
/// then the pipe reads a late answer to the end, so the next request's
/// answer is never mistaken for it.
#[cfg(not(test))]
const STUCK: Duration = Duration::from_secs(150);
#[cfg(test)]
const STUCK: Duration = Duration::from_secs(3);

fn patience(request: &fs::Request) -> Duration {
    match request {
        fs::Request::Delete { .. } => DELETE_PATIENCE,
        _ => PATIENCE,
    }
}

fn encode(bytes: &Vec<u8>) -> Result<Vec<u8>, Error> {
    let mut out = (bytes.len() as u32).to_be_bytes().to_vec();
    out.extend_from_slice(bytes);
    Ok(out)
}
pub(super) async fn write(send: &mut SendStream, bytes: Vec<u8>) -> Result<(), Error> {
    crate::sidestream::write_frame(send, &bytes, encode, "the service frame was not written").await
}
pub(super) async fn read(recv: &mut RecvStream) -> Result<Vec<u8>, Error> {
    crate::sidestream::read_frame(
        recv,
        |len| {
            if len == 0 || len > MAX_FRAME {
                Err(Error::Malformed)
            } else {
                Ok(())
            }
        },
        |b| Ok(b.to_vec()),
    )
    .await
}
pub(super) fn preamble(stamp: SessionStamp, id: u64, op: u8, kind: u8) -> Vec<u8> {
    let mut bytes = vec![TAG_SERVICE, VERSION, op];
    bytes.extend_from_slice(&stamp.id.to_be_bytes());
    bytes.extend_from_slice(&stamp.generation.to_be_bytes());
    bytes.extend_from_slice(&id.to_be_bytes());
    bytes.push(kind);
    bytes
}
pub(super) fn status(bytes: &[u8]) -> Result<u8, Error> {
    if bytes.len() != 3 || bytes[0] != VERSION {
        return Err(Error::IncompatibleVersion);
    }
    match bytes[1] {
        0 => Ok(bytes[2]),
        1 => Err(Error::Unsupported("remote service")),
        2 => Err(Error::TransferDenied),
        3 => Err(Error::StaleSession),
        4 => Err(Error::Busy),
        6 => Err(Error::Unauthorized),
        7 => Err(Error::NoSession),
        _ => Err(Error::IncompatibleVersion),
    }
}

struct Work {
    request: fs::Request,
    deadline: std::time::Instant,
    reply: std::sync::mpsc::SyncSender<Result<super::fs::FsOutcome, Error>>,
}

/// One file-service pipe. Calls are serialized through a bounded queue.
/// Drop/close cancels only this pipe; the parent peer and sibling services live.
pub struct RemoteFilesService {
    tx: tokio::sync::mpsc::Sender<Work>,
    lease: ServiceLease,
    stop: tokio::sync::watch::Sender<bool>,
    /// The connection and wire generation waits are made on, beside the pipe.
    link: Connection,
    generation: u64,
    runtime: tokio::runtime::Handle,
    /// Which watch is current; a new one, or none, ends the one before.
    watching: Arc<AtomicU64>,
    /// Bumped each time the watched folder changes.
    folder_changes: Arc<AtomicU64>,
}
impl RemoteFilesService {
    fn request(&self, request: fs::Request) -> Result<super::fs::FsOutcome, Error> {
        if self.lease.is_cancelled() {
            return Err(Error::NoSession);
        }
        let (reply, rx) = std::sync::mpsc::sync_channel(1);
        let patience = patience(&request);
        self.tx
            .try_send(Work {
                request,
                reply,
                deadline: std::time::Instant::now() + patience,
            })
            .map_err(|e| match e {
                tokio::sync::mpsc::error::TrySendError::Full(_) => Error::Busy,
                _ => Error::NoSession,
            })?;
        let result = rx.recv_timeout(patience).map_err(|error| match error {
            std::sync::mpsc::RecvTimeoutError::Timeout => Error::Timeout,
            std::sync::mpsc::RecvTimeoutError::Disconnected => Error::NoSession,
        })?;
        if self.lease.is_cancelled() {
            return Err(Error::NoSession);
        }
        result
    }
    pub fn info(&self) -> &ServiceInfo {
        self.lease.info()
    }
    pub fn is_cancelled(&self) -> bool {
        self.lease.is_cancelled()
    }
    pub fn close(&self) {
        self.stop.send_replace(true);
    }

    /// Watches one folder on the other computer, or none: each change to
    /// its entries bumps [`Self::folder_revision`]. Waits run one after
    /// another on streams of their own, each from the last fingerprint, so
    /// the pipe is never held and no change between them is missed.
    pub fn watch(&self, folder: Option<(u64, String)>) {
        let token = self.watching.fetch_add(1, Ordering::AcqRel) + 1;
        let Some((root, path)) = folder else {
            return;
        };
        let link = self.link.clone();
        let generation = self.generation;
        let watching = self.watching.clone();
        let changes = self.folder_changes.clone();
        let stop = self.stop.subscribe();
        self.runtime.spawn(async move {
            let mut since = None;
            while watching.load(Ordering::Acquire) == token
                && !*stop.borrow()
                && link.close_reason().is_none()
            {
                match super::fs::wait_for_change(&link, generation, root, &path, since.unwrap_or(0))
                    .await
                {
                    Ok(now) => {
                        if since.is_some_and(|before| before != now)
                            && watching.load(Ordering::Acquire) == token
                        {
                            changes.fetch_add(1, Ordering::AcqRel);
                        }
                        since = Some(now);
                    }
                    // A folder that went, or a refusal, is tried again calmly.
                    Err(_) => tokio::time::sleep(Duration::from_secs(2)).await,
                }
            }
        });
    }

    /// How many times the watched folder has changed; compare with the last read.
    pub fn folder_revision(&self) -> u64 {
        self.folder_changes.load(Ordering::Acquire)
    }
    pub fn list(
        &self,
        root: u64,
        path: &str,
        offset: u64,
        limit: u16,
    ) -> Result<fs::DirPage, Error> {
        match self.request(fs::Request::List {
            id: 1,
            generation: 0,
            root,
            path: path.into(),
            offset,
            limit,
        })? {
            super::fs::FsOutcome::Page(page) => Ok(page),
            _ => Err(Error::Malformed),
        }
    }
    /// The host's roots, home folder and usual folders.
    pub fn places(&self) -> Result<fs::Places, Error> {
        match self.request(fs::Request::Places {
            id: 1,
            generation: 0,
        })? {
            super::fs::FsOutcome::Places(places) => Ok(places),
            _ => Err(Error::Malformed),
        }
    }
    pub fn mkdir(&self, root: u64, path: &str, name: &str) -> Result<(), Error> {
        self.request(fs::Request::Mkdir {
            id: 1,
            generation: 0,
            root,
            path: path.into(),
            name: name.into(),
        })
        .map(|_| ())
    }
    pub fn rename(
        &self,
        root: u64,
        path: &str,
        name: &str,
        expected: Option<fs::EntryToken>,
        new_name: &str,
    ) -> Result<(), Error> {
        self.request(fs::Request::Rename {
            id: 1,
            generation: 0,
            root,
            path: path.into(),
            name: name.into(),
            expected,
            new_name: new_name.into(),
        })
        .map(|_| ())
    }
    /// Deletes an entry on the other computer: to its Trash when `trash`.
    pub fn delete(
        &self,
        root: u64,
        path: &str,
        name: &str,
        expected: Option<fs::EntryToken>,
        trash: bool,
    ) -> Result<(), Error> {
        self.request(fs::Request::Delete {
            id: 1,
            generation: 0,
            root,
            path: path.into(),
            name: name.into(),
            expected,
            trash,
        })
        .map(|_| ())
    }
}

impl Core {
    #[cfg(test)]
    pub(crate) fn test_service_frame(
        &self,
        id: u64,
        generation: u64,
        bytes: Vec<u8>,
    ) -> Result<Vec<u8>, Error> {
        let (link, _) = self.service_link(id, generation)?;
        self.runtime.block_on(async {
            let (mut send, mut recv) = link.open_bi().await.map_err(Error::network_debug)?;
            write(&mut send, bytes).await?;
            send.finish().map_err(Error::network_debug)?;
            tokio::time::timeout(TIMEOUT, read(&mut recv))
                .await
                .map_err(|_| Error::Timeout)?
        })
    }
    pub fn hosted_services(&self) -> Result<Vec<ServiceInfo>, Error> {
        let state = self.alive_state()?;
        let mut records: Vec<_> = state
            .hosted_services
            .values()
            .map(|r| r.info.clone())
            .collect();
        records.sort_by_key(|r| r.id);
        Ok(records)
    }
    /// Negotiates the service-stream protocol with the actual host. Old hosts
    /// do not implement this tag and are refused; the existing link is kept.
    pub fn remote_service_support(
        &self,
        id: u64,
        generation: u64,
    ) -> Result<ServiceSupport, Error> {
        let (link, stamp) = self.service_link(id, generation)?;
        let flags = self.runtime.block_on(async {
            tokio::time::timeout(TIMEOUT, async {
                let (mut send, mut recv) = link.open_bi().await.map_err(Error::network_debug)?;
                write(&mut send, preamble(stamp, 0, PROBE, 0)).await?;
                send.finish().map_err(Error::network_debug)?;
                status(&read(&mut recv).await?)
            })
            .await
            .map_err(|_| Error::Timeout)?
        })?;
        Ok(ServiceSupport {
            desktop: flags & 1 != 0,
            files: flags & 2 != 0,
            terminal: flags & 4 != 0,
            camera: flags & 8 != 0,
            system_audio: flags & 16 != 0,
            microphone: flags & 32 != 0,
            system_actions: flags & 64 != 0,
        })
    }
    pub fn close_hosted_service(&self, id: u64, generation: u64) -> Result<(), Error> {
        let mut state = self.alive_state()?;
        let record = state.hosted_services.get_mut(&id).ok_or(Error::NoSession)?;
        if record.info.generation != generation {
            return Err(Error::StaleSession);
        }
        if record.change(generation, ServicePhase::Closed(CloseCause::OwnerClose)) {
            self.shared.events.publish(CoreEvent::HostedServiceChanged {
                service: record.info.clone(),
            });
        }
        super::desktop_service::retire_owner(&mut state, id, generation, true);
        drop(state);
        super::service_jobs::interrupt_orphaned(&self.shared);
        Ok(())
    }
    pub(super) fn service_link(
        &self,
        id: u64,
        generation: u64,
    ) -> Result<(Connection, SessionStamp), Error> {
        let state = self.alive_state()?;
        let session = live_session(&state, false, id)?;
        if session.generation != generation {
            return Err(Error::StaleSession);
        }
        Ok((
            session.link.clone().ok_or(Error::NoSession)?,
            session.wire_stamp,
        ))
    }
    /// Opens a Files backend on its own stream and task. The local owner must
    /// already exist; host admission re-checks Transfer and wire generation.
    pub fn open_files_service(
        &self,
        id: u64,
        generation: u64,
    ) -> Result<RemoteFilesService, Error> {
        let mut lease = self.service_lease(id, generation)?;
        if lease.info().kind != ServiceKind::Files {
            return Err(Error::InvalidSetting("this is not a Files service"));
        }
        let (link, stamp) = self.service_link(lease.info().connection_id, generation)?;
        let (watch_link, watch_generation) = (link.clone(), stamp.generation);
        let stop = self
            .alive_state()?
            .services
            .get(&id)
            .ok_or(Error::NoSession)?
            .stop
            .clone();
        let (tx, mut rx) = tokio::sync::mpsc::channel::<Work>(4);
        let (ready, result) = std::sync::mpsc::sync_channel(1);
        let shared = self.shared.clone();
        self.runtime.spawn(async move {
            let connection = lease
                .run(async {
                    tokio::time::timeout(TIMEOUT, async {
                        let (mut send, mut recv) =
                            link.open_bi().await.map_err(Error::network_debug)?;
                        write(&mut send, preamble(stamp, id, OPEN, 2)).await?;
                        status(&read(&mut recv).await?)?;
                        Ok((send, recv))
                    })
                    .await
                    .map_err(|_| Error::Timeout)?
                })
                .await;
            let (mut send, mut recv) = match connection {
                Ok(pair) => {
                    let _ = ready.send(Ok(()));
                    pair
                }
                Err(error) => {
                    let _ = ready.send(Err(error));
                    return;
                }
            };
            // Lease cancellation and handle abandonment are local-owner ends;
            // unexpected EOF or a fatal exchange error means the far end is
            // gone. The tail below turns this into the close cause.
            let mut owner_done = false;
            loop {
                let mut unexpected = [0u8; 1];
                let work = tokio::select! {
                    biased;
                    _ = lease.cancelled() => {
                        owner_done = true;
                        break;
                    }
                    _ = recv.read(&mut unexpected) => break,
                    work = rx.recv() => match work {
                        Some(work) => work,
                        None => {
                            owner_done = true;
                            break;
                        }
                    },
                };
                // Its caller has stopped waiting; nothing is sent for it.
                if std::time::Instant::now() >= work.deadline {
                    let _ = work.reply.send(Err(Error::Timeout));
                    continue;
                }
                let outcome = lease
                    .run(async {
                        tokio::time::timeout(
                            STUCK,
                            exchange(&mut send, &mut recv, stamp, work.request),
                        )
                        .await
                        .map_err(|_| Error::Timeout)?
                    })
                    .await;
                let fatal = matches!(
                    outcome,
                    Err(Error::Network(_)
                        | Error::Timeout
                        | Error::Malformed
                        | Error::StaleSession
                        | Error::NoSession)
                );
                let _ = work.reply.send(outcome);
                if fatal {
                    break;
                }
            }
            // Closing the pipe produces EOF remotely and closes only that host
            // backend. There is no connection-wide close here.
            let _ = send.finish();
            let cause = if lease.is_cancelled() || owner_done {
                CloseCause::OwnerClose
            } else {
                CloseCause::PeerClose
            };
            let mut state = lock(&shared.state);
            if let Some(record) = state.services.get_mut(&id) {
                if link.close_reason().is_none()
                    && record.info.generation == generation
                    && record.info.phase == ServicePhase::Active
                    && record.change(generation, ServicePhase::Closed(cause))
                {
                    shared.events.publish(CoreEvent::ServiceChanged {
                        service: record.info.clone(),
                    });
                }
            }
            drop(state);
            super::service_jobs::interrupt_orphaned(&shared);
        });
        result
            .recv_timeout(TIMEOUT + Duration::from_secs(1))
            .map_err(|_| Error::Timeout)??;
        Ok(RemoteFilesService {
            tx,
            lease: self.service_lease(id, generation)?,
            stop,
            link: watch_link,
            generation: watch_generation,
            runtime: self.runtime.handle().clone(),
            watching: Arc::new(AtomicU64::new(0)),
            folder_changes: Arc::new(AtomicU64::new(0)),
        })
    }

    /// Adopts desktop input/media already negotiated by this connection.
    /// Neutral connections cannot start Desktop through this bridge.
    pub fn open_desktop_service(
        &self,
        id: u64,
        generation: u64,
    ) -> Result<RemoteDesktopService, Error> {
        let neutral = {
            let state = self.alive_state()?;
            state
                .services
                .get(&id)
                .and_then(|r| state.outgoing.get(&r.info.connection_id))
                .is_some_and(|c| c.services_only)
        };
        if neutral {
            return super::desktop_stream::open(self, id, generation);
        }
        let mut lease = self.service_lease(id, generation)?;
        if lease.info().kind != ServiceKind::Desktop {
            return Err(Error::InvalidSetting("this is not a Desktop service"));
        }
        let connection_id = lease.info().connection_id;
        let (link, stamp) = self.service_link(connection_id, generation)?;
        let stop = {
            let mut state = self.alive_state()?;
            let session = state
                .outgoing
                .get_mut(&connection_id)
                .ok_or(Error::NoSession)?;
            if session.generation != generation {
                return Err(Error::StaleSession);
            }
            if session.files_only || *session.desktop_stop.borrow() {
                return Err(Error::Unsupported("desktop service on this connection"));
            }
            if session.desktop_owner.is_some() {
                return Err(Error::Busy);
            }
            session.desktop_owner = Some(id);
            state
                .services
                .get(&id)
                .ok_or(Error::NoSession)?
                .stop
                .clone()
        };
        let (ready, result) = std::sync::mpsc::sync_channel(1);
        let shared = self.shared.clone();
        self.runtime.spawn(async move {
            let opened = lease
                .run(async {
                    tokio::time::timeout(TIMEOUT, async {
                        let (mut send, mut recv) =
                            link.open_bi().await.map_err(Error::network_debug)?;
                        write(&mut send, preamble(stamp, id, OPEN, 1)).await?;
                        status(&read(&mut recv).await?)?;
                        Ok((send, recv))
                    })
                    .await
                    .map_err(|_| Error::Timeout)?
                })
                .await;
            match opened {
                Ok((mut send, mut recv)) => {
                    let _ = ready.send(Ok(()));
                    let mut unexpected = [0u8; 1];
                    tokio::select! {
                        biased;
                        _ = lease.cancelled() => {},
                        _ = recv.read(&mut unexpected) => {},
                    }
                    let _ = send.finish();
                    // A lost parent preserves the logical owner for reconnect.
                    super::desktop_service::finish_controller(
                        &shared,
                        id,
                        generation,
                        link.close_reason().is_none(),
                        if lease.is_cancelled() {
                            CloseCause::OwnerClose
                        } else {
                            CloseCause::PeerClose
                        },
                    );
                }
                Err(error) => {
                    if matches!(
                        error,
                        Error::Unsupported(_)
                            | Error::Unauthorized
                            | Error::Busy
                            | Error::IncompatibleVersion
                    ) {
                        let mut state = lock(&shared.state);
                        if let Some(session) = state
                            .outgoing
                            .get_mut(&connection_id)
                            .filter(|s| s.generation == generation && s.desktop_owner == Some(id))
                        {
                            session.desktop_owner = None;
                        }
                    } else {
                        // Admission may have committed before the reply was lost.
                        super::desktop_service::finish_controller(
                            &shared,
                            id,
                            generation,
                            link.close_reason().is_none(),
                            if lease.is_cancelled() {
                                CloseCause::OwnerClose
                            } else {
                                CloseCause::PeerClose
                            },
                        );
                    }
                    let _ = ready.send(Err(error));
                }
            }
        });
        result
            .recv_timeout(TIMEOUT + Duration::from_secs(1))
            .map_err(|_| Error::Timeout)??;
        Ok(RemoteDesktopService {
            lease: self.service_lease(id, generation)?,
            stop,
            shared: Arc::downgrade(&self.shared),
            desktop: None,
        })
    }
}

async fn exchange(
    send: &mut SendStream,
    recv: &mut RecvStream,
    stamp: SessionStamp,
    mut request: fs::Request,
) -> Result<super::fs::FsOutcome, Error> {
    match &mut request {
        fs::Request::List { generation, .. }
        | fs::Request::Mkdir { generation, .. }
        | fs::Request::Rename { generation, .. }
        | fs::Request::Delete { generation, .. }
        | fs::Request::Places { generation, .. } => *generation = stamp.generation,
        // A wait holds its stream; it has one of its own, never this pipe.
        fs::Request::Watch { .. } => {
            return Err(Error::InvalidSetting("a wait has a stream of its own"))
        }
    }
    let list = matches!(request, fs::Request::List { .. });
    let places = matches!(request, fs::Request::Places { .. });
    write(send, fs::encode_request(&request)?).await?;
    let response = fs::decode_response(&read(recv).await?).map_err(|e| e.to_error())?;
    if response.id != 1 {
        return Err(Error::Malformed);
    }
    super::fs::status_error(response.status, &response.message)?;
    if list {
        Ok(super::fs::FsOutcome::Page(
            fs::decode_list_body(&read(recv).await?).map_err(|e| e.to_error())?,
        ))
    } else if places {
        Ok(super::fs::FsOutcome::Places(
            fs::decode_places_body(&read(recv).await?).map_err(|e| e.to_error())?,
        ))
    } else {
        Ok(super::fs::FsOutcome::Done)
    }
}

pub(super) struct HostedGuard {
    pub(super) shared: Arc<Shared>,
    pub(super) id: u64,
}
impl Drop for HostedGuard {
    fn drop(&mut self) {
        let mut state = lock(&self.shared.state);
        if let Some(mut record) = state.hosted_services.remove(&self.id) {
            super::desktop_service::retire_owner(&mut state, self.id, record.info.generation, true);
            record.stop.send_replace(true);
            if !record.info.phase.finished() {
                // Backstop for a serve task that ended without an explicit
                // close. A live parent means the backend or loop itself died.
                let cause = if state.shut_down {
                    CloseCause::Shutdown
                } else {
                    match state.incoming.get(&record.info.connection_id) {
                        Some(session) if session.phase == SessionPhase::Active => {
                            CloseCause::DeviceLoss
                        }
                        _ => CloseCause::ParentLoss,
                    }
                };
                record.info.phase = ServicePhase::Closed(cause);
                self.shared.events.publish(CoreEvent::HostedServiceChanged {
                    service: record.info,
                });
            }
        }
        drop(state);
        super::service_jobs::interrupt_orphaned(&self.shared);
    }
}

pub(super) async fn serve(
    shared: Arc<Shared>,
    connection_id: u64,
    peer: [u8; 32],
    mut send: SendStream,
    mut recv: RecvStream,
    body: &[u8],
    mut parent_stop: tokio::sync::watch::Receiver<bool>,
) -> Result<(), Error> {
    // body excludes the dispatcher tag.
    if body.len() != 27 {
        return Err(Error::Malformed);
    }
    if body[0] != VERSION {
        write(&mut send, vec![VERSION, 5, 0]).await?;
        return Ok(());
    }
    let stamp = SessionStamp::new(
        u64::from_be_bytes(body[2..10].try_into().unwrap()),
        u64::from_be_bytes(body[10..18].try_into().unwrap()),
    );
    let remote_id = u64::from_be_bytes(body[18..26].try_into().unwrap());
    let kind = body[26];
    let service_kind = match kind {
        1 => Some(ServiceKind::Desktop),
        2 => Some(ServiceKind::Files),
        3 => Some(ServiceKind::Terminal),
        4 => Some(ServiceKind::Camera),
        5 => Some(ServiceKind::SystemAudio),
        6 => Some(ServiceKind::Microphone),
        7 => Some(ServiceKind::System),
        8 => Some(ServiceKind::ScreenSnapshot),
        _ => None,
    };
    if matches!(body[1], 3 | 4) && remote_id == 0 && kind == 3 {
        {
            let state = lock(&shared.state);
            let session = live_session(&state, true, connection_id)?;
            if session.wire_stamp != stamp || session.peer_key != peer {
                return Err(Error::StaleSession);
            }
            if !session.services_only {
                return Err(Error::Unsupported("terminal registry"));
            }
        }
        return super::terminal_stream::registry(
            shared,
            peer,
            body[1] == 4,
            send,
            recv,
            parent_stop,
        )
        .await;
    }
    if body[1] == PROBE && remote_id == 0 && kind == crate::snapshot::WIRE_KIND {
        return super::snapshot::probe(shared, connection_id, peer, send, stamp).await;
    }
    if body[1] == 2 && remote_id == 0 && matches!(kind, 4 | 5 | 6) {
        let supported = {
            let state = lock(&shared.state);
            let session = live_session(&state, true, connection_id)?;
            if session.wire_stamp != stamp || session.peer_key != peer {
                return Err(Error::StaleSession);
            }
            session.services_only
        };
        if !supported {
            write(&mut send, vec![VERSION, 1, 0]).await?;
            return Ok(());
        }
        return if kind == 4 {
            super::camera_stream::list(shared, peer, send, parent_stop).await
        } else {
            super::audio_stream::list(
                shared,
                peer,
                if kind == 5 {
                    crate::audio::AudioKind::System
                } else {
                    crate::audio::AudioKind::Microphone
                },
                send,
                parent_stop,
            )
            .await
        };
    }
    let outcome = {
        let mut state = lock(&shared.state);
        let session = live_session(&state, true, connection_id)?;
        if session.wire_stamp != stamp || session.peer_key != peer {
            Err(3)
        } else if body[1] == PROBE && remote_id == 0 && kind == 0 {
            Ok(None)
        } else if body[1] != OPEN || remote_id == 0 {
            Err(5)
        } else if service_kind.is_none() {
            Err(1)
        } else if !super::service_permissions::admitted(&state, &peer, service_kind.unwrap()) {
            Err(if kind == 2 { 2 } else { 6 })
        } else if kind != 2
            && !(kind == 4 && session.services_only && lock(&shared.camera_factory).is_some())
            && !(matches!(kind, 5 | 6)
                && session.services_only
                && lock(&shared.audio_factory).as_ref().is_some_and(|f| {
                    f.supports(if kind == 5 {
                        crate::audio::AudioKind::System
                    } else {
                        crate::audio::AudioKind::Microphone
                    })
                }))
            && !(kind == 3 && session.services_only && lock(&shared.terminal_factory).is_some())
            && !(kind == 7 && session.services_only)
            && !(kind == 1 && !session.files_only && !*session.desktop_stop.borrow())
            && !(kind == 8
                && (session.services_only || !session.files_only)
                && lock(&shared.snapshot_factory).is_some())
        {
            Err(1)
        } else if state.hosted_services.len() >= 16
            || state.hosted_services.values().any(|r| {
                r.info.connection_id == connection_id
                    && (Some(r.info.kind) == service_kind && kind != 3
                        || r.remote_id == Some(remote_id))
            })
        {
            Err(4)
        } else if kind == 8
            && !super::snapshot::snapshot_room(&state, &peer, std::time::Instant::now())
        {
            Err(4)
        } else {
            let peer_id = session.peer_id.clone();
            let id = state.next_service_id;
            state.next_service_id = id.checked_add(1).ok_or(Error::Busy)?;
            let info = ServiceInfo {
                id,
                connection_id,
                generation: stamp.generation,
                peer_id,
                kind: service_kind.unwrap(),
                phase: ServicePhase::Active,
            };
            state.hosted_services.insert(
                id,
                super::services::ServiceRecord {
                    info: info.clone(),
                    remote_id: Some(remote_id),
                    stream_open: false,
                    stop: tokio::sync::watch::channel(false).0,
                    desktop: None,
                },
            );
            if kind == 1 {
                state
                    .incoming
                    .get_mut(&connection_id)
                    .ok_or(Error::NoSession)?
                    .desktop_owner = Some(id);
            }
            if kind == 8 {
                super::snapshot::note_snapshot(&mut state, &peer, std::time::Instant::now());
            }
            shared
                .events
                .publish(CoreEvent::HostedServiceChanged { service: info });
            Ok(Some(id))
        }
    };
    let id = match outcome {
        Err(status) => {
            write(&mut send, vec![VERSION, status, 0]).await?;
            return Ok(());
        }
        Ok(None) => {
            let desktop = lock(&shared.state)
                .incoming
                .get(&connection_id)
                .is_some_and(|s| {
                    (s.services_only
                        && lock(&shared.video_caps).can_encode()
                        && monitor_size(&shared) != (0, 0))
                        || (!s.files_only && !*s.desktop_stop.borrow())
                });
            let terminal = lock(&shared.state)
                .incoming
                .get(&connection_id)
                .is_some_and(|s| s.services_only)
                && lock(&shared.terminal_factory).is_some();
            let camera = lock(&shared.state)
                .incoming
                .get(&connection_id)
                .is_some_and(|s| s.services_only)
                && lock(&shared.camera_factory).is_some();
            let audio = lock(&shared.state)
                .incoming
                .get(&connection_id)
                .is_some_and(|s| s.services_only);
            let factory = lock(&shared.audio_factory).clone();
            let system_audio = audio
                && factory
                    .as_ref()
                    .is_some_and(|f| f.supports(crate::audio::AudioKind::System));
            let microphone = audio
                && factory
                    .as_ref()
                    .is_some_and(|f| f.supports(crate::audio::AudioKind::Microphone));
            let system_actions = lock(&shared.state)
                .incoming
                .get(&connection_id)
                .is_some_and(|s| s.services_only)
                && lock(&shared.system_actions).is_some();
            write(
                &mut send,
                vec![
                    VERSION,
                    0,
                    2 | u8::from(desktop)
                        | if terminal { 4 } else { 0 }
                        | if camera { 8 } else { 0 }
                        | if system_audio { 16 } else { 0 }
                        | if microphone { 32 } else { 0 }
                        | if system_actions { 64 } else { 0 },
                ],
            )
            .await?;
            return Ok(());
        }
        Ok(Some(id)) => id,
    };
    let _owner = HostedGuard {
        shared: shared.clone(),
        id,
    };
    let mut service_stop = lock(&shared.state)
        .hosted_services
        .get(&id)
        .ok_or(Error::NoSession)?
        .stop
        .subscribe();
    if matches!(kind, 5 | 6) {
        return super::audio_stream::serve(
            shared,
            id,
            if kind == 5 {
                crate::audio::AudioKind::System
            } else {
                crate::audio::AudioKind::Microphone
            },
            send,
            recv,
            parent_stop,
            service_stop,
        )
        .await;
    }
    if kind == 4 {
        return super::camera_stream::serve(shared, id, send, recv, parent_stop, service_stop)
            .await;
    }
    if kind == 3 {
        return super::terminal_stream::serve(shared, id, send, recv, parent_stop, service_stop)
            .await;
    }
    if kind == 7 {
        return super::system_stream::serve(shared, id, send, recv, parent_stop, service_stop)
            .await;
    }
    if kind == 8 {
        return super::snapshot::serve(shared, peer, send, recv, parent_stop, service_stop).await;
    }
    write(&mut send, vec![VERSION, 0, 2]).await?;
    if kind == 1 {
        // This stream is the ownership lease. Input/media retain the existing
        // negotiated transports; no filesystem or payload frames belong here.
        let mut unexpected = [0u8; 1];
        if !*parent_stop.borrow() && !*service_stop.borrow() {
            tokio::select! {
                biased;
                _ = parent_stop.changed() => {},
                _ = service_stop.changed() => {},
                _ = recv.read(&mut unexpected) => {},
            }
        }
        let _ = send.finish();
        return Ok(());
    }
    loop {
        if *parent_stop.borrow() || *service_stop.borrow() {
            break;
        }
        let request = tokio::select! {
            _ = parent_stop.changed() => break,
            _ = service_stop.changed() => break,
            request = read(&mut recv) => match request { Ok(request) => request, Err(_) => break },
        };
        // The existing handler checks current peer, generation, Transfer grant
        // and approved roots at each operation, under the mutation locks.
        tokio::select! {
            biased;
            _ = parent_stop.changed() => break,
            _ = service_stop.changed() => break,
            result = super::fs::handle_fs_request(&shared, connection_id, &peer, &mut send, &request) => result.map_err(|_| Error::Malformed)?,
        }
    }
    let _ = send.finish();
    Ok(())
}
