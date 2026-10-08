//! Versioned, independently owned service pipes on the existing peer link.
use super::*;
use crate::protocol::SessionStamp;
use crate::{fs, ServiceInfo, ServiceKind, ServiceLease, ServicePhase, ServiceSupport};
use iroh::endpoint::{Connection, RecvStream, SendStream};

pub(super) const TAG_SERVICE: u8 = 8;
const VERSION: u8 = 1;
const OPEN: u8 = 1;
const PROBE: u8 = 0;
const MAX_FRAME: usize = super::fs::MAX_FS_FRAME;
const TIMEOUT: Duration = Duration::from_secs(5);

fn encode(bytes: &Vec<u8>) -> Result<Vec<u8>, Error> {
    let mut out = (bytes.len() as u32).to_be_bytes().to_vec();
    out.extend_from_slice(bytes);
    Ok(out)
}
async fn write(send: &mut SendStream, bytes: Vec<u8>) -> Result<(), Error> {
    crate::sidestream::write_frame(send, &bytes, encode, "the service frame was not written").await
}
async fn read(recv: &mut RecvStream) -> Result<Vec<u8>, Error> {
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
fn preamble(stamp: SessionStamp, id: u64, op: u8, kind: u8) -> Vec<u8> {
    let mut bytes = vec![TAG_SERVICE, VERSION, op];
    bytes.extend_from_slice(&stamp.id.to_be_bytes());
    bytes.extend_from_slice(&stamp.generation.to_be_bytes());
    bytes.extend_from_slice(&id.to_be_bytes());
    bytes.push(kind);
    bytes
}
fn status(bytes: &[u8]) -> Result<u8, Error> {
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
}
impl RemoteFilesService {
    fn request(&self, request: fs::Request) -> Result<super::fs::FsOutcome, Error> {
        if self.lease.is_cancelled() {
            return Err(Error::NoSession);
        }
        let (reply, rx) = std::sync::mpsc::sync_channel(1);
        self.tx
            .try_send(Work {
                request,
                reply,
                deadline: std::time::Instant::now() + TIMEOUT,
            })
            .map_err(|e| match e {
                tokio::sync::mpsc::error::TrySendError::Full(_) => Error::Busy,
                _ => Error::NoSession,
            })?;
        let result =
            rx.recv_timeout(TIMEOUT + Duration::from_secs(1))
                .map_err(|error| match error {
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
        })
    }
    pub fn close_hosted_service(&self, id: u64, generation: u64) -> Result<(), Error> {
        let mut state = self.alive_state()?;
        let record = state.hosted_services.get_mut(&id).ok_or(Error::NoSession)?;
        if record.info.generation != generation {
            return Err(Error::StaleSession);
        }
        if record.change(generation, ServicePhase::Closed) {
            self.shared.events.publish(CoreEvent::HostedServiceChanged {
                service: record.info.clone(),
            });
        }
        super::desktop_service::retire_owner(&mut state, id, generation, true);
        drop(state);
        super::service_jobs::cancel_orphaned(&self.shared);
        Ok(())
    }
    fn service_link(&self, id: u64, generation: u64) -> Result<(Connection, SessionStamp), Error> {
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
            loop {
                let mut unexpected = [0u8; 1];
                let work = tokio::select! {
                    biased;
                    _ = lease.cancelled() => break,
                    _ = recv.read(&mut unexpected) => break,
                    work = rx.recv() => match work { Some(work) => work, None => break },
                };
                if std::time::Instant::now() >= work.deadline {
                    let _ = work.reply.send(Err(Error::Timeout));
                    continue;
                }
                let outcome = lease
                    .run(async {
                        tokio::time::timeout_at(
                            tokio::time::Instant::from_std(work.deadline),
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
            let mut state = lock(&shared.state);
            if let Some(record) = state.services.get_mut(&id) {
                if link.close_reason().is_none()
                    && record.info.generation == generation
                    && record.info.phase == ServicePhase::Active
                    && record.change(generation, ServicePhase::Closed)
                {
                    shared.events.publish(CoreEvent::ServiceChanged {
                        service: record.info.clone(),
                    });
                }
            }
            drop(state);
            super::service_jobs::cancel_orphaned(&shared);
        });
        result
            .recv_timeout(TIMEOUT + Duration::from_secs(1))
            .map_err(|_| Error::Timeout)??;
        Ok(RemoteFilesService {
            tx,
            lease: self.service_lease(id, generation)?,
            stop,
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
        | fs::Request::Rename { generation, .. } => *generation = stamp.generation,
    }
    let list = matches!(request, fs::Request::List { .. });
    write(send, fs::encode_request(&request)?).await?;
    let response = fs::decode_response(&read(recv).await?).map_err(|e| e.to_error())?;
    if response.id != 1 {
        return Err(Error::Malformed);
    }
    match response.status {
        0 => {}
        fs::STATUS_EXISTS => return Err(Error::FileExists),
        fs::STATUS_DENIED => return Err(Error::TransferDenied),
        fs::STATUS_STALE => return Err(Error::StaleSession),
        _ => {
            return Err(Error::InvalidSetting(
                "the remote filesystem refused the request",
            ))
        }
    }
    if list {
        Ok(super::fs::FsOutcome::Page(
            fs::decode_list_body(&read(recv).await?).map_err(|e| e.to_error())?,
        ))
    } else {
        Ok(super::fs::FsOutcome::Done)
    }
}

struct HostedGuard {
    shared: Arc<Shared>,
    id: u64,
}
impl Drop for HostedGuard {
    fn drop(&mut self) {
        let mut state = lock(&self.shared.state);
        if let Some(mut record) = state.hosted_services.remove(&self.id) {
            super::desktop_service::retire_owner(&mut state, self.id, record.info.generation, true);
            record.stop.send_replace(true);
            if record.info.phase != ServicePhase::Closed {
                record.info.phase = ServicePhase::Closed;
                self.shared.events.publish(CoreEvent::HostedServiceChanged {
                    service: record.info,
                });
            }
        }
        drop(state);
        super::service_jobs::cancel_orphaned(&self.shared);
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
        _ => None,
    };
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
            && !(kind == 1 && !session.files_only && !*session.desktop_stop.borrow())
        {
            Err(1)
        } else if state.hosted_services.len() >= 16
            || state
                .hosted_services
                .values()
                .any(|r| r.info.connection_id == connection_id && Some(r.info.kind) == service_kind)
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
                    stop: tokio::sync::watch::channel(false).0,
                    desktop: None,
                },
            );
            if kind == 1 {
                state
                    .incoming
                    .as_mut()
                    .ok_or(Error::NoSession)?
                    .desktop_owner = Some(id);
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
            let desktop = lock(&shared.state).incoming.as_ref().is_some_and(|s| {
                (s.services_only
                    && shared
                        .h264_encode
                        .load(std::sync::atomic::Ordering::Relaxed)
                    && monitor_size(&shared) != (0, 0))
                    || (!s.files_only && !*s.desktop_stop.borrow())
            });
            write(&mut send, vec![VERSION, 0, 2 | u8::from(desktop)]).await?;
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
