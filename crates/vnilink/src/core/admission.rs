// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! Ask-each-time admission: the person at this computer accepts a paired
//! computer before it starts to use what is shared.
//!
//! The presence link to every paired computer is idle and neutral, so a link
//! coming up asks nothing. Asking belongs where use begins: a controller
//! sends `ADMIT` on the service stream before it opens a service or asks for
//! one of a service's lists, and waits for the answer on that stream. The
//! host answers at once when the peer is approved, refuses at once when the
//! kind is not shared, and otherwise publishes
//! [`CoreEvent::AdmissionRequested`], tells the controller it is asking, and
//! waits for [`Core::answer_admission_request`] or the answer's deadline.
//! Concurrent asks from one peer join one request, so one question covers a
//! desktop that opens its video and input together.
//!
//! An approval belongs to the peer, not to one service or link. It lasts
//! while the peer has any hosted service open, and for a grace after
//! acceptance and after its last hosted service ends, so a dropped link that
//! reopens its services, or a view that reopens, does not ask again.
//! Screenshots never ask: one is admitted only while an approval is live.
//! [`crate::SharingPolicy::unattended`] approves every paired computer; turning
//! it off forgets the approvals it made. Forgetting a device ends its
//! request and approval. Every gate that admits a service checks
//! [`approved`] besides the sharing consent.
// trace: records every ignored failure

use super::service_stream::{preamble, read, service_kind, status, wire_kind, write, VERSION};
use super::*;
use crate::protocol::SessionStamp;
use crate::{AdmissionRequest, ServiceKind};
use iroh::endpoint::{Connection, SendStream};
use std::time::Instant;

/// The service-stream verb that asks to be admitted.
pub(super) const ADMIT: u8 = 6;
/// How long the host waits for its person's answer.
const ANSWER_WAIT: Duration = Duration::from_secs(30);
/// How long an approval outlives its acceptance and the peer's last service.
const GRACE: Duration = Duration::from_secs(60);
/// How long the controller waits for the host's first answer.
const FIRST_ANSWER: Duration = Duration::from_secs(5);
/// The longest the controller waits for the person's answer: the host's own
/// wait plus the time its answer takes to arrive.
const PATIENCE: Duration = Duration::from_secs(40);

/// What the host's first frame says besides a refusal.
const ADMITTED: u8 = 0;
const ASKING: u8 = 1;

/// The wire refusals that are admission's own; [`status`] reads them.
pub(super) const DECLINED: u8 = 8;
pub(super) const NOT_ADMITTED: u8 = 9;
pub(super) const UNANSWERED: u8 = 10;

/// Approvals, open requests and, as a controller, the peers it waits on.
pub(super) struct Admission {
    /// When each peer's approval ends unless it is using a service, by
    /// endpoint id.
    approvals: HashMap<String, Instant>,
    requests: HashMap<u64, Request>,
    next_request: u64,
    /// Peers this computer, as a controller, waits on, with how many asks.
    waiting: HashMap<String, usize>,
    answer_wait: Duration,
    grace: Duration,
}

struct Request {
    endpoint_id: String,
    deadline: Instant,
    answer: tokio::sync::watch::Sender<Option<bool>>,
    /// Asks waiting on it; the last to leave unanswered ends it.
    waiters: usize,
}

impl Default for Admission {
    fn default() -> Self {
        Self {
            approvals: HashMap::new(),
            requests: HashMap::new(),
            next_request: 1,
            waiting: HashMap::new(),
            answer_wait: ANSWER_WAIT,
            grace: GRACE,
        }
    }
}

impl Admission {
    /// Forgets every approval; open requests still wait for their answer.
    pub(super) fn forget_approvals(&mut self) {
        self.approvals.clear();
    }
}

/// Whether the paired computer `endpoint_id` may start using a shared
/// service now.
pub(super) fn approved(state: &State, endpoint_id: &str, now: Instant) -> bool {
    state.trust.sharing.unattended
        || state
            .admission
            .approvals
            .get(endpoint_id)
            .is_some_and(|until| *until > now)
        || state
            .hosted_services
            .values()
            .any(|record| record.info.peer_id == endpoint_id && !record.info.phase.finished())
}

/// Starts the grace of `endpoint_id`'s approval again: one of its hosted
/// services ended. Its other services keep it approved meanwhile.
pub(super) fn service_ended(state: &mut State, endpoint_id: &str, now: Instant) {
    let until = now + state.admission.grace;
    state
        .admission
        .approvals
        .insert(endpoint_id.to_owned(), until);
}

/// Ends `endpoint_id`'s approval and request: the device was forgotten.
pub(super) fn forget(shared: &Shared, state: &mut State, endpoint_id: &str) {
    state.admission.approvals.remove(endpoint_id);
    let ended: Vec<u64> = state
        .admission
        .requests
        .iter()
        .filter(|(_, request)| request.endpoint_id == endpoint_id)
        .map(|(id, _)| *id)
        .collect();
    for id in ended {
        end_request(shared, state, id, Some(false));
    }
}

/// Removes request `id`, telling its waiters `answer`, and says it ended.
fn end_request(shared: &Shared, state: &mut State, id: u64, answer: Option<bool>) {
    if let Some(request) = state.admission.requests.remove(&id) {
        request.answer.send_replace(answer);
        shared
            .events
            .publish(CoreEvent::AdmissionRequestEnded { request_id: id });
    }
}

impl Core {
    /// The requests waiting for the person at this computer, as a window
    /// that missed their events shows them.
    pub fn admission_requests(&self) -> Result<Vec<AdmissionRequest>, Error> {
        let _engine = self.engine();
        let state = self.alive_state()?;
        let now = Instant::now();
        let mut requests: Vec<_> = state
            .admission
            .requests
            .iter()
            .map(|(id, request)| AdmissionRequest {
                id: *id,
                endpoint_id: request.endpoint_id.clone(),
                remaining: request.deadline.saturating_duration_since(now),
            })
            .collect();
        requests.sort_by_key(|request| request.id);
        Ok(requests)
    }

    /// Answers request `id`. Accepting approves its computer for the grace,
    /// within which it opens what it asked for.
    pub fn answer_admission_request(&self, id: u64, accept: bool) -> Result<(), Error> {
        let _engine = self.engine();
        let mut state = self.alive_state()?;
        let endpoint_id = state
            .admission
            .requests
            .get(&id)
            .map(|request| request.endpoint_id.clone())
            .ok_or(Error::NoAdmissionRequest)?;
        tracing::info!(
            target: crate::trace::HOSTING,
            request = id,
            accept,
            "admission answered"
        );
        if accept {
            let until = Instant::now() + state.admission.grace;
            state.admission.approvals.insert(endpoint_id, until);
        }
        end_request(&self.shared, &mut state, id, Some(accept));
        Ok(())
    }

    /// Asks the host of connection `id` to admit this computer to `kind`,
    /// and calls `done` with the answer: at once when the host has approved
    /// it, after its person answers otherwise. Never blocks the caller.
    /// [`CoreEvent::AdmissionWaiting`] says while the host's person is asked.
    pub fn admit(
        &self,
        id: u64,
        generation: u64,
        kind: ServiceKind,
        done: impl FnOnce(Result<(), Error>) + Send + 'static,
    ) {
        let _engine = self.engine();
        let target = self.service_link(id, generation).and_then(|(link, stamp)| {
            let endpoint_id = live_session(&*self.alive_state()?, false, id)?
                .peer_id
                .clone();
            Ok((link, stamp, endpoint_id))
        });
        let (link, stamp, endpoint_id) = match target {
            Ok(target) => target,
            Err(error) => return done(Err(error)),
        };
        let shared = self.shared.clone();
        self.runtime.spawn(async move {
            done(ask(&shared, link, stamp, &endpoint_id, kind).await);
        });
    }

    /// Shortens the host's wait for an answer and an approval's grace, so a
    /// test sees each end without waiting for the product's.
    #[cfg(any(test, feature = "test-probes"))]
    pub fn set_admission_timing(&self, answer_wait: Duration, grace: Duration) {
        let _engine = self.engine();
        if let Ok(mut state) = self.alive_state() {
            state.admission.answer_wait = answer_wait;
            state.admission.grace = grace;
        }
    }
}

/// The controller's side of one ask.
async fn ask(
    shared: &Shared,
    link: Connection,
    stamp: SessionStamp,
    endpoint_id: &str,
    kind: ServiceKind,
) -> Result<(), Error> {
    let (mut send, mut recv, first) = tokio::time::timeout(FIRST_ANSWER, async {
        let (mut send, mut recv) = link.open_bi().await.map_err(Error::network_debug)?;
        write(&mut send, preamble(stamp, 0, ADMIT, wire_kind(kind))).await?;
        let first = read(&mut recv).await?;
        Ok::<_, Error>((send, recv, first))
    })
    .await
    .map_err(|_| Error::Timeout)??;
    let last = match status(&first)? {
        ADMITTED => None,
        ASKING => {
            let _waiting = WaitingOn::new(shared, endpoint_id);
            Some(tokio::time::timeout(PATIENCE, read(&mut recv)).await)
        }
        _ => return Err(Error::Malformed),
    };
    crate::ignored!(
        crate::trace::SERVICE,
        "a stream that cannot finish is closed with its link",
        send.finish()
    );
    match last {
        None => Ok(()),
        Some(last) => match status(&last.map_err(|_| Error::Timeout)??)? {
            ADMITTED => Ok(()),
            _ => Err(Error::Malformed),
        },
    }
}

/// One ask waiting on `endpoint_id`'s person. The first says this computer
/// waits on that computer, and the last to end says it no longer does.
struct WaitingOn<'a> {
    shared: &'a Shared,
    endpoint_id: &'a str,
}

impl<'a> WaitingOn<'a> {
    fn new(shared: &'a Shared, endpoint_id: &'a str) -> Self {
        let mut state = lock(&shared.state);
        let count = state
            .admission
            .waiting
            .entry(endpoint_id.to_owned())
            .or_default();
        *count += 1;
        if *count == 1 {
            shared.events.publish(CoreEvent::AdmissionWaiting {
                endpoint_id: endpoint_id.to_owned(),
                waiting: true,
            });
        }
        Self {
            shared,
            endpoint_id,
        }
    }
}

impl Drop for WaitingOn<'_> {
    fn drop(&mut self) {
        let mut state = lock(&self.shared.state);
        let last = state
            .admission
            .waiting
            .get_mut(self.endpoint_id)
            .is_some_and(|count| {
                *count = count.saturating_sub(1);
                *count == 0
            });
        if last {
            state.admission.waiting.remove(self.endpoint_id);
            self.shared.events.publish(CoreEvent::AdmissionWaiting {
                endpoint_id: self.endpoint_id.to_owned(),
                waiting: false,
            });
        }
    }
}

/// What the host does with one ask, decided under the state lock.
enum Decision {
    Answer(u8),
    Wait {
        id: u64,
        deadline: Instant,
        answer: tokio::sync::watch::Receiver<Option<bool>>,
    },
}

/// Leaves request `id` when an ask stops waiting, ending it unanswered when
/// it was the last.
struct Waiter<'a> {
    shared: &'a Shared,
    id: u64,
}

impl Drop for Waiter<'_> {
    fn drop(&mut self) {
        let mut state = lock(&self.shared.state);
        let last = state
            .admission
            .requests
            .get_mut(&self.id)
            .is_some_and(|request| {
                request.waiters = request.waiters.saturating_sub(1);
                request.waiters == 0
            });
        if last {
            end_request(self.shared, &mut state, self.id, None);
        }
    }
}

/// The host's side of one ask on `connection_id`; it reads nothing after
/// the preamble, and answers on `send`.
pub(super) async fn serve(
    shared: Arc<Shared>,
    connection_id: u64,
    peer: [u8; 32],
    stamp: SessionStamp,
    kind: u8,
    mut send: SendStream,
    mut parent_stop: tokio::sync::watch::Receiver<bool>,
) -> Result<(), Error> {
    let decision = {
        let mut state = lock(&shared.state);
        let now = Instant::now();
        let session = live_session(&state, true, connection_id)?;
        let endpoint_id = session.peer_id.clone();
        let stale = session.wire_stamp != stamp || session.peer_key != peer;
        match service_kind(kind) {
            _ if stale => Decision::Answer(3),
            None => Decision::Answer(1),
            Some(kind) if !super::service_permissions::admitted(&state, &peer, kind) => {
                Decision::Answer(6)
            }
            Some(_) if approved(&state, &endpoint_id, now) => Decision::Answer(ADMITTED),
            Some(ServiceKind::ScreenSnapshot) => Decision::Answer(NOT_ADMITTED),
            Some(_) => {
                let joined = state
                    .admission
                    .requests
                    .iter_mut()
                    .find(|(_, request)| request.endpoint_id == endpoint_id);
                if let Some((id, request)) = joined {
                    request.waiters += 1;
                    tracing::info!(
                        target: crate::trace::HOSTING,
                        connection = connection_id,
                        request = *id,
                        "admission joined"
                    );
                    Decision::Wait {
                        id: *id,
                        deadline: request.deadline,
                        answer: request.answer.subscribe(),
                    }
                } else {
                    let id = state.admission.next_request;
                    state.admission.next_request = id.checked_add(1).ok_or(Error::Busy)?;
                    let wait = state.admission.answer_wait;
                    let deadline = now + wait;
                    let (answer, receiver) = tokio::sync::watch::channel(None);
                    state.admission.requests.insert(
                        id,
                        Request {
                            endpoint_id: endpoint_id.clone(),
                            deadline,
                            answer,
                            waiters: 1,
                        },
                    );
                    tracing::info!(
                        target: crate::trace::HOSTING,
                        connection = connection_id,
                        request = id,
                        "admission asked"
                    );
                    shared.events.publish(CoreEvent::AdmissionRequested {
                        request_id: id,
                        endpoint_id,
                        remaining_ms: u64::try_from(wait.as_millis()).unwrap_or(u64::MAX),
                    });
                    Decision::Wait {
                        id,
                        deadline,
                        answer: receiver,
                    }
                }
            }
        }
    };
    let (id, deadline, mut answer) = match decision {
        Decision::Answer(code) => {
            write(&mut send, vec![VERSION, code, 0]).await?;
            crate::ignored!(
                crate::trace::SERVICE,
                "a stream that cannot finish is closed with its link",
                send.finish()
            );
            return Ok(());
        }
        Decision::Wait {
            id,
            deadline,
            answer,
        } => (id, deadline, answer),
    };
    let _waiter = Waiter {
        shared: &shared,
        id,
    };
    write(&mut send, vec![VERSION, 0, ASKING]).await?;
    let answered = async {
        loop {
            if let Some(accept) = *answer.borrow_and_update() {
                return Some(accept);
            }
            if answer.changed().await.is_err() {
                return *answer.borrow();
            }
        }
    };
    let code = tokio::select! {
        accept = answered => match accept {
            Some(true) => ADMITTED,
            Some(false) => DECLINED,
            None => UNANSWERED,
        },
        () = tokio::time::sleep_until(deadline.into()) => UNANSWERED,
        // The controller stopped waiting, or the link is ending: no one is
        // left to tell.
        _ = send.stopped() => return Ok(()),
        _ = parent_stop.wait_for(|stop| *stop) => return Ok(()),
    };
    write(&mut send, vec![VERSION, code, 0]).await?;
    crate::ignored!(
        crate::trace::SERVICE,
        "a stream that cannot finish is closed with its link",
        send.finish()
    );
    Ok(())
}
