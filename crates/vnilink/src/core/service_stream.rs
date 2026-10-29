//! Versioned, independently owned service pipes on the existing peer link.
// trace: records every ignored failure
use super::*;
use crate::protocol::SessionStamp;
use crate::{fs, CloseCause, ServiceInfo, ServiceKind, ServiceLease, ServicePhase, ServiceSupport};
use iroh::endpoint::{Connection, RecvStream, SendStream};

pub(super) const TAG_SERVICE: u8 = 8;
pub(super) const VERSION: u8 = 1;
const OPEN: u8 = 1;
const PROBE: u8 = 0;
/// One wait on an open Files service's folder, on a stream of its own.
const WAIT: u8 = 5;
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
    /// The connection and wire stamp waits are made on, beside the pipe.
    link: Connection,
    stamp: SessionStamp,
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
        let stamp = self.stamp;
        let id = self.lease.info().id;
        let watching = self.watching.clone();
        let changes = self.folder_changes.clone();
        let stop = self.stop.subscribe();
        self.runtime.spawn(async move {
            let mut since = None;
            while watching.load(Ordering::Acquire) == token
                && !*stop.borrow()
                && link.close_reason().is_none()
            {
                match wait_for_change(&link, stamp, id, root, &path, since.unwrap_or(0)).await {
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
        let _engine = self.engine();
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
        let _engine = self.engine();
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
        let _engine = self.engine();
        self.end_hosted_service(id, generation, CloseCause::OwnerClose)
    }
    /// Closes a hosted service because this computer's own producer for it
    /// failed. The viewer is told what failed; see [`crate::StreamEnd`].
    pub fn fail_hosted_service(
        &self,
        id: u64,
        generation: u64,
        failure: crate::StreamFailure,
    ) -> Result<(), Error> {
        let _engine = self.engine();
        self.end_hosted_service(id, generation, CloseCause::Failed(failure))
    }
    fn end_hosted_service(&self, id: u64, generation: u64, cause: CloseCause) -> Result<(), Error> {
        let mut state = self.alive_state()?;
        let record = state.hosted_services.get_mut(&id).ok_or(Error::NoSession)?;
        if record.info.generation != generation {
            return Err(Error::StaleSession);
        }
        if record.change(generation, ServicePhase::Closed(cause)) {
            self.shared.events.publish(CoreEvent::HostedServiceChanged {
                service: record.info.clone(),
            });
        }
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
        let _engine = self.engine();
        let mut lease = self.service_lease(id, generation)?;
        if lease.info().kind != ServiceKind::Files {
            return Err(Error::InvalidSetting("this is not a Files service"));
        }
        let (link, stamp) = self.service_link(lease.info().connection_id, generation)?;
        let (watch_link, watch_stamp) = (link.clone(), stamp);
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
                    crate::ignored!(
                        crate::trace::SERVICE,
                        "the caller stopped waiting for its answer",
                        ready.send(Ok(()))
                    );
                    pair
                }
                Err(error) => {
                    crate::ignored!(
                        crate::trace::SERVICE,
                        "the caller stopped waiting for its answer",
                        ready.send(Err(error))
                    );
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
                    crate::ignored!(
                        crate::trace::SERVICE,
                        "the caller stopped waiting for its answer",
                        work.reply.send(Err(Error::Timeout))
                    );
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
                crate::ignored!(
                    crate::trace::SERVICE,
                    "the caller stopped waiting for its answer",
                    work.reply.send(outcome)
                );
                if fatal {
                    break;
                }
            }
            // Closing the pipe produces EOF remotely and closes only that host
            // backend. There is no connection-wide close here.
            crate::ignored!(
                crate::trace::SERVICE,
                "a stream that cannot finish is closed with its link",
                send.finish()
            );
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
            stamp: watch_stamp,
            runtime: self.runtime.handle().clone(),
            watching: Arc::new(AtomicU64::new(0)),
            folder_changes: Arc::new(AtomicU64::new(0)),
        })
    }

    /// Opens a Desktop service's own stream on its services connection.
    pub fn open_desktop_service(
        &self,
        id: u64,
        generation: u64,
    ) -> Result<RemoteDesktopService, Error> {
        let _engine = self.engine();
        super::desktop_stream::open(self, id, generation)
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

/// Waits, on a stream of its own beside the Files pipe, for the other
/// computer's folder to differ from `since`, and returns its fingerprint
/// then. The host answers when it changes, or after a while unchanged.
async fn wait_for_change(
    link: &Connection,
    stamp: SessionStamp,
    id: u64,
    root: u64,
    path: &str,
    since: u64,
) -> Result<u64, Error> {
    let (mut send, mut recv) = link.open_bi().await.map_err(Error::network_debug)?;
    write(&mut send, preamble(stamp, id, WAIT, 2)).await?;
    status(&read(&mut recv).await?)?;
    let request = fs::Request::Watch {
        id: 1,
        generation: stamp.generation,
        root,
        path: path.to_owned(),
        since,
    };
    write(&mut send, fs::encode_request(&request)?).await?;
    let response = fs::decode_response(&read(&mut recv).await?).map_err(|e| e.to_error())?;
    if response.id != 1 {
        return Err(Error::Malformed);
    }
    super::fs::status_error(response.status, &response.message)?;
    let body = read(&mut recv).await?;
    crate::ignored!(
        crate::trace::SERVICE,
        "a stream that cannot finish is closed with its link",
        send.finish()
    );
    Ok(u64::from_be_bytes(
        body.try_into().map_err(|_| Error::Malformed)?,
    ))
}

/// The host's side of one wait. Admitted only while the Files service it
/// names is open on this connection and Files is still allowed, as the
/// Files pipe is; it ends with that service.
async fn serve_files_wait(
    shared: Arc<Shared>,
    connection_id: u64,
    peer: [u8; 32],
    stamp: SessionStamp,
    remote_id: u64,
    mut send: SendStream,
    mut recv: RecvStream,
) -> Result<(), Error> {
    let admitted = {
        let state = lock(&shared.state);
        let session = live_session(&state, true, connection_id)?;
        if session.wire_stamp != stamp || session.peer_key != peer {
            Err(3)
        } else if !super::service_permissions::admitted(&state, &peer, ServiceKind::Files) {
            Err(2)
        } else {
            state
                .hosted_services
                .values()
                .find(|record| {
                    record.info.connection_id == connection_id
                        && record.remote_id == Some(remote_id)
                        && record.info.kind == ServiceKind::Files
                        && record.info.phase == ServicePhase::Active
                })
                .map(|record| record.stop.subscribe())
                .ok_or(7)
        }
    };
    let mut stop = match admitted {
        Ok(stop) => stop,
        Err(code) => {
            write(&mut send, vec![VERSION, code, 2]).await?;
            return Ok(());
        }
    };
    write(&mut send, vec![VERSION, 0, 2]).await?;
    let request = tokio::time::timeout(TIMEOUT, read(&mut recv))
        .await
        .map_err(|_| Error::Timeout)??;
    let fs::Request::Watch {
        id,
        root,
        path,
        since,
        ..
    } = fs::decode_request(&request).map_err(|e| e.to_error())?
    else {
        return Err(Error::Malformed);
    };
    let roots = lock(&shared.file_roots).clone();
    let answer = tokio::select! {
        answer = super::watch::answer_wait(roots, root, path, since) => answer,
        _ = stop.changed() => return Ok(()),
    };
    match answer {
        Ok(fingerprint) => {
            write(&mut send, fs::encode_response(&fs::Response::ok(id))).await?;
            write(&mut send, fingerprint.to_be_bytes().to_vec()).await?;
        }
        Err(_) => {
            let refused = fs::Response::coded(id, fs::STATUS_ERROR);
            write(&mut send, fs::encode_response(&refused)).await?;
        }
    }
    crate::ignored!(
        crate::trace::SERVICE,
        "a stream that cannot finish is closed with its link",
        send.finish()
    );
    Ok(())
}

pub(super) struct HostedGuard {
    pub(super) shared: Arc<Shared>,
    pub(super) id: u64,
}
impl Drop for HostedGuard {
    fn drop(&mut self) {
        let mut state = lock(&self.shared.state);
        if let Some(mut record) = state.hosted_services.remove(&self.id) {
            record.stop.send_replace(true);
            if !record.info.phase.finished() {
                // Backstop for a serve task that ended without an explicit
                // close. A live parent means the backend or loop itself died.
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
                // A lost connection or shutdown reads as any service's
                // phase change; only a died backend is called out.
                if cause == CloseCause::DeviceLoss {
                    record.info.phase = ServicePhase::Closed(cause);
                    tracing::info!(
                        target: crate::trace::HOSTING,
                        service = self.id,
                        kind = ?record.info.kind,
                        phase = record.info.phase.label(),
                        "service ended without closing"
                    );
                } else {
                    record.change(record.info.generation, ServicePhase::Closed(cause));
                }
                self.shared.events.publish(CoreEvent::HostedServiceChanged {
                    service: record.info,
                });
            }
        }
        drop(state);
        tracing::debug!(target: crate::trace::HOSTING, service = self.id, "service released");
        super::service_jobs::interrupt_orphaned(&self.shared);
    }
}

/// Names why hosted service `id` closed, from the serve loop that knows: its
/// viewer left, or its work ended, as a shell that exited. Without it,
/// [`HostedGuard`] can only guess from a live connection that the backend
/// died. A cause set first stays, and a viewer's close counts only while its
/// connection lives: a lost connection ends the stream too, and is the
/// guard's to name.
pub(super) fn close_hosted(shared: &Shared, id: u64, cause: CloseCause) {
    settle_hosted(
        shared,
        id,
        ServicePhase::Closed(cause),
        cause == CloseCause::PeerClose,
    );
}

/// Names hosted service `id` failed for `reason`, as a refused open, when
/// its stream ended with that error on a live connection. A lost
/// connection fails the stream too, and is the guard's to name.
pub(super) fn fail_hosted(shared: &Shared, id: u64, reason: &str) {
    settle_hosted(shared, id, ServicePhase::Failed(reason.to_owned()), true);
}

/// Moves hosted service `id` to `phase` unless a phase was set first, the
/// engine is shutting down, or, when `on_live_link`, its connection is gone.
fn settle_hosted(shared: &Shared, id: u64, phase: ServicePhase, on_live_link: bool) {
    let mut state = lock(&shared.state);
    let Some(connection) = state
        .hosted_services
        .get(&id)
        .filter(|record| !record.info.phase.finished())
        .map(|record| record.info.connection_id)
    else {
        return;
    };
    let connected = state
        .incoming
        .get(&connection)
        .is_some_and(LiveSession::connected);
    if (on_live_link && !connected) || state.shut_down {
        return;
    }
    let Some(record) = state.hosted_services.get_mut(&id) else {
        return;
    };
    let failed = match &phase {
        ServicePhase::Failed(reason) => Some(reason.clone()),
        _ => None,
    };
    record.info.phase = phase;
    if let Some(reason) = failed {
        // The reason is an error's fixed name, never its text.
        tracing::info!(
            target: crate::trace::HOSTING,
            service = id,
            kind = ?record.info.kind,
            reason,
            "service failed"
        );
    } else {
        tracing::info!(
            target: crate::trace::HOSTING,
            service = id,
            kind = ?record.info.kind,
            phase = record.info.phase.label(),
            "service closed"
        );
    }
    let service = record.info.clone();
    drop(state);
    shared
        .events
        .publish(CoreEvent::HostedServiceChanged { service });
}

/// How a hosted service's stream ended, as its serve loop saw it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum HostedEnd {
    /// The engine dropped the serve task before it said, as at teardown.
    Dropped,
    /// The viewer closed it, finished its stream or stopped reading ours.
    ViewerClosed,
    /// The connection carrying it ended.
    ConnectionEnded,
    /// This computer's engine shut down, which ends its connections and
    /// drops its tasks, whichever reaches the stream first.
    ShutDown,
    /// Its service was closed by something else, as a revoke.
    ServiceClosed,
    /// Its work ended on this computer, in the service's words (`shell
    /// exited`); this computer closes it.
    Finished(&'static str),
    /// Its backend failed, in the service's words (`capture failed`).
    Failed(&'static str),
    Malformed,
}

impl HostedEnd {
    /// How a stream whose service was closed ended: by its viewer when the
    /// peer closed the service, which also ends the stream; by this engine's
    /// shutdown, which the viewer learns as a lost connection, not a
    /// withdrawal; otherwise by whatever closed it.
    pub(super) fn service_closed(shared: &Shared, id: u64) -> Self {
        let phase = lock(&shared.state)
            .hosted_services
            .get(&id)
            .map(|record| record.info.phase.clone());
        match phase {
            Some(ServicePhase::Closed(CloseCause::PeerClose)) => Self::ViewerClosed,
            Some(ServicePhase::Closed(CloseCause::Shutdown)) => Self::ShutDown,
            _ => Self::ServiceClosed,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Dropped => "dropped",
            Self::ViewerClosed => "viewer closed",
            Self::ConnectionEnded => "connection ended",
            Self::ShutDown => "shut down",
            Self::ServiceClosed => "service closed",
            Self::Finished(words) | Self::Failed(words) => words,
            Self::Malformed => "malformed",
        }
    }

    /// The close cause this end names, where the serve loop is the one to
    /// name it; other ends are named by what closed the service.
    fn cause(self) -> Option<CloseCause> {
        match self {
            Self::ViewerClosed => Some(CloseCause::PeerClose),
            Self::Finished(_) => Some(CloseCause::OwnerClose),
            Self::Failed(_) => Some(CloseCause::DeviceLoss),
            _ => None,
        }
    }
}

/// Records how a hosted service's stream ended once it ends, even when the
/// engine drops its task, and names its close cause ([`close_hosted`]).
/// `record` writes the service's own record of it, given how it ended.
pub(super) struct HostedEnding {
    shared: Arc<Shared>,
    id: u64,
    end: Mutex<Option<HostedEnd>>,
    closed: std::sync::atomic::AtomicBool,
    record: Box<dyn Fn(&'static str) + Send + Sync>,
}

impl HostedEnding {
    pub(super) fn new(
        shared: &Arc<Shared>,
        id: u64,
        record: impl Fn(&'static str) + Send + Sync + 'static,
    ) -> Self {
        Self {
            shared: shared.clone(),
            id,
            end: Mutex::new(None),
            closed: std::sync::atomic::AtomicBool::new(false),
            record: Box::new(record),
        }
    }

    /// Notes how the stream ends; the first note stands.
    pub(super) fn why(&self, end: HostedEnd) {
        lock(&self.end).get_or_insert(end);
    }

    /// Records the end and names the service's cause, once: the end noted,
    /// else `failure`, the error the stream ended with, else that the
    /// engine dropped it.
    pub(super) fn close(&self, failure: Option<&'static str>) {
        if self.closed.swap(true, std::sync::atomic::Ordering::AcqRel) {
            return;
        }
        // At shutdown, connections end and tasks drop in either order, and a
        // write fails on the closing link as if the viewer had left: each is
        // the engine shutting down.
        let shutting_down = lock(&self.shared.state).shut_down;
        let end = match *lock(&self.end) {
            None
            | Some(HostedEnd::ConnectionEnded | HostedEnd::Dropped | HostedEnd::ViewerClosed)
                if shutting_down =>
            {
                Some(HostedEnd::ShutDown)
            }
            end => end,
        };
        (self.record)(match (end, failure) {
            (Some(end), _) => end.label(),
            (None, Some(failure)) => failure,
            (None, None) => HostedEnd::Dropped.label(),
        });
        match (end.and_then(HostedEnd::cause), end, failure) {
            (Some(cause), _, _) => close_hosted(&self.shared, self.id, cause),
            // Ended by an error nothing named, as a refused open: the
            // service failed for it, not its device.
            (None, None, Some(failure)) => fail_hosted(&self.shared, self.id, failure),
            _ => {}
        }
    }
}

impl Drop for HostedEnding {
    fn drop(&mut self) {
        self.close(None);
    }
}

/// A refusal's wire status by what it means.
fn refusal(status: u8) -> &'static str {
    match status {
        1 => "unsupported",
        2 | 6 => "not permitted",
        3 => "stale session",
        4 => "busy",
        5 => "malformed",
        _ => "other",
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
    if body[1] == WAIT && kind == 2 && remote_id != 0 {
        return serve_files_wait(shared, connection_id, peer, stamp, remote_id, send, recv).await;
    }
    if body[1] == PROBE && remote_id == 0 && kind == crate::snapshot::WIRE_KIND {
        return super::snapshot::probe(shared, connection_id, peer, send, stamp).await;
    }
    if body[1] == 2 && remote_id == 0 && matches!(kind, 4 | 5 | 6) {
        {
            let state = lock(&shared.state);
            let session = live_session(&state, true, connection_id)?;
            if session.wire_stamp != stamp || session.peer_key != peer {
                return Err(Error::StaleSession);
            }
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
            && !(kind == 4 && lock(&shared.camera_factory).is_some())
            && !(matches!(kind, 5 | 6)
                && lock(&shared.audio_factory).as_ref().is_some_and(|f| {
                    f.supports(if kind == 5 {
                        crate::audio::AudioKind::System
                    } else {
                        crate::audio::AudioKind::Microphone
                    })
                }))
            && !(kind == 3 && lock(&shared.terminal_factory).is_some())
            && kind != 7
            && !(kind == 8 && lock(&shared.snapshot_factory).is_some())
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
                    span: shared.span.clone(),
                },
            );
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
            tracing::info!(
                target: crate::trace::HOSTING,
                connection = connection_id,
                // An unknown kind is left out.
                kind = service_kind.as_ref().map(tracing::field::debug),
                status = refusal(status),
                "service refused"
            );
            write(&mut send, vec![VERSION, status, 0]).await?;
            return Ok(());
        }
        Ok(None) => {
            let hosted = lock(&shared.state).incoming.contains_key(&connection_id);
            let desktop =
                hosted && lock(&shared.video_caps).can_encode() && monitor_size(&shared) != (0, 0);
            let terminal = hosted && lock(&shared.terminal_factory).is_some();
            let camera = hosted && lock(&shared.camera_factory).is_some();
            let audio = hosted;
            let factory = lock(&shared.audio_factory).clone();
            let system_audio = audio
                && factory
                    .as_ref()
                    .is_some_and(|f| f.supports(crate::audio::AudioKind::System));
            let microphone = audio
                && factory
                    .as_ref()
                    .is_some_and(|f| f.supports(crate::audio::AudioKind::Microphone));
            let system_actions = hosted && lock(&shared.system_actions).is_some();
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
        Ok(Some(id)) => {
            tracing::info!(
                target: crate::trace::HOSTING,
                service = id,
                connection = connection_id,
                kind = ?service_kind,
                "service opened"
            );
            id
        }
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
    loop {
        if *parent_stop.borrow() || *service_stop.borrow() {
            break;
        }
        let request = tokio::select! {
            _ = parent_stop.changed() => break,
            _ = service_stop.changed() => break,
            // The viewer ended its pipe: it closed the service.
            request = read(&mut recv) => match request {
                Ok(request) => request,
                Err(_) => {
                    close_hosted(&shared, id, CloseCause::PeerClose);
                    break;
                }
            },
        };
        // The existing handler checks current peer, generation, Transfer grant
        // and shared roots at each operation, under the mutation locks.
        tokio::select! {
            biased;
            _ = parent_stop.changed() => break,
            _ = service_stop.changed() => break,
            result = super::fs::handle_fs_request(&shared, connection_id, &peer, &mut send, &request) => result.map_err(|_| Error::Malformed)?,
        }
    }
    crate::ignored!(
        crate::trace::SERVICE,
        "a stream that cannot finish is closed with its link",
        send.finish()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use crate::error::Error;
    use crate::test_support::{loopback_core, pair_for_control, transfer_pair, FilesystemLocation};
    use crate::{Core, ServiceKind, SessionInfo, SessionPhase};

    fn wait(name: &str, mut condition: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(8);
        while !condition() {
            assert!(Instant::now() < deadline, "{name} timed out");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn active(core: &Core, id: u64) -> SessionInfo {
        wait("service condition", || {
            core.controller_session(id).unwrap().phase == SessionPhase::Active
        });
        core.controller_session(id).unwrap()
    }

    fn owned_files(core: &Core, host: &Core) -> (crate::ServiceInfo, crate::RemoteFilesService) {
        let session = active(
            core,
            core.begin_connect_services(&host.endpoint_id()).unwrap().id,
        );
        let owner = core
            .attach_controller_service(session.id, session.generation, ServiceKind::Files)
            .unwrap();
        let pipe = core.open_files_service(owner.id, owner.generation).unwrap();
        (owner, pipe)
    }

    #[test]
    fn remote_service_negotiation_rejects_incompatible_or_stale_requests() {
        let controller = loopback_core();
        let host = loopback_core();
        pair_for_control(&host, &controller);
        let session = active(
            &controller,
            controller
                .begin_connect_services(&host.endpoint_id())
                .unwrap()
                .id,
        );
        let incoming = host.snapshot().unwrap().incoming.unwrap();
        let header = |version: u8, generation: u64, kind: u8| {
            let mut bytes = vec![8, version, 1];
            bytes.extend_from_slice(&incoming.id.to_be_bytes());
            bytes.extend_from_slice(&generation.to_be_bytes());
            bytes.extend_from_slice(&123u64.to_be_bytes());
            bytes.push(kind);
            bytes
        };
        for (bytes, expected) in [
            (header(2, incoming.generation, 2), 5),
            (header(1, incoming.generation + 1, 2), 3),
            (header(1, incoming.generation, 3), 6),
            (header(1, incoming.generation, 4), 6),
            (header(1, incoming.generation, 255), 1),
            (header(1, incoming.generation, 2), 2),
        ] {
            assert_eq!(
                controller
                    .test_service_frame(session.id, session.generation, bytes)
                    .unwrap(),
                vec![1, expected, 0]
            );
            assert!(host.hosted_services().unwrap().is_empty());
            assert_eq!(controller.controller_session(session.id).unwrap(), session);
        }
        // Permission and backend availability are distinct. An explicit grant
        // changes Denied to Unsupported without creating a shell/camera backend.
        host.set_service_sharing(ServiceKind::Terminal, true)
            .unwrap();
        assert_eq!(
            controller
                .test_service_frame(
                    session.id,
                    session.generation,
                    header(1, incoming.generation, 3)
                )
                .unwrap(),
            vec![1, 1, 0]
        );
        assert_eq!(
            controller
                .test_service_frame(
                    session.id,
                    session.generation,
                    header(1, incoming.generation, 4)
                )
                .unwrap(),
            vec![1, 6, 0]
        );
        assert_eq!(
            controller
                .test_service_frame(
                    session.id,
                    session.generation,
                    header(1, incoming.generation, 2)
                )
                .unwrap(),
            vec![1, 2, 0]
        );
        host.set_service_sharing(ServiceKind::Camera, true).unwrap();
        assert_eq!(
            controller
                .test_service_frame(
                    session.id,
                    session.generation,
                    header(1, incoming.generation, 4)
                )
                .unwrap(),
            vec![1, 1, 0]
        );
        assert!(host.hosted_services().unwrap().is_empty());
        assert!(controller
            .test_service_frame(session.id, session.generation, vec![8, 1])
            .is_err());
        assert!(
            controller
                .remote_service_support(session.id, session.generation)
                .unwrap()
                .files
        );
    }

    /// A listing slower than its caller waits is that request's timeout: the
    /// pipe stays open, reads the late answer to the end, and the next listing
    /// gets its own answer. Transfers keep their owner.
    #[test]
    fn a_slow_listing_times_out_without_ending_the_files_service() {
        let (host, controller, host_scratch, _controller_scratch) = transfer_pair("slowlist");
        std::fs::write(host_scratch.join("root/a.txt"), b"a").unwrap();
        let files = FilesystemLocation::on(&host, &host_scratch.join("root"));
        let (owner, pipe) = owned_files(&controller, &host);
        let key = std::str::FromStr::from_str(&controller.endpoint_id())
            .map(|key: iroh::PublicKey| *key.as_bytes())
            .unwrap();
        // Slower than the caller waits, quicker than a stuck host.
        *crate::core::LIST_DELAY.lock().unwrap() = Some((key, 1_800));
        assert!(matches!(
            pipe.list(files.root, &files.path(""), 0, 100),
            Err(Error::Timeout)
        ));
        *crate::core::LIST_DELAY.lock().unwrap() = None;
        let page = pipe.list(files.root, &files.path(""), 0, 100).unwrap();
        assert!(page.entries.iter().any(|entry| entry.name == "a.txt"));
        assert!(!pipe.is_cancelled());
        let service = controller.controller_service(owner.id).unwrap();
        assert_eq!(service.phase, crate::ServicePhase::Active);
        host.shutdown();
        controller.shutdown();
    }
}
