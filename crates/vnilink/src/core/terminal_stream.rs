//! Independently cancelled terminal pipe over an admitted neutral connection.
use super::service_stream::{read, write};
use super::terminal_runtime::{self, HostedTerminal};
use super::*;
use crate::terminal::{
    TerminalCommand, TerminalEvent, TerminalOptions, TerminalSessionInfo, TerminalSize,
    TERMINAL_CHUNK, TERMINAL_QUEUE,
};
use crate::{CloseCause, ServiceKind, ServiceLease, ServicePhase};
use iroh::endpoint::{RecvStream, SendStream};
const TIMEOUT: Duration = Duration::from_secs(5);

fn dimensions(bytes: &[u8]) -> Result<TerminalSize, Error> {
    if bytes.len() != 5 || bytes[0] != 2 {
        return Err(Error::Malformed);
    }
    TerminalSize {
        rows: u16::from_be_bytes([bytes[1], bytes[2]]),
        columns: u16::from_be_bytes([bytes[3], bytes[4]]),
    }
    .validate()
}
fn resize(size: TerminalSize) -> Vec<u8> {
    let mut b = vec![2];
    b.extend(size.rows.to_be_bytes());
    b.extend(size.columns.to_be_bytes());
    b
}
fn encode_event(event: TerminalEvent) -> Vec<u8> {
    match event {
        TerminalEvent::Output(bytes) => {
            let mut b = vec![1];
            b.extend(bytes);
            b
        }
        TerminalEvent::Exited(code) => {
            let mut b = vec![3];
            b.extend(code.to_be_bytes());
            b
        }
        TerminalEvent::OutputGap(lost) => {
            let mut bytes = vec![5];
            bytes.extend(lost.to_be_bytes());
            bytes
        }
        TerminalEvent::Failed(_) => vec![4], // Backend details never travel on the wire.
    }
}
fn decode_event(bytes: Vec<u8>) -> Result<TerminalEvent, Error> {
    match bytes.first() {
        Some(1) if bytes.len() > 1 && bytes.len() <= TERMINAL_CHUNK + 1 => {
            Ok(TerminalEvent::Output(bytes[1..].to_vec()))
        }
        Some(3) if bytes.len() == 5 => Ok(TerminalEvent::Exited(u32::from_be_bytes(
            bytes[1..].try_into().unwrap(),
        ))),
        Some(4) if bytes.len() == 1 => Ok(TerminalEvent::Failed(
            "remote terminal backend failed".into(),
        )),
        Some(5) if bytes.len() == 9 => Ok(TerminalEvent::OutputGap(u64::from_be_bytes(
            bytes[1..].try_into().unwrap(),
        ))),
        _ => Err(Error::Malformed),
    }
}

pub struct RemoteTerminalService {
    session: TerminalSessionInfo,
    lease: ServiceLease,
    commands: tokio::sync::mpsc::Sender<Vec<u8>>,
    events: Mutex<tokio::sync::mpsc::Receiver<TerminalEvent>>,
    stop: tokio::sync::watch::Sender<bool>,
}
impl RemoteTerminalService {
    pub fn session(&self) -> TerminalSessionInfo {
        self.session.clone()
    }
    pub fn info(&self) -> &crate::ServiceInfo {
        self.lease.info()
    }
    pub fn is_cancelled(&self) -> bool {
        self.lease.is_cancelled()
    }
    pub fn input(&self, bytes: &[u8]) -> Result<(), Error> {
        if bytes.is_empty() || bytes.len() > TERMINAL_CHUNK {
            return Err(Error::InvalidSetting(
                "terminal input chunk is out of range",
            ));
        }
        let mut frame = vec![1];
        frame.extend(bytes);
        self.send(frame)
    }
    pub fn resize(&self, size: TerminalSize) -> Result<(), Error> {
        self.send(resize(size.validate()?))
    }
    fn send(&self, bytes: Vec<u8>) -> Result<(), Error> {
        if self.is_cancelled() {
            return Err(Error::NoSession);
        }
        self.commands.try_send(bytes).map_err(|e| match e {
            tokio::sync::mpsc::error::TrySendError::Full(_) => Error::Busy,
            _ => Error::NoSession,
        })
    }
    /// Drains buffered output/status even after the service ends.
    pub fn poll(&self) -> Option<TerminalEvent> {
        lock(&self.events).try_recv().ok()
    }
    pub fn close(&self) {
        self.stop.send_replace(true);
    }
}
impl Drop for RemoteTerminalService {
    fn drop(&mut self) {
        self.close();
    }
}

impl Core {
    pub fn terminal_policy(&self) -> Result<crate::terminal::TerminalPolicy, Error> {
        Ok(self.alive_state()?.terminal_policy)
    }
    /// Host-local, durable and transactional. Busy leaves both runtime and saved
    /// policy intact; callers may retry after all shells/startups have ended.
    pub fn set_terminal_policy(
        &self,
        policy: crate::terminal::TerminalPolicy,
    ) -> Result<(), Error> {
        let policy = policy.validate()?;
        let mut state = self.alive_state()?;
        if !state.terminal_pending.is_empty() || !lock(&self.shared.terminal_sessions).is_empty() {
            return Err(Error::Busy);
        }
        let previous = std::mem::replace(&mut state.terminal_policy, policy);
        if let Err(error) = persist(&self.shared, &mut state) {
            state.terminal_policy = previous;
            return Err(error);
        }
        Ok(())
    }
    /// Opt-in backend availability; a separate per-peer Terminal grant is required.
    pub fn set_terminal_factory(
        &self,
        factory: Option<Arc<dyn crate::terminal::HostTerminalFactory>>,
    ) -> Result<(), Error> {
        let mut state = self.alive_state()?;
        // Lock order is always state, then factory.
        *lock(&self.shared.terminal_factory) = factory;
        for terminal in lock(&self.shared.terminal_sessions).values() {
            terminal
                .stop
                .store(true, std::sync::atomic::Ordering::Release);
        }
        for record in state
            .hosted_services
            .values_mut()
            .filter(|r| r.info.kind == ServiceKind::Terminal)
        {
            if record.change(
                record.info.generation,
                ServicePhase::Closed(CloseCause::DeviceLoss),
            ) {
                self.shared.events.publish(CoreEvent::HostedServiceChanged {
                    service: record.info.clone(),
                });
            }
        }
        Ok(())
    }
    /// Host-side retained shell records, including detached persistent sessions.
    pub fn hosted_terminal_sessions(&self) -> Result<Vec<TerminalSessionInfo>, Error> {
        self.ensure_alive()?;
        let mut sessions: Vec<_> = lock(&self.shared.terminal_sessions)
            .values()
            .map(|s| lock(&s.info).clone())
            .collect();
        sessions.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(sessions)
    }
    pub fn close_terminal_session(&self, id: &str) -> Result<(), Error> {
        self.ensure_alive()?;
        let sessions = lock(&self.shared.terminal_sessions);
        sessions
            .get(id)
            .ok_or(Error::NoSession)?
            .stop
            .store(true, std::sync::atomic::Ordering::Release);
        Ok(())
    }
    pub fn open_terminal_service(
        &self,
        id: u64,
        generation: u64,
        size: TerminalSize,
    ) -> Result<RemoteTerminalService, Error> {
        self.open_terminal_service_with_options(
            id,
            generation,
            TerminalOptions {
                size,
                persistent: false,
                resume_id: None,
            },
        )
    }
    pub fn open_terminal_service_with_options(
        &self,
        id: u64,
        generation: u64,
        options: TerminalOptions,
    ) -> Result<RemoteTerminalService, Error> {
        options.size.validate()?;
        if options
            .resume_id
            .as_ref()
            .is_some_and(|id| id.len() != 32 || !id.bytes().all(|c| c.is_ascii_hexdigit()))
        {
            return Err(Error::InvalidSetting("terminal resume ID is invalid"));
        }

        let mut lease = self.service_lease(id, generation)?;
        if lease.info().kind != ServiceKind::Terminal {
            return Err(Error::InvalidSetting("this is not a Terminal service"));
        }
        let handle_lease = lease.clone();
        let handle_connection = lease.info().connection_id;
        let (link, stamp) = self.service_link(lease.info().connection_id, generation)?;
        let stop = {
            let mut state = self.alive_state()?;
            let record = state.services.get_mut(&id).ok_or(Error::NoSession)?;
            if record.info.generation != generation {
                return Err(Error::StaleSession);
            }
            if record.info.phase != ServicePhase::Active || lease.is_cancelled() {
                return Err(Error::NoSession);
            }
            if record.stream_open {
                return Err(Error::Busy);
            }
            record.stream_open = true;
            record.stop.clone()
        };
        let (commands, mut requests) = tokio::sync::mpsc::channel::<Vec<u8>>(TERMINAL_QUEUE);
        let (events, output) = tokio::sync::mpsc::channel(TERMINAL_QUEUE);
        let (ready, result) = std::sync::mpsc::sync_channel(1);
        let shared = self.shared.clone();
        self.runtime.spawn(async move {
            let opened = lease
                .run(async {
                    tokio::time::timeout(TIMEOUT, async {
                        let (mut send, mut recv) =
                            link.open_bi().await.map_err(Error::network_debug)?;
                        write(&mut send, super::service_stream::preamble(stamp, id, 1, 3)).await?;
                        write(
                            &mut send,
                            serde_json::to_vec(&options).map_err(|_| Error::Malformed)?,
                        )
                        .await?;
                        super::service_stream::status(&read(&mut recv).await?)?;
                        let session: TerminalSessionInfo =
                            serde_json::from_slice(&read(&mut recv).await?)
                                .map_err(|_| Error::Malformed)?;
                        Ok((send, recv, session))
                    })
                    .await
                    .map_err(|_| Error::Timeout)?
                })
                .await;
            let mut phase = ServicePhase::Closed(CloseCause::PeerClose);
            match opened {
                Err(error) => {
                    phase = ServicePhase::Failed(error.to_string());
                    let _ = ready.send(Err(error));
                }
                Ok((mut send, mut recv, session)) => {
                    let session_id = session.id.clone();
                    let _ = ready.send(Ok(session));
                    let run = async {
                        let sending = async {
                            while let Some(frame) = requests.recv().await {
                                write(&mut send, frame).await?;
                            }
                            Ok::<(), Error>(())
                        };
                        let receiving = async {
                            loop {
                                let event = decode_event(read(&mut recv).await?)?;
                                let failed = matches!(event, TerminalEvent::Failed(_));
                                let finished = !matches!(
                                    event,
                                    TerminalEvent::Output(_) | TerminalEvent::OutputGap(_)
                                );
                                events.send(event).await.map_err(|_| Error::NoSession)?;
                                if failed {
                                    return Err(Error::InputFailed(
                                        "remote terminal backend failed",
                                    ));
                                }
                                if finished {
                                    return Ok::<(), Error>(());
                                }
                            }
                        };
                        tokio::select! { result = sending => result, result = receiving => result }
                    };
                    let result = lease.run(run).await;
                    if result.is_err() && !lease.is_cancelled() {
                        phase = ServicePhase::Failed("terminal pipe ended".into());
                        if !matches!(result, Err(Error::InputFailed(_))) {
                            let _ = events
                                .try_send(TerminalEvent::Failed("terminal pipe ended".into()));
                        }
                    } else if lease.is_cancelled() {
                        phase = ServicePhase::Closed(CloseCause::OwnerClose);
                    }
                    // FIN cannot distinguish a closed tab from a lost parent.
                    // Send an explicit close intent only while this parent is
                    // still active; a disconnected parent must retain its shell.
                    let explicit = lease.is_cancelled() && {
                        let state = lock(&shared.state);
                        !state.shut_down
                            && state.outgoing.get(&handle_connection).is_some_and(|s| {
                                s.generation == generation && s.phase == SessionPhase::Active
                            })
                    };
                    if explicit {
                        // A separate registry RPC bypasses input backpressure.
                        // Close must work even when the foreground job stopped
                        // reading and the PTY/input queue is full.
                        let close = async {
                            let (mut control, mut response) =
                                link.open_bi().await.map_err(Error::network_debug)?;
                            write(
                                &mut control,
                                super::service_stream::preamble(stamp, 0, 4, 3),
                            )
                            .await?;
                            write(&mut control, session_id.into_bytes()).await?;
                            super::service_stream::status(&read(&mut response).await?)?;
                            let _ = control.finish();
                            Ok::<(), Error>(())
                        };
                        if !matches!(tokio::time::timeout(TIMEOUT, close).await, Ok(Ok(()))) {
                            let _ = tokio::time::timeout(
                                Duration::from_millis(250),
                                write(&mut send, vec![6]),
                            )
                            .await;
                        }
                    }
                    let _ = send.finish();
                    // Finishing this stream only retires its local owner.
                }
            }
            let mut state = lock(&shared.state);
            if let Some(record) = state
                .services
                .get_mut(&id)
                .filter(|r| r.info.generation == generation)
            {
                if record.change(generation, phase) {
                    shared.events.publish(CoreEvent::ServiceChanged {
                        service: record.info.clone(),
                    });
                }
            }
        });
        let session = result
            .recv_timeout(TIMEOUT + Duration::from_secs(1))
            .map_err(|_| Error::Timeout)??;
        Ok(RemoteTerminalService {
            session,
            lease: handle_lease,
            commands,
            events: Mutex::new(output),
            stop,
        })
    }
}

pub(super) async fn serve(
    shared: Arc<Shared>,
    id: u64,
    mut send: SendStream,
    mut recv: RecvStream,
    mut parent: tokio::sync::watch::Receiver<bool>,
    mut stop: tokio::sync::watch::Receiver<bool>,
) -> Result<(), Error> {
    let factory = lock(&shared.terminal_factory)
        .clone()
        .ok_or(Error::Unsupported("terminal backend"))?;
    let peer = {
        let state = lock(&shared.state);
        let owner = state.hosted_services.get(&id).ok_or(Error::NoSession)?;
        parse_endpoint_id(&owner.info.peer_id)?
    };
    let mut attached: Option<terminal_runtime::Attachment> = None;
    let explicit_close = Arc::new(AtomicBool::new(false));
    // Clones for startup cancellation: a delayed native open must not leak a
    // retained shell when parent loss / revoke / shutdown wins first.
    let mut startup_parent = parent.clone();
    let mut startup_stop = stop.clone();
    let run = async {
        let bytes = tokio::time::timeout(TIMEOUT, read(&mut recv))
            .await
            .map_err(|_| Error::Timeout)??;
        let options: TerminalOptions =
            serde_json::from_slice(&bytes).map_err(|_| Error::Malformed)?;
        options.size.validate()?;
        // Fast-path refusal when cancellation already arrived before startup.
        if *startup_parent.borrow() || *startup_stop.borrow() {
            return Err(Error::NoSession);
        }
        let opened = async {
            let runtime = if let Some(session_id) = &options.resume_id {
                if *startup_parent.borrow() || *startup_stop.borrow() {
                    return Err(Error::NoSession);
                }
                let runtime = lock(&shared.terminal_sessions)
                    .get(session_id)
                    .cloned()
                    .ok_or(Error::NoSession)?;
                if runtime.peer != peer {
                    return Err(Error::Unauthorized);
                }
                runtime.attach(options.size)?;
                // Attach succeeded; re-check before admitting: a revoke that
                // landed during attach must not leave an attached shell.
                if *startup_parent.borrow() || *startup_stop.borrow() {
                    runtime.detach();
                    return Err(Error::NoSession);
                }
                runtime
            } else {
                let reservation =
                    terminal_runtime::reserve(shared.clone(), peer, options.persistent)?;
                let size = options.size;
                let startup = crate::BackendStartup::scoped(
                    startup_parent.clone(),
                    startup_stop.clone(),
                    TIMEOUT,
                );
                let _startup_guard = startup.guard();
                let join = tokio::task::spawn_blocking(move || {
                    startup.check()?;
                    let backend = factory.open(size, startup.clone())?;
                    startup.check()?;
                    Ok::<_, Error>((backend, reservation))
                });
                let opened = tokio::select! {
                    biased;
                    _ = startup_parent.changed() => return Err(Error::NoSession),
                    _ = startup_stop.changed() => return Err(Error::NoSession),
                    _ = read(&mut recv) => return Err(Error::NoSession),
                    result = tokio::time::timeout(TIMEOUT, join) => result
                        .map_err(|_| Error::Timeout)?
                        .map_err(|_| Error::Unsupported("terminal backend startup"))?,
                };
                let (backend, reservation) = match opened {
                    Ok(backend) => backend,
                    Err(_) => return Err(Error::Unsupported("terminal backend startup")),
                };
                // Cancellation may have arrived while waking from the blocking
                // call. Drop the backend without start() so no retained shell
                // is inserted; Drop reaps the PTY.
                if *startup_parent.borrow() || *startup_stop.borrow() {
                    drop(backend);
                    return Err(Error::NoSession);
                }
                let runtime = terminal_runtime::start(
                    shared.clone(),
                    peer,
                    backend,
                    &options,
                    reservation.policy,
                )?;
                drop(reservation);
                // start() inserts globally; if cancellation won in the gap,
                // stop/detach immediately so the ticker reaps it.
                if *startup_parent.borrow() || *startup_stop.borrow() {
                    runtime
                        .stop
                        .store(true, std::sync::atomic::Ordering::Release);
                    runtime.detach();
                    return Err(Error::NoSession);
                }
                runtime
            };
            Ok::<_, Error>(runtime)
        }
        .await;
        let runtime = match opened {
            Ok(runtime) => runtime,
            Err(error) => {
                let status = match error {
                    Error::Unauthorized => 6,
                    Error::Busy => 4,
                    Error::NoSession | Error::StaleSession => 3,
                    _ => 1,
                };
                write(&mut send, vec![1, status, 0]).await?;
                return Err(error);
            }
        };
        attached = Some(terminal_runtime::Attachment {
            terminal: runtime.clone(),
            explicit_close: explicit_close.clone(),
        });
        write(&mut send, vec![1, 0, 4]).await?;
        let metadata = lock(&runtime.info).clone();
        write(
            &mut send,
            serde_json::to_vec(&metadata).map_err(|_| Error::Malformed)?,
        )
        .await?;
        let mut changed = runtime.changed.subscribe();
        let mut offset = if options.resume_id.is_some() {
            runtime.replay_start()
        } else {
            0
        };
        let receiving = async {
            loop {
                let bytes = read(&mut recv).await?;
                let command = match bytes.first() {
                    Some(6) if bytes.len() == 1 => {
                        explicit_close.store(true, std::sync::atomic::Ordering::Release);
                        return Ok(());
                    }
                    Some(1) if bytes.len() > 1 && bytes.len() <= TERMINAL_CHUNK + 1 => {
                        TerminalCommand::Input(bytes[1..].to_vec())
                    }
                    Some(2) => {
                        let size = dimensions(&bytes)?;
                        TerminalCommand::Resize(size)
                    }
                    _ => return Err(Error::Malformed),
                };
                let new_size = match &command {
                    TerminalCommand::Resize(size) => Some(*size),
                    _ => None,
                };
                let mut command = command;
                loop {
                    match runtime.commands.try_send(command) {
                        Ok(()) => break,
                        Err(std::sync::mpsc::TrySendError::Full(value)) => command = value,
                        Err(_) => return Err(Error::NoSession),
                    }
                    // Cancellation drops this future and its unaccepted command.
                    // No blocking sender survives a lost connection.
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
                if let Some(size) = new_size {
                    lock(&runtime.info).size = size;
                }
            }
        };
        let sending = async {
            loop {
                if runtime.stop.load(std::sync::atomic::Ordering::Acquire) {
                    return Err(Error::NoSession);
                }
                let (first, bytes, ended) = runtime.read(offset);
                if first > offset {
                    write(
                        &mut send,
                        encode_event(TerminalEvent::OutputGap(first - offset)),
                    )
                    .await?;
                    offset = first;
                }
                if !bytes.is_empty() {
                    offset += bytes.len() as u64;
                    write(&mut send, encode_event(TerminalEvent::Output(bytes))).await?;
                    continue;
                }
                if let Some(event) = ended {
                    write(&mut send, encode_event(event)).await?;
                    explicit_close.store(true, std::sync::atomic::Ordering::Release);
                    return Ok(());
                }
                changed.changed().await.map_err(|_| Error::NoSession)?;
            }
        };
        tokio::select! { result = receiving => result, result = sending => result }
    };
    if *parent.borrow() || *stop.borrow() {
        return Err(Error::NoSession);
    }
    let result = tokio::select! { biased; _ = parent.changed() => Err(Error::NoSession), _ = stop.changed() => Err(Error::NoSession), result = run => result };
    if matches!(result, Err(Error::Malformed)) {
        explicit_close.store(true, std::sync::atomic::Ordering::Release);
    }
    drop(attached);
    let _ = send.finish();
    result
}

impl Core {
    /// Discovers this paired peer's retained shells after reconnect. Session IDs
    /// alone are not authorization; the host checks the peer's Terminal grant.
    pub fn remote_terminal_sessions(
        &self,
        connection: u64,
        generation: u64,
    ) -> Result<Vec<TerminalSessionInfo>, Error> {
        let bytes = self.terminal_registry_request(connection, generation, None)?;
        let sessions: Vec<TerminalSessionInfo> =
            serde_json::from_slice(&bytes).map_err(|_| Error::Malformed)?;
        if sessions.len() > crate::terminal::MAX_TERMINAL_SESSIONS {
            return Err(Error::Malformed);
        }
        for session in &sessions {
            session.size.validate()?;
        }
        Ok(sessions)
    }
    pub fn close_remote_terminal_session(
        &self,
        connection: u64,
        generation: u64,
        id: &str,
    ) -> Result<(), Error> {
        if id.len() != 32 || !id.bytes().all(|c| c.is_ascii_hexdigit()) {
            return Err(Error::InvalidSetting("terminal resume ID is invalid"));
        }
        self.terminal_registry_request(connection, generation, Some(id.to_owned()))
            .map(|_| ())
    }
    fn terminal_registry_request(
        &self,
        connection: u64,
        generation: u64,
        id: Option<String>,
    ) -> Result<Vec<u8>, Error> {
        let (link, stamp) = self.service_link(connection, generation)?;
        let mut stop = self
            .alive_state()?
            .outgoing
            .get(&connection)
            .ok_or(Error::NoSession)?
            .stop
            .subscribe();
        self.runtime.block_on(async {
            if *stop.borrow() { return Err(Error::NoSession); }
            let request = tokio::time::timeout(TIMEOUT, async {
                let (mut send, mut recv) = link.open_bi().await.map_err(Error::network_debug)?;
                write(&mut send, super::service_stream::preamble(stamp, 0, if id.is_some() {4} else {3}, 3)).await?;
                if let Some(id) = &id { write(&mut send, id.as_bytes().to_vec()).await?; }
                super::service_stream::status(&read(&mut recv).await?)?;
                let result = if id.is_none() { read(&mut recv).await? } else { Vec::new() };
                let _ = send.finish();
                Ok(result)
            });
            tokio::select! { biased; _ = stop.changed() => Err(Error::NoSession), result = request => result.map_err(|_| Error::Timeout)? }
        })
    }
}
pub(super) async fn registry(
    shared: Arc<Shared>,
    peer: [u8; 32],
    close: bool,
    mut send: SendStream,
    mut recv: RecvStream,
    mut parent: tokio::sync::watch::Receiver<bool>,
) -> Result<(), Error> {
    let run = async {
        let id = if close {
            Some(String::from_utf8(read(&mut recv).await?).map_err(|_| Error::Malformed)?)
        } else {
            None
        };
        let result = {
            let state = lock(&shared.state);
            if !state.trust.service_granted(&peer, ServiceKind::Terminal) {
                Err(Error::Unauthorized)
            } else if let Some(id) = &id {
                let sessions = lock(&shared.terminal_sessions);
                match sessions.get(id).filter(|s| s.peer == peer) {
                    Some(session) => {
                        session
                            .stop
                            .store(true, std::sync::atomic::Ordering::Release);
                        Ok(Vec::new())
                    }
                    None => Err(Error::NoSession),
                }
            } else {
                let mut sessions: Vec<_> = lock(&shared.terminal_sessions)
                    .values()
                    .filter(|s| {
                        s.peer == peer && !s.stop.load(std::sync::atomic::Ordering::Acquire)
                    })
                    .map(|s| lock(&s.info).clone())
                    .collect();
                sessions.sort_by(|a, b| a.id.cmp(&b.id));
                serde_json::to_vec(&sessions).map_err(|_| Error::Malformed)
            }
        };
        match result {
            Ok(bytes) => {
                write(&mut send, vec![1, 0, 4]).await?;
                if !close {
                    write(&mut send, bytes).await?;
                }
            }
            Err(error) => {
                write(
                    &mut send,
                    vec![
                        1,
                        if matches!(error, Error::Unauthorized) {
                            6
                        } else {
                            3
                        },
                        0,
                    ],
                )
                .await?;
            }
        }
        let _ = send.finish();
        Ok(())
    };
    if *parent.borrow() {
        return Err(Error::NoSession);
    }
    tokio::select! { biased; _ = parent.changed() => Err(Error::NoSession), result = tokio::time::timeout(TIMEOUT, run) => result.map_err(|_| Error::Timeout)? }
}
