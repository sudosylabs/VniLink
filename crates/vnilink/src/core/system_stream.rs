//! Hosted OS-action service: lock, restart, shutdown, privacy mode.
//!
//! Wire kind 7 on the service tag. The controller opens one System owner per
//! connection, then sends framed requests and reads framed replies. Error
//! replies reuse the shared status triple; success replies start with a zero
//! byte so the two can never be confused.
// trace: records every ignored failure

use super::service_stream::{preamble, read, status, write, VERSION};
use super::*;
use crate::system::{
    truncate_detail, HostSessionState, Notice, OperationState, PrivacyLease, PrivacyState,
    ScheduledSystemAction, SystemAction, SystemOperation, SystemOperationRecord, SystemSupport,
};
use crate::{CloseCause, ServiceInfo, ServiceKind, ServiceLease, ServicePhase};
use iroh::endpoint::{RecvStream, SendStream};

pub(super) const KIND: u8 = 7;
const OPEN: u8 = 1;
const TIMEOUT: Duration = Duration::from_secs(5);

const REQ_EXECUTE: u8 = 1;
const REQ_QUERY: u8 = 2;
const REQ_SUPPORT: u8 = 3;
const REQ_SET_PRIVACY: u8 = 4;
const REQ_GET_PRIVACY: u8 = 5;
const REQ_CANCEL: u8 = 6;

const TAG_OPERATION: u8 = 1;
const TAG_SUPPORT: u8 = 3;
const TAG_PRIVACY: u8 = 4;

/// Retained operations per host. Queries are generation-fenced, so old
/// connections can never reach a replacement's records.
const MAX_OPERATIONS: usize = 64;
const MAX_DETAIL: usize = 256;
const MAX_SESSION_LABEL: usize = 64;

pub(crate) fn encode_operation(operation: &SystemOperation) -> Vec<u8> {
    let detail = truncate_detail(operation.detail.clone());
    let mut out = vec![
        0,
        TAG_OPERATION,
        operation.action.wire(),
        operation.state.wire(),
    ];
    out.extend_from_slice(&operation.id.to_be_bytes());
    out.push(u8::from(operation.completion_observable));
    out.extend_from_slice(&operation.notice_remaining_ms.to_be_bytes());
    out.extend_from_slice(&(detail.len() as u16).to_be_bytes());
    out.extend_from_slice(detail.as_bytes());
    out
}

pub(crate) fn decode_operation(bytes: &[u8]) -> Result<SystemOperation, Error> {
    if bytes.len() < 19 || bytes[0] != 0 || bytes[1] != TAG_OPERATION {
        return Err(Error::Malformed);
    }
    let action = SystemAction::decode(bytes[2])?;
    let state = OperationState::decode(bytes[3])?;
    let id = u64::from_be_bytes(bytes[4..12].try_into().map_err(|_| Error::Malformed)?);
    let completion_observable = match bytes[12] {
        0 => false,
        1 => true,
        _ => return Err(Error::Malformed),
    };
    let notice_remaining_ms =
        u32::from_be_bytes(bytes[13..17].try_into().map_err(|_| Error::Malformed)?);
    let len = u16::from_be_bytes(bytes[17..19].try_into().map_err(|_| Error::Malformed)?) as usize;
    if len > MAX_DETAIL || bytes.len() != 19 + len {
        return Err(Error::Malformed);
    }
    let detail = std::str::from_utf8(&bytes[19..]).map_err(|_| Error::Malformed)?;
    Ok(SystemOperation {
        id,
        action,
        state,
        completion_observable,
        detail: detail.into(),
        notice_remaining_ms,
    })
}

pub(crate) fn encode_support(support: SystemSupport, session: &HostSessionState) -> Vec<u8> {
    let mut label = session.session.as_bytes().to_vec();
    if label.len() > MAX_SESSION_LABEL {
        label.truncate(MAX_SESSION_LABEL);
        while label.last().is_some_and(|b| b & 0xC0 == 0x80) {
            label.pop();
        }
    }
    let flag = |value: Option<bool>| match value {
        None => 2,
        Some(false) => 0,
        Some(true) => 1,
    };
    let mut out = vec![
        0,
        TAG_SUPPORT,
        support.bits(),
        flag(session.locked),
        flag(session.login_screen),
        u8::from(session.headless),
        label.len() as u8,
    ];
    out.extend_from_slice(&label);
    out
}

pub(crate) fn decode_support(bytes: &[u8]) -> Result<(SystemSupport, HostSessionState), Error> {
    if bytes.len() < 7 || bytes[0] != 0 || bytes[1] != TAG_SUPPORT {
        return Err(Error::Malformed);
    }
    let support = SystemSupport::decode(bytes[2])?;
    let flag = |byte: u8| match byte {
        0 => Ok(Some(false)),
        1 => Ok(Some(true)),
        2 => Ok(None),
        _ => Err(Error::Malformed),
    };
    let locked = flag(bytes[3])?;
    let login_screen = flag(bytes[4])?;
    let headless = match bytes[5] {
        0 => false,
        1 => true,
        _ => return Err(Error::Malformed),
    };
    let len = bytes[6] as usize;
    if len > MAX_SESSION_LABEL || bytes.len() != 7 + len {
        return Err(Error::Malformed);
    }
    let session = std::str::from_utf8(&bytes[7..]).map_err(|_| Error::Malformed)?;
    Ok((
        support,
        HostSessionState {
            locked,
            login_screen,
            headless,
            session: session.into(),
        },
    ))
}

pub(crate) fn encode_privacy(state: PrivacyState) -> Vec<u8> {
    let mut out = vec![
        0,
        TAG_PRIVACY,
        u8::from(state.display_private),
        u8::from(state.input_blocked),
        u8::from(state.holder_connection.is_some()),
    ];
    out.extend_from_slice(&state.holder_connection.unwrap_or(0).to_be_bytes());
    out
}

pub(crate) fn decode_privacy(bytes: &[u8]) -> Result<PrivacyState, Error> {
    if bytes.len() != 13 || bytes[0] != 0 || bytes[1] != TAG_PRIVACY {
        return Err(Error::Malformed);
    }
    let bit = |byte: u8| match byte {
        0 => Ok(false),
        1 => Ok(true),
        _ => Err(Error::Malformed),
    };
    let display_private = bit(bytes[2])?;
    let input_blocked = bit(bytes[3])?;
    let holder_connection = match bytes[4] {
        0 => None,
        1 => Some(u64::from_be_bytes(
            bytes[5..13].try_into().map_err(|_| Error::Malformed)?,
        )),
        _ => return Err(Error::Malformed),
    };
    if holder_connection.is_none() && bytes[5..13].iter().any(|b| *b != 0) {
        return Err(Error::Malformed);
    }
    Ok(PrivacyState {
        display_private,
        input_blocked,
        holder_connection,
    })
}

struct Work {
    bytes: Vec<u8>,
    deadline: std::time::Instant,
    reply: std::sync::mpsc::SyncSender<Result<Vec<u8>, Error>>,
}

/// One System pipe on a neutral connection. Calls serialize through a
/// bounded queue; drop ends the worker, the parent and siblings survive.
pub struct RemoteSystemService {
    tx: tokio::sync::mpsc::Sender<Work>,
    lease: ServiceLease,
    stop: tokio::sync::watch::Sender<bool>,
}

impl RemoteSystemService {
    fn round_trip(&self, bytes: Vec<u8>) -> Result<Vec<u8>, Error> {
        if self.lease.is_cancelled() {
            return Err(Error::NoSession);
        }
        let (reply, rx) = std::sync::mpsc::sync_channel(1);
        self.tx
            .try_send(Work {
                bytes,
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

    fn request(&self, bytes: Vec<u8>) -> Result<Vec<u8>, Error> {
        let reply = self.round_trip(bytes)?;
        if reply.len() == 3 && reply.first() == Some(&1) {
            return match status(&reply) {
                Err(error) => Err(error),
                Ok(_) => Err(Error::Malformed),
            };
        }
        Ok(reply)
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

    /// Asks the host to run one action. The returned record is a receipt:
    /// `Admitted` means accepted, `Refused` means denied, `Executed` means
    /// the native call returned. A lost reply on restart/shutdown proves
    /// nothing either way; the host never replays the operation.
    pub fn execute(&self, action: SystemAction) -> Result<SystemOperation, Error> {
        decode_operation(&self.request(vec![REQ_EXECUTE, action.wire()])?)
    }

    /// Reads one retained operation on the same connection generation.
    pub fn operation(&self, id: u64) -> Result<SystemOperation, Error> {
        let mut bytes = vec![REQ_QUERY];
        bytes.extend_from_slice(&id.to_be_bytes());
        decode_operation(&self.request(bytes)?)
    }

    /// Cancels a restart or shutdown this connection asked for while its
    /// notice is still counting down.
    pub fn cancel(&self, id: u64) -> Result<SystemOperation, Error> {
        let mut bytes = vec![REQ_CANCEL];
        bytes.extend_from_slice(&id.to_be_bytes());
        decode_operation(&self.request(bytes)?)
    }

    /// Native facilities and session boundaries of the host. Grant and
    /// policy still gate every action; support alone authorizes nothing.
    pub fn support(&self) -> Result<(SystemSupport, HostSessionState), Error> {
        decode_support(&self.request(vec![REQ_SUPPORT])?)
    }

    /// Enables or releases display privacy and local-input blocking. Only
    /// one System owner holds the lease; a second requester sees `Busy`.
    /// Closing the owner, revoke, parent loss and shutdown all restore.
    pub fn set_privacy(
        &self,
        display_private: bool,
        input_blocked: bool,
    ) -> Result<PrivacyState, Error> {
        decode_privacy(&self.request(vec![
            REQ_SET_PRIVACY,
            u8::from(display_private),
            u8::from(input_blocked),
        ])?)
    }

    pub fn privacy(&self) -> Result<PrivacyState, Error> {
        decode_privacy(&self.request(vec![REQ_GET_PRIVACY])?)
    }
}

impl Core {
    /// Native OS-action backend. Removing it refuses new executions; a held
    /// privacy lease is restored exactly once by the owning serve loop
    /// through the backend retained in the lease.
    pub fn set_system_actions(
        &self,
        actions: Option<Arc<dyn crate::system::HostSystemActions>>,
    ) -> Result<(), Error> {
        let _engine = self.engine();
        // Serial first (privacy order), then state, then the adapter slot:
        // a replacement never interleaves with an in-flight enable, level
        // change or restore.
        let _serial = lock(&self.shared.privacy_serial);
        let mut state = self.alive_state()?;
        // Lock order is always state, then the adapter slot.
        *lock(&self.shared.system_actions) = actions;
        for record in state
            .hosted_services
            .values_mut()
            .filter(|r| r.info.kind == ServiceKind::System)
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

    pub fn system_policy(&self) -> Result<crate::system::SystemPolicy, Error> {
        let _engine = self.engine();
        Ok(self.alive_state()?.system_policy)
    }

    /// Host-local, durable and transactional: a failed save keeps the
    /// previous policy and the live lease. A saved narrowing of
    /// `allow_privacy` also releases the active privacy lease, because a
    /// lease granted under the old policy must not survive it; the holding
    /// owner stays connected, only the local facilities restore.
    pub fn set_system_policy(&self, policy: crate::system::SystemPolicy) -> Result<(), Error> {
        let _engine = self.engine();
        let policy = policy.validate()?;
        let mut state = self.alive_state()?;
        let previous = std::mem::replace(&mut state.system_policy, policy);
        if let Err(error) = persist(&self.shared, &mut state) {
            state.system_policy = previous;
            return Err(error);
        }
        let disallowed = !state.system_policy.allow_privacy;
        // Release the state guard before reconciling: reconciliation takes
        // the serial restoration lock first, like every other privacy path.
        drop(state);
        if disallowed {
            // The save stands; a failed native restore reports so the
            // caller learns the restriction is still active. Any save while
            // disallowed retries a retained restore, so repeating a failed
            // narrowing is a retry even though the value did not change.
            release_all_privacy(&self.shared)?;
        }
        Ok(())
    }

    /// Local backend facilities and session boundaries, without a peer.
    pub fn system_support(&self) -> Result<(SystemSupport, HostSessionState), Error> {
        let _engine = self.engine();
        let _state = self.alive_state()?;
        match lock(&self.shared.system_actions).clone() {
            Some(actions) => Ok((actions.support(), actions.session_state())),
            None => Ok((SystemSupport::default(), HostSessionState::unknown(""))),
        }
    }

    /// Restarts and shutdowns counting down on this computer.
    pub fn scheduled_system_actions(&self) -> Result<Vec<ScheduledSystemAction>, Error> {
        let _engine = self.engine();
        let state = self.alive_state()?;
        let mut scheduled: Vec<_> = state
            .system_operations
            .values()
            .filter(|record| record.operation.state == OperationState::Scheduled)
            .filter_map(|record| {
                Some(ScheduledSystemAction {
                    operation_id: record.operation.id,
                    action: record.operation.action,
                    endpoint_id: PublicKey::from_bytes(&record.peer).ok()?.to_string(),
                    remaining_ms: with_remaining(record).notice_remaining_ms,
                })
            })
            .collect();
        scheduled.sort_by_key(|action| action.operation_id);
        Ok(scheduled)
    }

    /// Cancels a restart or shutdown counting down on this computer, as its
    /// user chose in the notice. One that already ended is left as it is.
    pub fn cancel_system_action(&self, id: u64) -> Result<SystemOperation, Error> {
        let _engine = self.engine();
        drop(self.alive_state()?);
        cancel(&self.shared, None, id, "cancelled at that computer")
            .map_err(|_| Error::InvalidSetting("no such system action"))
    }

    /// Runs a scheduled restart or shutdown now, as its user chose in the notice.
    pub fn run_system_action_now(&self, id: u64) -> Result<(), Error> {
        let _engine = self.engine();
        let mut state = self.alive_state()?;
        let record = state
            .system_operations
            .get_mut(&id)
            .ok_or(Error::InvalidSetting("no such system action"))?;
        let notice = record
            .notice
            .as_mut()
            .filter(|_| record.operation.state == OperationState::Scheduled)
            .ok_or(Error::InvalidSetting("the system action is not scheduled"))?;
        notice.run_now = true;
        notice.wake.notify_one();
        Ok(())
    }

    /// Shortens the notice a restart or shutdown waits out, for tests.
    #[cfg(any(test, feature = "test-probes"))]
    pub fn set_system_action_notice(&self, notice: Duration) {
        let _engine = self.engine();
        *lock(&self.shared.system_notice) = notice;
    }

    /// Local privacy/input state and its holding connection, if any.
    pub fn system_privacy(&self) -> Result<PrivacyState, Error> {
        let _engine = self.engine();
        let state = self.alive_state()?;
        Ok(current_privacy(&state))
    }

    /// Opens a System backend on its own stream and task. The local owner
    /// must already exist; host admission re-checks the System grant.
    pub fn open_system_service(
        &self,
        id: u64,
        generation: u64,
    ) -> Result<RemoteSystemService, Error> {
        let _engine = self.engine();
        let mut lease = self.service_lease(id, generation)?;
        if lease.info().kind != ServiceKind::System {
            return Err(Error::InvalidSetting("this is not a System service"));
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
                        write(&mut send, preamble(stamp, id, OPEN, KIND)).await?;
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
                        crate::trace::SYSTEM,
                        "the caller stopped waiting for its answer",
                        ready.send(Ok(()))
                    );
                    pair
                }
                Err(error) => {
                    crate::ignored!(
                        crate::trace::SYSTEM,
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
                if std::time::Instant::now() >= work.deadline {
                    crate::ignored!(
                        crate::trace::SYSTEM,
                        "the caller stopped waiting for its answer",
                        work.reply.send(Err(Error::Timeout))
                    );
                    continue;
                }
                let outcome = lease
                    .run(async {
                        tokio::time::timeout_at(
                            tokio::time::Instant::from_std(work.deadline),
                            exchange(&mut send, &mut recv, work.bytes),
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
                    crate::trace::SYSTEM,
                    "the caller stopped waiting for its answer",
                    work.reply.send(outcome)
                );
                if fatal {
                    break;
                }
            }
            crate::ignored!(
                crate::trace::SYSTEM,
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
        Ok(RemoteSystemService {
            tx,
            lease: self.service_lease(id, generation)?,
            stop,
        })
    }
}

async fn exchange(
    send: &mut SendStream,
    recv: &mut RecvStream,
    request: Vec<u8>,
) -> Result<Vec<u8>, Error> {
    write(send, request).await?;
    read(recv).await
}

#[derive(Clone, Copy)]
struct Admission {
    connection_id: u64,
    generation: u64,
    service_id: u64,
}

fn admit(shared: &Shared, id: u64) -> Result<Admission, u8> {
    let state = lock(&shared.state);
    admit_locked(&state, id)
}

fn admit_locked(state: &State, id: u64) -> Result<Admission, u8> {
    let record = state.hosted_services.get(&id).ok_or(3)?;
    let connection_id = record.info.connection_id;
    let generation = record.info.generation;
    let session = state.incoming.get(&connection_id).ok_or(3)?;
    if session.phase != SessionPhase::Active
        || session.generation != generation
        || !session.services_only
    {
        return Err(3);
    }
    let peer = session.peer_key;
    if !state.trust.service_granted(&peer, ServiceKind::System) {
        return Err(6);
    }
    if record.info.phase != ServicePhase::Active {
        return Err(3);
    }
    Ok(Admission {
        connection_id,
        generation,
        service_id: id,
    })
}

fn prune(state: &mut State) {
    while state.system_operations.len() > MAX_OPERATIONS {
        if let Some(oldest) = state.system_operations.keys().min().copied() {
            state.system_operations.remove(&oldest);
        } else {
            break;
        }
    }
}

/// Worker-start outcome: either the native call ran, or the start gate
/// stood the action down with a refusal detail. Queued-but-unstarted
/// actions never run without fresh authority.
enum Started {
    Ran(Result<(), Error>),
    StoodDown(&'static str),
}

async fn execute(
    shared: &std::sync::Arc<Shared>,
    admission: Admission,
    action: SystemAction,
) -> Result<Vec<u8>, Error> {
    let scheduled = action != SystemAction::Lock;
    let id = {
        let mut state = lock(&shared.state);
        if !state.system_policy.allows(action) {
            let reply = refuse_locked(&mut state, &admission, action, "disabled by host policy");
            return Ok(reply);
        }
        let actions = lock(&shared.system_actions).clone();
        let Some(actions) = actions else {
            let reply = refuse_locked(&mut state, &admission, action, "no system backend");
            return Ok(reply);
        };
        let supported = match action {
            SystemAction::Lock => actions.support().lock,
            SystemAction::Restart => actions.support().restart,
            SystemAction::Shutdown => actions.support().shutdown,
        };
        if !supported {
            let reply = refuse_locked(&mut state, &admission, action, "not supported on this host");
            return Ok(reply);
        }
        let peer = state
            .incoming
            .get(&admission.connection_id)
            .map(|session| session.peer_key)
            .ok_or(Error::NoSession)?;
        let id = state.next_operation_id;
        state.next_operation_id = id.saturating_add(1).max(1);
        // Its end is recorded as it ends; an action the computer never
        // answers shows as admitted and never ended.
        tracing::info!(
            target: crate::trace::SYSTEM,
            operation = id,
            action = ?action,
            connection = admission.connection_id,
            scheduled,
            "system action admitted"
        );
        // A restart or shutdown waits out its notice first; a lock runs now.
        let notice = scheduled.then(|| Notice {
            deadline: std::time::Instant::now() + *lock(&shared.system_notice),
            run_now: false,
            wake: Arc::new(tokio::sync::Notify::new()),
        });
        state.system_operations.insert(
            id,
            SystemOperationRecord {
                operation: SystemOperation {
                    id,
                    action,
                    state: if scheduled {
                        OperationState::Scheduled
                    } else {
                        OperationState::Admitted
                    },
                    completion_observable: action.completion_observable(),
                    detail: if scheduled { "scheduled" } else { "admitted" }.into(),
                    notice_remaining_ms: 0,
                },
                connection_id: admission.connection_id,
                generation: admission.generation,
                peer,
                notice,
            },
        );
        prune(&mut state);
        if scheduled {
            let record = &state.system_operations[&id];
            let operation = with_remaining(record);
            shared.events.publish(CoreEvent::SystemActionScheduled {
                operation_id: id,
                action,
                endpoint_id: PublicKey::from_bytes(&peer)
                    .map_err(|_| Error::Malformed)?
                    .to_string(),
                remaining_ms: operation.notice_remaining_ms,
            });
            drop(state);
            tokio::spawn(run_scheduled(Arc::clone(shared), admission, action, id));
            return Ok(encode_operation(&operation));
        }
        id
    };
    let outcome = start(shared, admission, action).await?;
    finish(shared, id, action, outcome, false)
}

/// Waits out a scheduled operation's notice, then runs it under fresh
/// authority. Cancelling, or the asking computer going away, ends it first.
async fn run_scheduled(
    shared: std::sync::Arc<Shared>,
    admission: Admission,
    action: SystemAction,
    id: u64,
) {
    loop {
        let (wake, deadline, run_now) = {
            let state = lock(&shared.state);
            let Some(record) = state.system_operations.get(&id) else {
                return;
            };
            let Some(notice) = record.notice.as_ref() else {
                return;
            };
            if record.operation.state != OperationState::Scheduled {
                return;
            }
            let asker_present = asker_present(&state, &admission);
            if !asker_present {
                drop(state);
                crate::ignored!(
                    crate::trace::SYSTEM,
                    "an operation already settled needs no change",
                    cancel(&shared, None, id, "the computer that asked disconnected")
                );
                return;
            }
            (notice.wake.clone(), notice.deadline, notice.run_now)
        };
        let now = std::time::Instant::now();
        if run_now || now >= deadline {
            break;
        }
        // Short waits notice a departed asker during the countdown.
        let wait = (deadline - now).min(Duration::from_millis(500));
        tokio::select! {
            _ = tokio::time::sleep(wait) => {}
            _ = wake.notified() => {}
        }
    }
    {
        let mut state = lock(&shared.state);
        let Some(record) = state.system_operations.get_mut(&id) else {
            return;
        };
        if record.operation.state != OperationState::Scheduled {
            return;
        }
        record.notice = None;
        record.operation.state = OperationState::Admitted;
        record.operation.detail = "admitted".into();
    }
    let Ok(outcome) = start(&shared, admission, action).await else {
        return;
    };
    crate::ignored!(
        crate::trace::SYSTEM,
        "an operation already settled needs no change",
        finish(&shared, id, action, outcome, true)
    );
}

/// Whether the connection and System owner that asked are still active.
fn asker_present(state: &State, admission: &Admission) -> bool {
    state
        .incoming
        .get(&admission.connection_id)
        .is_some_and(|session| {
            session.generation == admission.generation && session.phase == SessionPhase::Active
        })
        && state
            .hosted_services
            .get(&admission.service_id)
            .is_some_and(|owner| {
                owner.info.phase == ServicePhase::Active
                    && owner.info.connection_id == admission.connection_id
                    && owner.info.generation == admission.generation
            })
}

/// Runs the native call on a blocking worker under fresh authority.
async fn start(
    shared: &std::sync::Arc<Shared>,
    admission: Admission,
    action: SystemAction,
) -> Result<Started, Error> {
    // Native calls run on a blocking worker; the serve loop stays responsive
    // to revoke and parent loss while one runs. Authority is rechecked when
    // the worker STARTS, not when the action was admitted: a policy, grant,
    // backend, owner, or connection change that lands while queued stands the
    // action down as Refused instead of running it.
    let gate_shared = std::sync::Arc::clone(shared);
    tokio::task::spawn_blocking(move || {
        let shared = gate_shared;
        let gate =
            || -> Result<std::sync::Arc<dyn crate::system::HostSystemActions>, &'static str> {
                let state = lock(&shared.state);
                if !state.system_policy.allows(action) {
                    return Err("disabled by host policy");
                }
                let session = state
                    .incoming
                    .get(&admission.connection_id)
                    .ok_or("connection closed")?;
                if session.generation != admission.generation {
                    return Err("connection closed");
                }
                // The parent must be Active, not merely unfinished: a
                // Stopping parent has already begun teardown.
                if session.phase != SessionPhase::Active {
                    return Err("connection not active");
                }
                if !super::service_permissions::admitted(
                    &state,
                    &session.peer_key,
                    crate::ServiceKind::System,
                ) {
                    return Err("system permission revoked");
                }
                let actions = lock(&shared.system_actions)
                    .clone()
                    .ok_or("no system backend")?;
                let supported = match action {
                    SystemAction::Lock => actions.support().lock,
                    SystemAction::Restart => actions.support().restart,
                    SystemAction::Shutdown => actions.support().shutdown,
                };
                if !supported {
                    return Err("not supported on this host");
                }
                // Last: the exact System owner must still be active. Revoke
                // and backend removal close it synchronously, so they keep
                // their own details above; a bare owner close lands here even
                // when the parent connection lives on.
                let owner = state
                    .hosted_services
                    .get(&admission.service_id)
                    .ok_or("system owner closed")?;
                if owner.info.phase != ServicePhase::Active
                    || owner.info.connection_id != admission.connection_id
                    || owner.info.generation != admission.generation
                {
                    return Err("system owner closed");
                }
                Ok(actions)
            };
        let actions = match gate() {
            Ok(actions) => actions,
            Err(detail) => return Started::StoodDown(detail),
        };
        Started::Ran(match action {
            SystemAction::Lock => actions.lock_workstation(),
            SystemAction::Restart => actions.restart(),
            SystemAction::Shutdown => actions.shutdown(),
        })
    })
    .await
    .map_err(|_| Error::ShutDown)
}

/// Records how a started operation ended. A scheduled one also tells the
/// host's notice; its asker leaving reads as cancelled, not refused.
fn finish(
    shared: &Shared,
    id: u64,
    action: SystemAction,
    outcome: Started,
    scheduled: bool,
) -> Result<Vec<u8>, Error> {
    let mut state = lock(&shared.state);
    let Some(record) = state.system_operations.get_mut(&id) else {
        return Err(Error::NoSession);
    };
    let (next, detail) = match &outcome {
        Started::Ran(Ok(())) => (
            OperationState::Executed,
            match action {
                SystemAction::Lock => "workstation lock requested".to_owned(),
                SystemAction::Restart => {
                    "restart requested; completion is not observable".to_owned()
                }
                SystemAction::Shutdown => {
                    "shutdown requested; completion is not observable".to_owned()
                }
            },
        ),
        Started::Ran(Err(error)) => (OperationState::Failed, error.to_string()),
        Started::StoodDown(
            "connection closed" | "connection not active" | "system owner closed",
        ) if scheduled => (
            OperationState::Cancelled,
            "the computer that asked disconnected".to_owned(),
        ),
        Started::StoodDown(detail) => (OperationState::Refused, detail.to_string()),
    };
    tracing::info!(
        target: crate::trace::SYSTEM,
        operation = id,
        action = ?action,
        state = ?next,
        scheduled,
        error = match &outcome {
            Started::Ran(Err(error)) => Some(error.name()),
            _ => None,
        },
        stood_down = match &outcome {
            Started::StoodDown(detail) => Some(*detail),
            _ => None,
        },
        "system action ended"
    );
    record.operation.state = next;
    record.operation.detail = truncate_detail(detail);
    let operation = with_remaining(record);
    if scheduled {
        shared.events.publish(CoreEvent::SystemActionEnded {
            operation_id: id,
            state: next,
        });
    }
    Ok(encode_operation(&operation))
}

fn refuse_locked(
    state: &mut State,
    admission: &Admission,
    action: SystemAction,
    detail: &str,
) -> Vec<u8> {
    let id = state.next_operation_id;
    state.next_operation_id = id.saturating_add(1).max(1);
    tracing::info!(
        target: crate::trace::SYSTEM,
        operation = id,
        action = ?action,
        connection = admission.connection_id,
        detail,
        "system action refused"
    );
    let operation = SystemOperation {
        id,
        action,
        state: OperationState::Refused,
        completion_observable: action.completion_observable(),
        detail: truncate_detail(detail.into()),
        notice_remaining_ms: 0,
    };
    let peer = state
        .incoming
        .get(&admission.connection_id)
        .map_or([0; 32], |session| session.peer_key);
    state.system_operations.insert(
        id,
        SystemOperationRecord {
            operation: operation.clone(),
            connection_id: admission.connection_id,
            generation: admission.generation,
            peer,
            notice: None,
        },
    );
    prune(state);
    encode_operation(&operation)
}

fn query(shared: &Shared, admission: &Admission, id: u64) -> Result<Vec<u8>, u8> {
    let state = lock(&shared.state);
    let record = state.system_operations.get(&id).ok_or(7)?;
    if record.connection_id != admission.connection_id || record.generation != admission.generation
    {
        return Err(7);
    }
    Ok(encode_operation(&with_remaining(record)))
}

/// The operation as it reads now: a scheduled one with the notice left.
fn with_remaining(record: &SystemOperationRecord) -> SystemOperation {
    let mut operation = record.operation.clone();
    operation.notice_remaining_ms = record.notice.as_ref().map_or(0, |notice| {
        u32::try_from(
            notice
                .deadline
                .saturating_duration_since(std::time::Instant::now())
                .as_millis(),
        )
        .unwrap_or(u32::MAX)
    });
    operation
}

/// Ends a scheduled operation without running it. `from` limits who may:
/// a viewer cancels only what its own connection asked for.
fn cancel(
    shared: &Shared,
    from: Option<&Admission>,
    id: u64,
    detail: &str,
) -> Result<SystemOperation, u8> {
    let mut state = lock(&shared.state);
    let record = state.system_operations.get_mut(&id).ok_or(7)?;
    if from.is_some_and(|admission| {
        record.connection_id != admission.connection_id || record.generation != admission.generation
    }) {
        return Err(7);
    }
    if record.operation.state != OperationState::Scheduled {
        return Ok(with_remaining(record));
    }
    record.operation.state = OperationState::Cancelled;
    record.operation.detail = truncate_detail(detail.into());
    if let Some(notice) = record.notice.take() {
        notice.wake.notify_one();
    }
    tracing::info!(
        target: crate::trace::SYSTEM,
        operation = id,
        action = ?record.operation.action,
        by_viewer = from.is_some(),
        detail,
        "system action cancelled"
    );
    let operation = with_remaining(record);
    shared.events.publish(CoreEvent::SystemActionEnded {
        operation_id: id,
        state: OperationState::Cancelled,
    });
    Ok(operation)
}

fn support(shared: &Shared) -> Vec<u8> {
    match lock(&shared.system_actions).clone() {
        Some(actions) => encode_support(actions.support(), &actions.session_state()),
        None => encode_support(SystemSupport::default(), &HostSessionState::unknown("")),
    }
}

fn current_privacy(state: &State) -> PrivacyState {
    match state.privacy.as_ref() {
        Some(lease) => {
            let (display_private, input_blocked) = lease
                .actions
                .observed_privacy()
                .unwrap_or((lease.display_private, lease.input_blocked));
            PrivacyState {
                display_private,
                input_blocked,
                holder_connection: Some(lease.connection_id),
            }
        }
        None => PrivacyState::default(),
    }
}

fn set_privacy(
    shared: &Shared,
    admission: &Admission,
    service_id: u64,
    display_private: bool,
    input_blocked: bool,
) -> Result<Vec<u8>, u8> {
    set_privacy_inner(
        shared,
        admission,
        service_id,
        display_private,
        input_blocked,
        &std::sync::atomic::AtomicBool::new(false),
    )
}

fn privacy_authorized(
    state: &State,
    admission: &Admission,
    cancelled: &std::sync::atomic::AtomicBool,
) -> bool {
    !cancelled.load(std::sync::atomic::Ordering::Acquire)
        && admit_locked(state, admission.service_id).is_ok_and(|fresh| {
            fresh.connection_id == admission.connection_id
                && fresh.generation == admission.generation
        })
}

fn set_privacy_inner(
    shared: &Shared,
    admission: &Admission,
    service_id: u64,
    display_private: bool,
    input_blocked: bool,
    cancelled: &std::sync::atomic::AtomicBool,
) -> Result<Vec<u8>, u8> {
    // One lock order, serial then state: the adapter call below runs without
    // the state lock, and a concurrent restore cannot interleave with it.
    let _serial = lock(&shared.privacy_serial);
    let refused = |status: u8, why: &'static str| {
        tracing::info!(
            target: crate::trace::SYSTEM,
            connection = admission.connection_id,
            display_private,
            input_blocked,
            why,
            "privacy refused"
        );
        Err(status)
    };
    let (actions, previous) = {
        let state = lock(&shared.state);
        if !privacy_authorized(&state, admission, cancelled) {
            return refused(3, "the viewer may not use system actions now");
        }
        if let Some(lease) = state.privacy.as_ref() {
            if lease.connection_id != admission.connection_id || lease.service_id != service_id {
                return refused(4, "another viewer holds privacy");
            }
        }
        let enabling = display_private || input_blocked;
        if enabling && !state.system_policy.allow_privacy {
            return refused(6, "the host's policy does not allow privacy");
        }
        if !enabling && state.privacy.is_none() {
            return Ok(encode_privacy(PrivacyState::default()));
        }
        // The lease owns its backend: level changes and release target the
        // adapter that applied the restriction, never the current slot (a
        // replacement may have swung it since). Fresh enables bind the slot.
        // The lease itself is written only after acknowledged outcomes
        // below, so it always reports attested state, never intent.
        let actions = match state.privacy.as_ref() {
            Some(lease) => lease.actions.clone(),
            None => lock(&shared.system_actions).clone().ok_or(1)?,
        };
        let previous = state.privacy.as_ref().map(|lease| {
            lease
                .actions
                .observed_privacy()
                .unwrap_or((lease.display_private, lease.input_blocked))
        });
        (actions, previous)
    };
    let support = actions.support();
    if display_private && !support.privacy || input_blocked && !support.input_block {
        return Err(1);
    }
    let authorized = || {
        let state = lock(&shared.state);
        privacy_authorized(&state, admission, cancelled)
            && (!(display_private || input_blocked) || state.system_policy.allow_privacy)
    };
    if !authorized() {
        return Err(3);
    }
    // An unheld, unrequested facility is never touched, even when the
    // backend supports it: calling a backend that rejects the value
    // (Windows display privacy rejects both booleans) must not fail an
    // input-only enable, and a no-op call must not invent a failure.
    // Asking an unsupported facility to turn on is already refused above.
    let (previous_display, previous_input) = previous.unwrap_or((false, false));
    let display_tried = display_private || previous_display;
    let input_tried = input_blocked || previous_input;
    // Independent attempts: a refused display never skips an input
    // restore, on the way in or out. Per-facility outcomes decide the
    // acknowledged remainder below.
    let display_ok = !display_tried || actions.set_display_private(display_private).is_ok();
    let input_ok =
        !input_tried || (authorized() && actions.set_input_blocked(input_blocked).is_ok());
    tracing::info!(
        target: crate::trace::SYSTEM,
        connection = admission.connection_id,
        display_private,
        input_blocked,
        display_ok,
        input_ok,
        "privacy applied"
    );
    let mut state = lock(&shared.state);
    if !privacy_authorized(&state, admission, cancelled)
        || ((display_private || input_blocked) && !state.system_policy.allow_privacy)
    {
        // Native acknowledgement can arrive after revoke/cancellation. Restore
        // independently and retain failed restores so local cleanup can retry.
        let lease = PrivacyLease {
            connection_id: admission.connection_id,
            service_id,
            display_private: if display_ok {
                display_private
            } else {
                previous_display
            },
            input_blocked: if input_ok {
                input_blocked
            } else {
                previous_input
            },
            actions: actions.clone(),
        };
        drop(state);
        let ((display_held, input_held), _) = restore_privacy_lease(&lease);
        state = lock(&shared.state);
        state.privacy = if display_held || input_held {
            Some(PrivacyLease {
                display_private: display_held,
                input_blocked: input_held,
                ..lease
            })
        } else {
            None
        };
        return Err(3);
    }
    if display_ok && input_ok {
        // All attempted calls acknowledged: the requested flags are the
        // state, and a full disable drops the lease.
        let enabling = display_private || input_blocked;
        state.privacy = if enabling {
            Some(PrivacyLease {
                connection_id: admission.connection_id,
                service_id,
                display_private,
                input_blocked,
                actions: actions.clone(),
            })
        } else {
            None
        };
        return Ok(encode_privacy(current_privacy(&state)));
    }
    drop(state);
    // The adapter refused partway. A failed call changed nothing — trust
    // the ack and keep the previous flag. A fresh-apply (newly restricted
    // where nothing was held) reverts toward false so a failed enable
    // stays all-or-nothing; narrowing keeps its partial progress. The
    // lease holds the acknowledged remainder, never the requested flags.
    let revert_display = display_tried && display_ok && display_private && !previous_display;
    let revert_input = input_tried && input_ok && input_blocked && !previous_input;
    let display_held = if !display_tried {
        false
    } else if !display_ok {
        previous_display
    } else if revert_display {
        actions.set_display_private(false).is_err()
    } else {
        display_private
    };
    let input_held = if !input_tried {
        false
    } else if !input_ok {
        previous_input
    } else if revert_input {
        actions.set_input_blocked(false).is_err()
    } else {
        input_blocked
    };
    let mut state = lock(&shared.state);
    if previous.is_some() {
        state.privacy = None;
    }
    if display_held || input_held {
        if state.privacy.is_none() {
            state.privacy = Some(PrivacyLease {
                connection_id: admission.connection_id,
                service_id,
                display_private: display_held,
                input_blocked: input_held,
                actions: actions.clone(),
            });
        }
    }
    Err(1)
}

/// Acknowledged per-facility restore of a retained lease through its own
/// backend. Each held facility is attempted independently — a display
/// failure never skips an input unblock — and facilities the lease never
/// held are not touched, so an unsupported backend cannot fail the
/// restore of a restriction it never applied. Returns what remains held
/// plus the first native error, for the caller to report for retry.
fn restore_privacy_lease(lease: &PrivacyLease) -> ((bool, bool), Option<Error>) {
    let display_error = if lease.display_private {
        lease.actions.set_display_private(false).err()
    } else {
        None
    };
    let input_error = if lease.input_blocked {
        lease.actions.set_input_blocked(false).err()
    } else {
        None
    };
    let remainder = (display_error.is_some(), input_error.is_some());
    let error = display_error.or(input_error);
    match &error {
        None => tracing::info!(
            target: crate::trace::SYSTEM,
            connection = lease.connection_id,
            display = lease.display_private,
            input = lease.input_blocked,
            "privacy restored"
        ),
        Some(error) => tracing::warn!(
            target: crate::trace::SYSTEM,
            connection = lease.connection_id,
            display_held = remainder.0,
            input_held = remainder.1,
            failure = error.name(),
            "privacy not restored: it stays held until a restore works"
        ),
    }
    (remainder, error)
}

/// Releases the active privacy lease after a saved policy revoked its
/// authority. Same serial-then-state order as enable and owner teardown,
/// so no path interleaves; restoring exactly once like teardown.
/// Restoration targets the backend retained in the lease, never the
/// current slot; a failed native restore keeps the lease (ownership is
/// not forgotten and Core keeps reporting the restriction) and the
/// failure reports to the caller for retry.
pub(super) fn release_all_privacy(shared: &Shared) -> Result<(), Error> {
    let _serial = lock(&shared.privacy_serial);
    let held = {
        let mut state = lock(&shared.state);
        state.privacy.take()
    };
    let Some(lease) = held else {
        return Ok(());
    };
    let ((display_held, input_held), error) = restore_privacy_lease(&lease);
    if display_held || input_held {
        lock(&shared.state).privacy = Some(PrivacyLease {
            connection_id: lease.connection_id,
            service_id: lease.service_id,
            display_private: display_held,
            input_blocked: input_held,
            actions: lease.actions.clone(),
        });
    }
    match error {
        Some(error) => Err(error),
        None => Ok(()),
    }
}

/// Restores local display and input when the holding owner ends for any
/// reason: explicit close, revoke, parent loss or shutdown. Restoration
/// targets the backend retained in the lease, never the current slot; a
/// failed native restore keeps the lease so ownership is not forgotten
/// and Core keeps reporting the restriction until it clears.
pub(super) fn release_privacy_for(shared: &Shared, connection_id: u64, service_id: u64) {
    let _serial = lock(&shared.privacy_serial);
    let held = {
        let mut state = lock(&shared.state);
        let ours = matches!(&state.privacy, Some(lease)
            if lease.connection_id == connection_id && lease.service_id == service_id);
        if ours {
            state.privacy.take()
        } else {
            None
        }
    };
    if let Some(lease) = held {
        let ((display_held, input_held), _) = restore_privacy_lease(&lease);
        if display_held || input_held {
            lock(&shared.state).privacy = Some(PrivacyLease {
                connection_id: lease.connection_id,
                service_id: lease.service_id,
                display_private: display_held,
                input_blocked: input_held,
                actions: lease.actions.clone(),
            });
        }
    }
}

pub(super) async fn serve(
    shared: Arc<Shared>,
    id: u64,
    mut send: SendStream,
    mut recv: RecvStream,
    mut parent_stop: tokio::sync::watch::Receiver<bool>,
    mut service_stop: tokio::sync::watch::Receiver<bool>,
) -> Result<(), Error> {
    // The controller's open waits for this acknowledgment before queueing
    // requests; without it every open times out.
    write(&mut send, vec![VERSION, 0, 0]).await?;
    let outcome = serve_inner(
        &shared,
        id,
        &mut send,
        &mut recv,
        &mut parent_stop,
        &mut service_stop,
    )
    .await;
    let holder = {
        let state = lock(&shared.state);
        state
            .hosted_services
            .get(&id)
            .map(|record| (record.info.connection_id, record.info.generation))
    };
    let cleanup_shared = shared.clone();
    tokio::task::spawn_blocking(move || {
        let connection_id = holder.map(|(connection_id, _)| connection_id).or_else(|| {
            lock(&cleanup_shared.state)
                .privacy
                .as_ref()
                .filter(|lease| lease.service_id == id)
                .map(|lease| lease.connection_id)
        });
        if let Some(connection_id) = connection_id {
            release_privacy_for(&cleanup_shared, connection_id, id);
        }
    })
    .await
    .map_err(|_| Error::ShutDown)?;
    outcome
}

/// Native work owns late-result cleanup even after the async waiter leaves.
async fn privacy_request(
    shared: &Arc<Shared>,
    admission: Admission,
    display: bool,
    input: bool,
    parent_stop: &mut tokio::sync::watch::Receiver<bool>,
    service_stop: &mut tokio::sync::watch::Receiver<bool>,
) -> Result<Option<Result<Vec<u8>, u8>>, Error> {
    struct CancelOnDrop(Arc<std::sync::atomic::AtomicBool>);
    impl Drop for CancelOnDrop {
        fn drop(&mut self) {
            self.0.store(true, std::sync::atomic::Ordering::Release);
        }
    }
    let cancelled = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let _cancel = CancelOnDrop(cancelled.clone());
    let worker_shared = shared.clone();
    let work = tokio::task::spawn_blocking(move || {
        set_privacy_inner(
            &worker_shared,
            &admission,
            admission.service_id,
            display,
            input,
            &cancelled,
        )
    });
    tokio::select! {
        biased;
        _ = parent_stop.changed() => Ok(None),
        _ = service_stop.changed() => Ok(None),
        reply = work => Ok(Some(reply.map_err(|_| Error::ShutDown)?)),
    }
}

async fn serve_inner(
    shared: &Arc<Shared>,
    id: u64,
    send: &mut SendStream,
    recv: &mut RecvStream,
    parent_stop: &mut tokio::sync::watch::Receiver<bool>,
    service_stop: &mut tokio::sync::watch::Receiver<bool>,
) -> Result<(), Error> {
    loop {
        let request = tokio::select! {
            biased;
            _ = parent_stop.changed() => break,
            _ = service_stop.changed() => break,
            request = read(recv) => request?,
        };
        let admission = match admit(shared, id) {
            Ok(admission) => admission,
            Err(code) => {
                write(send, vec![1, code, 0]).await?;
                if code == 3 {
                    break;
                }
                continue;
            }
        };
        if request.is_empty() {
            return Err(Error::Malformed);
        }
        match request[0] {
            REQ_EXECUTE if request.len() == 2 => {
                let action = match SystemAction::decode(request[1]) {
                    Ok(action) => action,
                    Err(_) => return Err(Error::Malformed),
                };
                // The viewer sends nothing while it waits for this reply:
                // anything it sends now, its stream closing included, means
                // it gave up. A native action that never answers then holds
                // no owner, and the viewer's next request is served.
                let reply = tokio::select! {
                    biased;
                    _ = parent_stop.changed() => break,
                    _ = service_stop.changed() => break,
                    _ = read(recv) => break,
                    reply = execute(shared, admission, action) => reply?,
                };
                write(send, reply).await?;
            }
            REQ_QUERY if request.len() == 9 => {
                let id =
                    u64::from_be_bytes(request[1..9].try_into().map_err(|_| Error::Malformed)?);
                match query(shared, &admission, id) {
                    Ok(reply) => write(send, reply).await?,
                    Err(code) => write(send, vec![1, code, 0]).await?,
                }
            }
            REQ_CANCEL if request.len() == 9 => {
                let id =
                    u64::from_be_bytes(request[1..9].try_into().map_err(|_| Error::Malformed)?);
                match cancel(
                    shared,
                    Some(&admission),
                    id,
                    "cancelled by the computer that asked",
                ) {
                    Ok(operation) => write(send, encode_operation(&operation)).await?,
                    Err(code) => write(send, vec![1, code, 0]).await?,
                }
            }
            REQ_SUPPORT if request.len() == 1 => {
                write(send, support(shared)).await?;
            }
            REQ_SET_PRIVACY if request.len() == 3 => {
                let bit = |byte: u8| match byte {
                    0 => Ok(false),
                    1 => Ok(true),
                    _ => Err(Error::Malformed),
                };
                let display = bit(request[1])?;
                let input = bit(request[2])?;
                let Some(reply) =
                    privacy_request(shared, admission, display, input, parent_stop, service_stop)
                        .await?
                else {
                    break;
                };
                match reply {
                    Ok(reply) => write(send, reply).await?,
                    Err(code) => write(send, vec![1, code, 0]).await?,
                }
            }
            REQ_GET_PRIVACY if request.len() == 1 => {
                let reply = encode_privacy(current_privacy(&lock(&shared.state)));
                write(send, reply).await?;
            }
            _ => return Err(Error::Malformed),
        }
    }
    Ok(())
}

#[cfg(test)]
mod review_authority_tests {
    use super::*;
    struct Actions(std::sync::atomic::AtomicUsize);
    impl crate::HostSystemActions for Actions {
        fn support(&self) -> SystemSupport {
            SystemSupport {
                restart: true,
                ..SystemSupport::default()
            }
        }
        fn lock_workstation(&self) -> Result<(), Error> {
            unreachable!()
        }
        fn restart(&self) -> Result<(), Error> {
            self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(())
        }
        fn shutdown(&self) -> Result<(), Error> {
            unreachable!()
        }
        fn set_display_private(&self, _: bool) -> Result<(), Error> {
            unreachable!()
        }
        fn set_input_blocked(&self, _: bool) -> Result<(), Error> {
            unreachable!()
        }
        fn session_state(&self) -> HostSessionState {
            HostSessionState::unknown("fake")
        }
    }
    #[test]
    fn review_queued_restart_rechecks_disabled_policy() {
        let host = crate::tests::support::start_direct();
        let controller = crate::tests::support::start_direct();
        let (admission, _system) = admitted_system_owner(&host, &controller);
        let actions = Arc::new(Actions(std::sync::atomic::AtomicUsize::new(0)));
        host.set_system_actions(Some(actions.clone())).unwrap();
        host.set_system_policy(crate::SystemPolicy {
            allow_restart: true,
            ..Default::default()
        })
        .unwrap();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .max_blocking_threads(1)
            .build()
            .unwrap();
        runtime.block_on(async {
            let (release,blocked)=std::sync::mpsc::sync_channel(1);
            let (entered,ready)=std::sync::mpsc::sync_channel(1);
            let blocker=tokio::task::spawn_blocking(move || { entered.send(()).unwrap(); blocked.recv().unwrap(); });
            ready.recv().unwrap();
            let mut action=Box::pin(start(&host.shared,admission,SystemAction::Restart));
            tokio::select! { biased; _=&mut action => panic!("execution unexpectedly completed"), _=tokio::task::yield_now()=>{} }
            host.set_system_policy(crate::SystemPolicy::default()).unwrap();
            release.send(()).unwrap();
            blocker.await.unwrap();
            let _=action.await.unwrap();
        });
        assert_eq!(
            actions.0.load(std::sync::atomic::Ordering::SeqCst),
            0,
            "queued restart ran after host disabled it"
        );
        host.shutdown();
        controller.shutdown();
    }
}

/// Admits a real System owner over a services-only parent and returns the
/// admission the serve loop would hand the worker. Test-only.
#[cfg(test)]
fn admitted_system_owner(host: &Core, controller: &Core) -> (Admission, RemoteSystemService) {
    use crate::tests::support::pair_for_control;
    pair_for_control(host, controller);
    host.set_service_sharing(crate::ServiceKind::System, true)
        .unwrap();
    let parent = controller
        .begin_connect_services(&host.endpoint_id())
        .unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(8);
    while controller.controller_session(parent.id).unwrap().phase != SessionPhase::Active {
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let parent = controller.controller_session(parent.id).unwrap();
    let owner = controller
        .attach_controller_service(parent.id, parent.generation, ServiceKind::System)
        .unwrap();
    let _system = controller
        .open_system_service(owner.id, owner.generation)
        .unwrap();
    let hosted = host
        .hosted_services()
        .unwrap()
        .into_iter()
        .find(|record| record.kind == ServiceKind::System)
        .unwrap();
    // Keep the live owner through the test: dropping it here races the
    // request's fresh authority check with asynchronous owner retirement.
    (admit(&host.shared, hosted.id).unwrap(), _system)
}

#[cfg(test)]
mod worker_start_gate_tests {
    use super::*;
    use crate::tests::support::{pair_for_control, start_direct};

    struct Actions(std::sync::atomic::AtomicUsize);
    impl crate::HostSystemActions for Actions {
        fn support(&self) -> SystemSupport {
            SystemSupport {
                restart: true,
                ..SystemSupport::default()
            }
        }
        fn lock_workstation(&self) -> Result<(), Error> {
            unreachable!()
        }
        fn restart(&self) -> Result<(), Error> {
            self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(())
        }
        fn shutdown(&self) -> Result<(), Error> {
            unreachable!()
        }
        fn set_display_private(&self, _: bool) -> Result<(), Error> {
            unreachable!()
        }
        fn set_input_blocked(&self, _: bool) -> Result<(), Error> {
            unreachable!()
        }
        fn session_state(&self) -> HostSessionState {
            HostSessionState::unknown("fake")
        }
    }

    /// Runs one action through a saturated blocking pool, applies a
    /// mid-queue authority change, and returns the recorded
    /// (state, detail) once the worker settles.
    fn stood_down_outcome(
        runtime: &tokio::runtime::Runtime,
        core: &Core,
        admission: Admission,
        change: impl FnOnce(),
    ) -> (OperationState, String) {
        runtime.block_on(async {
            let (release, blocked) = std::sync::mpsc::sync_channel(1);
            let (entered, ready) = std::sync::mpsc::sync_channel(1);
            let blocker = tokio::task::spawn_blocking(move || {
                entered.send(()).unwrap();
                blocked.recv().unwrap();
            });
            ready.recv().unwrap();
            // The start gate runs for a lock at once and for a restart once
            // its notice ends; both stand down the same way.
            let mut action = Box::pin(start(&core.shared, admission, SystemAction::Restart));
            tokio::select! { biased; _=&mut action => panic!("execution unexpectedly completed"), _=tokio::task::yield_now()=>{} }
            change();
            release.send(()).unwrap();
            blocker.await.unwrap();
            match action.await.unwrap() {
                Started::StoodDown(detail) => (OperationState::Refused, detail.to_owned()),
                Started::Ran(_) => (OperationState::Executed, "ran".to_owned()),
            }
        })
    }

    #[test]
    fn rereview_queued_restart_must_stand_down_when_system_owner_closes() {
        let host = start_direct();
        let controller = start_direct();
        pair_for_control(&host, &controller);
        let actions = Arc::new(Actions(std::sync::atomic::AtomicUsize::new(0)));
        host.set_system_actions(Some(actions.clone())).unwrap();
        host.set_service_sharing(crate::ServiceKind::System, true)
            .unwrap();
        host.set_system_policy(crate::SystemPolicy {
            allow_restart: true,
            ..Default::default()
        })
        .unwrap();
        let parent = controller
            .begin_connect_services(&host.endpoint_id())
            .unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(8);
        while controller.controller_session(parent.id).unwrap().phase != SessionPhase::Active {
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(10));
        }
        let parent = controller.controller_session(parent.id).unwrap();
        let owner = controller
            .attach_controller_service(parent.id, parent.generation, ServiceKind::System)
            .unwrap();
        let _system = controller
            .open_system_service(owner.id, owner.generation)
            .unwrap();
        let hosted = host
            .hosted_services()
            .unwrap()
            .into_iter()
            .find(|r| r.kind == ServiceKind::System)
            .unwrap();
        let admission = admit(&host.shared, hosted.id).unwrap();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .max_blocking_threads(1)
            .build()
            .unwrap();
        let outcome = stood_down_outcome(&runtime, &host, admission, || {
            host.close_hosted_service(hosted.id, hosted.generation)
                .unwrap()
        });
        let executions = actions.0.load(std::sync::atomic::Ordering::SeqCst);
        println!("owner-close queued result={outcome:?}, fake native executions={executions}");
        host.shutdown();
        controller.shutdown();
        assert_eq!(executions, 0, "closed System owner executed queued restart");
        assert!(
            outcome.1.contains("system owner closed"),
            "unexpected detail: {:?}",
            outcome.1
        );
    }

    #[test]
    fn rereview_queued_restart_must_stand_down_when_parent_stopping() {
        // Adapted from the v3 probe: the admission now carries the exact
        // System owner, so the fixture opens a real owner (otherwise the
        // owner check, not the parent check, would fire).
        let host = start_direct();
        let controller = start_direct();
        let (admission, _system) = admitted_system_owner(&host, &controller);
        let actions = Arc::new(Actions(std::sync::atomic::AtomicUsize::new(0)));
        host.set_system_actions(Some(actions.clone())).unwrap();
        host.set_system_policy(crate::SystemPolicy {
            allow_restart: true,
            ..Default::default()
        })
        .unwrap();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .max_blocking_threads(1)
            .build()
            .unwrap();
        let outcome = stood_down_outcome(&runtime, &host, admission, || {
            lock(&host.shared.state)
                .incoming
                .get_mut(&admission.connection_id)
                .unwrap()
                .phase = SessionPhase::Stopping;
        });
        let executions = actions.0.load(std::sync::atomic::Ordering::SeqCst);
        println!("stopping-parent queued result={outcome:?}, fake native executions={executions}");
        host.shutdown();
        controller.shutdown();
        assert_eq!(executions, 0, "Stopping parent executed queued restart");
        assert!(
            outcome.1.contains("connection not active"),
            "unexpected detail: {:?}",
            outcome.1
        );
    }

    #[test]
    fn queued_restart_stands_down_as_refused_on_policy_backend_and_grant_changes() {
        let host = start_direct();
        let controller = start_direct();
        let (admission, _system) = admitted_system_owner(&host, &controller);
        let actions = Arc::new(Actions(std::sync::atomic::AtomicUsize::new(0)));
        host.set_system_actions(Some(actions.clone())).unwrap();
        let allow_restart = || crate::SystemPolicy {
            allow_restart: true,
            ..Default::default()
        };
        host.set_system_policy(allow_restart()).unwrap();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .max_blocking_threads(1)
            .build()
            .unwrap();
        // Mid-queue policy flip: refused, native never runs.
        let (state, detail) = stood_down_outcome(&runtime, &host, admission, || {
            host.set_system_policy(crate::SystemPolicy::default())
                .unwrap();
        });
        assert_eq!(state, OperationState::Refused);
        assert!(detail.contains("disabled by host policy"), "{detail}");
        host.set_system_policy(allow_restart()).unwrap();
        // Mid-queue backend removal: refused, native never runs.
        let (state, detail) = stood_down_outcome(&runtime, &host, admission, || {
            host.set_system_actions(None).unwrap();
        });
        assert_eq!(state, OperationState::Refused);
        assert!(detail.contains("no system backend"), "{detail}");
        host.set_system_actions(Some(actions.clone())).unwrap();
        // Mid-queue permission revoke: refused, native never runs.
        let (state, detail) = stood_down_outcome(&runtime, &host, admission, || {
            host.set_service_sharing(crate::ServiceKind::System, false)
                .unwrap();
        });
        assert_eq!(state, OperationState::Refused);
        assert!(detail.contains("system permission revoked"), "{detail}");
        assert_eq!(
            actions.0.load(std::sync::atomic::Ordering::SeqCst),
            0,
            "native restart ran despite stand-down"
        );
        host.shutdown();
        controller.shutdown();
    }
}

#[cfg(test)]
mod v7_privacy_admission_probe {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    struct Fake(AtomicBool, AtomicBool);
    impl crate::HostSystemActions for Fake {
        fn support(&self) -> SystemSupport {
            SystemSupport {
                input_block: true,
                ..Default::default()
            }
        }
        fn lock_workstation(&self) -> Result<(), Error> {
            unreachable!()
        }
        fn restart(&self) -> Result<(), Error> {
            unreachable!()
        }
        fn shutdown(&self) -> Result<(), Error> {
            unreachable!()
        }
        fn set_display_private(&self, _: bool) -> Result<(), Error> {
            unreachable!()
        }
        fn set_input_blocked(&self, held: bool) -> Result<(), Error> {
            self.0.store(held, Ordering::SeqCst);
            if held {
                self.1.store(true, Ordering::SeqCst);
            }
            Ok(())
        }
        fn session_state(&self) -> HostSessionState {
            HostSessionState::unknown("fake")
        }
    }
    #[test]
    fn v7_stale_privacy_admission_after_revoke_must_not_apply() {
        let host = crate::tests::support::start_direct();
        let controller = crate::tests::support::start_direct();
        let actions = Arc::new(Fake(AtomicBool::new(false), AtomicBool::new(false)));
        host.set_system_actions(Some(actions.clone())).unwrap();
        host.set_system_policy(crate::SystemPolicy {
            allow_privacy: true,
            ..Default::default()
        })
        .unwrap();
        let (admission, _system) = admitted_system_owner(&host, &controller);
        let serial = lock(&host.shared.privacy_serial);
        let queued_shared = host.shared.clone();
        let (entered, ready) = std::sync::mpsc::sync_channel(1);
        let worker = std::thread::spawn(move || {
            entered.send(()).unwrap();
            set_privacy(
                &queued_shared,
                &admission,
                admission.service_id,
                false,
                true,
            )
        });
        ready.recv().unwrap();
        std::thread::scope(|scope| {
            let revoking = scope.spawn(|| host.set_service_sharing(ServiceKind::System, false));
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
            while admit(&host.shared, admission.service_id).is_ok() {
                assert!(
                    std::time::Instant::now() < deadline,
                    "revocation did not fence admission"
                );
                std::thread::yield_now();
            }
            drop(serial);
            revoking.join().unwrap().unwrap();
        });
        let outcome = worker.join().unwrap();
        let applied = actions.1.load(Ordering::SeqCst);
        let _ = release_all_privacy(&host.shared);
        host.shutdown();
        controller.shutdown();
        assert!(outcome.is_err() && !applied, "stale privacy request applied after revoke: reply={outcome:?}, input_blocked={applied}");
    }
    #[test]
    fn late_privacy_ack_after_cancellation_is_restored() {
        struct Delayed {
            held: AtomicBool,
            entered: std::sync::mpsc::SyncSender<()>,
            proceed: std::sync::Mutex<std::sync::mpsc::Receiver<()>>,
        }
        impl crate::HostSystemActions for Delayed {
            fn support(&self) -> SystemSupport {
                SystemSupport {
                    input_block: true,
                    ..Default::default()
                }
            }
            fn lock_workstation(&self) -> Result<(), Error> {
                unreachable!()
            }
            fn restart(&self) -> Result<(), Error> {
                unreachable!()
            }
            fn shutdown(&self) -> Result<(), Error> {
                unreachable!()
            }
            fn set_display_private(&self, _: bool) -> Result<(), Error> {
                unreachable!()
            }
            fn set_input_blocked(&self, held: bool) -> Result<(), Error> {
                if held {
                    self.entered.send(()).unwrap();
                    lock(&self.proceed).recv().unwrap();
                }
                self.held.store(held, Ordering::SeqCst);
                Ok(())
            }
            fn session_state(&self) -> HostSessionState {
                HostSessionState::unknown("delayed")
            }
        }
        let host = crate::tests::support::start_direct();
        let controller = crate::tests::support::start_direct();
        let (entered, ready) = std::sync::mpsc::sync_channel(1);
        let (proceed, wait) = std::sync::mpsc::channel();
        let actions = Arc::new(Delayed {
            held: AtomicBool::new(false),
            entered,
            proceed: Mutex::new(wait),
        });
        host.set_system_actions(Some(actions.clone())).unwrap();
        host.set_system_policy(crate::SystemPolicy {
            allow_privacy: true,
            ..Default::default()
        })
        .unwrap();
        let (admission, _system) = admitted_system_owner(&host, &controller);
        let cancelled = Arc::new(AtomicBool::new(false));
        let worker_cancel = cancelled.clone();
        let shared = host.shared.clone();
        let worker = std::thread::spawn(move || {
            set_privacy_inner(
                &shared,
                &admission,
                admission.service_id,
                false,
                true,
                &worker_cancel,
            )
        });
        ready.recv_timeout(Duration::from_secs(5)).unwrap();
        cancelled.store(true, Ordering::Release);
        proceed.send(()).unwrap();
        assert!(worker.join().unwrap().is_err());
        assert!(!actions.held.load(Ordering::SeqCst));
        assert!(lock(&host.shared.state).privacy.is_none());
        host.shutdown();
        controller.shutdown();
    }
    #[test]
    fn slow_privacy_ack_leaves_async_waiter_cancellable() {
        struct Delayed {
            entered: tokio::sync::mpsc::UnboundedSender<()>,
            proceed: Mutex<std::sync::mpsc::Receiver<()>>,
            restored: AtomicBool,
        }
        impl crate::HostSystemActions for Delayed {
            fn support(&self) -> SystemSupport {
                SystemSupport {
                    input_block: true,
                    ..Default::default()
                }
            }
            fn lock_workstation(&self) -> Result<(), Error> {
                unreachable!()
            }
            fn restart(&self) -> Result<(), Error> {
                unreachable!()
            }
            fn shutdown(&self) -> Result<(), Error> {
                unreachable!()
            }
            fn set_display_private(&self, _: bool) -> Result<(), Error> {
                unreachable!()
            }
            fn set_input_blocked(&self, held: bool) -> Result<(), Error> {
                if held {
                    self.entered.send(()).unwrap();
                    lock(&self.proceed)
                        .recv_timeout(Duration::from_secs(5))
                        .unwrap();
                } else {
                    self.restored.store(true, Ordering::SeqCst);
                }
                Ok(())
            }
            fn session_state(&self) -> HostSessionState {
                HostSessionState::unknown("delayed")
            }
        }
        let host = crate::tests::support::start_direct();
        let controller = crate::tests::support::start_direct();
        let (entered, mut ready) = tokio::sync::mpsc::unbounded_channel();
        let (proceed, wait) = std::sync::mpsc::channel();
        let actions = Arc::new(Delayed {
            entered,
            proceed: Mutex::new(wait),
            restored: AtomicBool::new(false),
        });
        host.set_system_actions(Some(actions.clone())).unwrap();
        host.set_system_policy(crate::SystemPolicy {
            allow_privacy: true,
            ..Default::default()
        })
        .unwrap();
        let (admission, _system) = admitted_system_owner(&host, &controller);
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            let (_parent, mut parent_stop) = tokio::sync::watch::channel(false);
            let (close, mut service_stop) = tokio::sync::watch::channel(false);
            let mut request = Box::pin(privacy_request(
                &host.shared,
                admission,
                false,
                true,
                &mut parent_stop,
                &mut service_stop,
            ));
            tokio::select! {
                _ = &mut request => panic!("native request completed before acknowledgement"),
                _ = ready.recv() => {},
            }
            close.send(true).unwrap();
            let result = tokio::time::timeout(Duration::from_secs(1), &mut request)
                .await
                .unwrap()
                .unwrap();
            assert!(result.is_none(), "close did not cancel async waiter");
            proceed.send(()).unwrap();
            tokio::time::timeout(Duration::from_secs(2), async {
                while !actions.restored.load(Ordering::SeqCst) {
                    tokio::task::yield_now().await;
                }
            })
            .await
            .unwrap();
        });
        assert!(lock(&host.shared.state).privacy.is_none());
        host.shutdown();
        controller.shutdown();
    }
}
