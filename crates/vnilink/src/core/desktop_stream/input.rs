// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! Input a viewer sends: admitting it, ordering it and applying it to the host's desktop.

use super::*;

/// How long a pointer stays where it is before the viewer repeats that
/// position on the stream.
pub(super) const POINTER_SETTLE: Duration = Duration::from_millis(50);

/// Whether a pointer position sent `at` has rested long enough by `now`
/// to be repeated on the stream.
pub(super) fn pointer_settled(at: std::time::Instant, now: std::time::Instant) -> bool {
    now.saturating_duration_since(at) >= POINTER_SETTLE
}

/// Commands the host reads ahead of the one it applies. A burst of pointer
/// moves is applied as its newest.
pub(super) const HOST_COMMAND_BATCH: usize = 64;

/// One message from the viewer's control stream, decoded once.
pub(in crate::core) struct HostCommand {
    pub(in crate::core) body: Vec<u8>,
    /// The command a `COMMAND` message holds; `None` for the lease actions.
    pub(in crate::core) command: Option<Result<ControlMessage, Error>>,
}

impl HostCommand {
    pub(in crate::core) fn new(body: Vec<u8>) -> Self {
        let command = (body.first() == Some(&COMMAND) && body.len() >= 9)
            .then(|| ControlMessage::decode(&body[9..]));
        Self { body, command }
    }

    /// Whether applying it calls into the platform: lease actions, input
    /// and typed text.
    pub(super) fn native(&self) -> bool {
        match self.body.first() {
            Some(&RELEASE) | Some(&ACQUIRE_CONTROL) | Some(&YIELD_CONTROL) => true,
            _ => matches!(
                self.command,
                Some(Ok(ControlMessage::Input { .. }
                    | ControlMessage::Text { .. }
                    | ControlMessage::Chord { .. }))
            ),
        }
    }

    /// The pointer position it moves to, with the lease epoch it was sent
    /// under, if it is only that.
    pub(super) fn pointer_move(&self) -> Option<(u64, u64)> {
        match &self.command {
            Some(Ok(ControlMessage::Input {
                stamp,
                event: crate::InputEvent::Pointer { .. },
            })) => Some((
                u64::from_be_bytes(self.body.get(1..9)?.try_into().ok()?),
                stamp.id,
            )),
            _ => None,
        }
    }
}

/// Drops each pointer position another follows in the same batch, under the
/// same lease and for the same desktop: the host applies where the pointer
/// is now, not where it passed. Keys, buttons and wheel turns stay, in order.
pub(super) fn coalesce_pointer_moves(batch: &mut Vec<HostCommand>) {
    let moves: Vec<_> = batch.iter().map(HostCommand::pointer_move).collect();
    let mut index = 0;
    batch.retain(|_| {
        let superseded = moves[index].is_some() && moves.get(index + 1) == Some(&moves[index]);
        index += 1;
        !superseded
    });
}

pub(super) async fn host_receive(
    shared: &Arc<Shared>,
    desktop: &Arc<DesktopRuntime>,
    recv: &mut RecvStream,
) -> Result<(), Error> {
    let (queued, mut arrived) = tokio::sync::mpsc::channel(HOST_COMMAND_BATCH);
    let reader = async {
        loop {
            if queued.send(read(recv).await?).await.is_err() {
                return Ok(());
            }
        }
    };
    let applier = async {
        while let Some(first) = arrived.recv().await {
            let mut batch = vec![HostCommand::new(first)];
            while batch.len() < HOST_COMMAND_BATCH {
                let Ok(body) = arrived.try_recv() else { break };
                batch.push(HostCommand::new(body));
            }
            coalesce_pointer_moves(&mut batch);
            if batch.iter().any(HostCommand::native) {
                // The platform is called from a blocking thread, once for
                // the burst.
                let shared = shared.clone();
                let desktop = desktop.clone();
                tokio::task::spawn_blocking(move || apply_host_batch(&shared, &desktop, batch))
                    .await
                    .map_err(|_| Error::NoSession)??;
            } else {
                apply_host_batch(shared, desktop, batch)?;
            }
        }
        Ok(())
    };
    tokio::try_join!(reader, applier).map(|_| ())
}

pub(in crate::core) fn apply_host_batch(
    shared: &Shared,
    desktop: &DesktopRuntime,
    batch: Vec<HostCommand>,
) -> Result<(), Error> {
    for HostCommand { body, command } in batch {
        process_host_message(shared, desktop, &body, command)?;
    }
    Ok(())
}

pub(super) fn wait_input_cleanup(shared: &Shared) {
    while shared
        .input_cleanup
        .load(std::sync::atomic::Ordering::Acquire)
        != 0
    {
        drop(lock(&shared.desktop));
        std::thread::yield_now();
    }
}

/// What admission reads of `state` and `desktop` for input sent under
/// `sent_under`.
pub(super) fn input_facts(
    state: &State,
    desktop: &DesktopRuntime,
    sent_under: u64,
) -> Result<Facts, Error> {
    let peer = parse_endpoint_id(&desktop.info.service.peer_id)?;
    Ok(Facts {
        permitted: state.trust.service_granted(&peer, ServiceKind::Desktop)
            && state.trust.granted(&peer, crate::session::Grant::Control),
        holder: match state.desktop_control {
            Some(holder) if holder == desktop.stamp => Holder::This,
            Some(_) => Holder::Another,
            None => Holder::Nobody,
        },
        sent_under,
        lease: *desktop.control_state.borrow(),
        retired: desktop.retired.load(std::sync::atomic::Ordering::Acquire),
    })
}

/// Whether `lease` is still `desktop`'s.
pub(super) fn holds(desktop: &DesktopRuntime, lease: Lease) -> bool {
    lease.holds(
        *desktop.control_state.borrow(),
        desktop.retired.load(std::sync::atomic::Ordering::Acquire),
    )
}

/// The adapter, taken for one native call. Waiting for it and for old
/// cleanup happens on a blocking worker, never while holding Core state:
/// state transactions may inspect held input and must remain responsive.
pub(super) struct NativeInputGuard<'a> {
    adapter: std::sync::MutexGuard<'a, Option<Box<dyn crate::HostDesktop>>>,
    _call: InFlight<'a>,
}

impl std::ops::Deref for NativeInputGuard<'_> {
    type Target = Option<Box<dyn crate::HostDesktop>>;
    fn deref(&self) -> &Self::Target {
        &self.adapter
    }
}

impl std::ops::DerefMut for NativeInputGuard<'_> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.adapter
    }
}

/// Takes the adapter once old cleanup has finished, readmitting the input
/// sent under `epoch` (and rechecking a `pointer` against the topology)
/// while holding the adapter and Core state together.
pub(super) fn native_input_adapter<'a>(
    shared: &'a Shared,
    desktop: &'a DesktopRuntime,
    epoch: u64,
    pointer: Option<crate::InputEvent>,
) -> Result<Result<(NativeInputGuard<'a>, Lease), Refusal>, Error> {
    loop {
        #[cfg(test)]
        if std::thread::current().name() == Some("v9-pointer-probe") {
            v9_pointer_wait_probe::WAITING.store(true, std::sync::atomic::Ordering::Release);
        }
        let adapter = lock(&shared.desktop);
        let state = match shared.state.try_lock() {
            Ok(state) => state,
            Err(std::sync::TryLockError::Poisoned(error)) => error.into_inner(),
            Err(std::sync::TryLockError::WouldBlock) => {
                drop(adapter);
                drop(lock(&shared.state));
                continue;
            }
        };
        runtime(
            &state,
            desktop.info.service.id,
            desktop.info.service.generation,
            true,
        )?;
        let lease = match super::host_input::readmit(input_facts(&state, desktop, epoch)?) {
            Ok(lease) => lease,
            Err(reason) => return Ok(Err(reason)),
        };
        if shared
            .input_cleanup
            .load(std::sync::atomic::Ordering::Acquire)
            == 0
        {
            if let Some(event) = pointer {
                let selected = lock(&desktop.selected);
                if let Err(error) = super::topology::accept_pointer(shared, event, &selected) {
                    return Ok(Err(pointer_refusal(&error)));
                }
            }
            let call = desktop.host_input.enter();
            drop(state);
            return Ok(Ok((
                NativeInputGuard {
                    adapter,
                    _call: call,
                },
                lease,
            )));
        }
        drop(state);
        drop(adapter);
        std::thread::yield_now();
    }
}

/// A pointer the host's topology does not take, as one on a display it does
/// not show.
pub(super) fn pointer_refusal(error: &Error) -> Refusal {
    Refusal::PointerOutside(crate::input::InputFault::from_error(error).message())
}

/// One input a viewer sent on its desktop stream.
pub(super) enum HostInput {
    Event(crate::InputEvent),
    Text(String),
    Chord(String),
}

impl HostInput {
    pub(super) fn kind(&self) -> &'static str {
        match self {
            Self::Event(event) => event.kind(),
            Self::Text(_) | Self::Chord(_) => "typed text",
        }
    }
}

/// Admits one input under Core state: under the lease it was sent with,
/// a pointer on a display the host shows, text a viewer may send. Gives
/// the pointer to recheck once the backend is free, or why not.
/// Input refused here is discarded without applying or observing held
/// state; viewers stay connected and ask for control once the current
/// controller yields.
pub(super) fn admit_host_input(
    shared: &Shared,
    desktop: &DesktopRuntime,
    state: &State,
    epoch: u64,
    input: &HostInput,
) -> Result<Result<Option<crate::InputEvent>, Refusal>, Error> {
    if let Err(reason) = super::host_input::admit(input_facts(state, desktop, epoch)?) {
        return Ok(Err(reason));
    }
    Ok(match input {
        HostInput::Event(event) => {
            let selected = lock(&desktop.selected);
            match super::topology::accept_pointer(shared, *event, &selected) {
                Ok(()) => Ok(Some(*event)),
                Err(error) => Err(pointer_refusal(&error)),
            }
        }
        HostInput::Text(text) | HostInput::Chord(text) => {
            // A viewer never sends empty text; one that does has nothing typed.
            match crate::input::key_text_problem(text) {
                Some(problem) => Err(Refusal::InvalidText(problem)),
                None if text.is_empty() => Err(Refusal::InvalidText("text entry is empty")),
                None => Ok(None),
            }
        }
    })
}

/// Applies one admitted input, Core state released: the adapter is taken
/// with the lease rechecked, and input applied as that lease ended is
/// released under the same adapter guard.
pub(super) fn apply_host_input(
    shared: &Shared,
    desktop: &DesktopRuntime,
    epoch: u64,
    pointer: Option<crate::InputEvent>,
    input: &HostInput,
) -> Result<InputOutcome, Error> {
    let refused = |reason| Ok(InputOutcome::Refused { reason });
    let (mut adapter, lease) = match native_input_adapter(shared, desktop, epoch, pointer)? {
        Ok(taken) => taken,
        Err(reason) => return refused(reason),
    };
    if !holds(desktop, lease) {
        return refused(
            if desktop.retired.load(std::sync::atomic::Ordering::Acquire) {
                Refusal::Ended
            } else {
                Refusal::ControlChanged
            },
        );
    }
    let Some(adapter) = adapter.as_mut() else {
        return refused(Refusal::Unavailable("no desktop adapter is installed"));
    };
    let session = &desktop.host_input;
    let failed = |error: &Error| {
        super::keys::note_input_failure(shared, desktop.info.service.connection_id, error)
    };
    match input {
        HostInput::Event(event) => {
            if let Err(error) = adapter.apply(*event) {
                session.settle(holds(desktop, lease), adapter.as_mut(), failed);
                return refused(Refusal::of_backend(error)?);
            }
            if session.applied(*event, holds(desktop, lease), adapter.as_mut(), failed) {
                lock(&desktop.activity).input(*event, std::time::Instant::now());
                super::power::note_control_used(shared);
            }
        }
        HostInput::Text(text) | HostInput::Chord(text) => {
            let typed = match input {
                HostInput::Chord(_) => adapter.type_chord(text),
                _ => adapter.type_text(text),
            };
            let ended = session.settle(holds(desktop, lease), adapter.as_mut(), failed);
            if let Err(error) = typed {
                return refused(Refusal::of_backend(error)?);
            }
            if !ended {
                lock(&desktop.activity).touch(std::time::Instant::now());
                super::power::note_control_used(shared);
            }
        }
    }
    Ok(InputOutcome::Applied)
}

/// Validate under Core state, take the adapter without waiting, then release
/// Core state before native work. Late input is released under the same adapter
/// guard when retirement or a lease transition overtakes the native call.
#[cfg(test)]
pub(super) fn process_host_command(
    shared: &Shared,
    desktop: &DesktopRuntime,
    body: &[u8],
) -> Result<(), Error> {
    let HostCommand { command, .. } = HostCommand::new(body.to_vec());
    process_host_message(shared, desktop, body, command)
}

/// [`process_host_command`] for a message whose command is already decoded.
pub(super) fn process_host_message(
    shared: &Shared,
    desktop: &DesktopRuntime,
    body: &[u8],
    command: Option<Result<ControlMessage, Error>>,
) -> Result<(), Error> {
    let mut state = lock(&shared.state);
    runtime(
        &state,
        desktop.info.service.id,
        desktop.info.service.generation,
        true,
    )?;
    if !state.trust.service_granted(
        &parse_endpoint_id(&desktop.info.service.peer_id)?,
        ServiceKind::Desktop,
    ) {
        return Err(Error::Unauthorized);
    }
    if body.len() < 9 {
        return Err(Error::Malformed);
    }
    let epoch = u64::from_be_bytes(body[1..9].try_into().unwrap());
    let current_epoch = desktop.control_state.borrow().1;
    let input_allowed = state.trust.granted(
        &parse_endpoint_id(&desktop.info.service.peer_id)?,
        crate::session::Grant::Control,
    );
    if body.len() == 9 && body[0] == RELEASE {
        if epoch == current_epoch {
            release_desktop_input(shared, desktop);
        }
        return Ok(());
    }
    if body.len() == 9 && body[0] == ACQUIRE_CONTROL {
        if epoch != current_epoch {
            return Ok(());
        }
        if shared
            .input_cleanup
            .load(std::sync::atomic::Ordering::Acquire)
            != 0
        {
            drop(state);
            wait_input_cleanup(shared);
            return process_host_message(shared, desktop, body, command);
        }
        if !input_allowed {
            tracing::info!(
                target: crate::trace::INPUT,
                service = desktop.info.service.id,
                "control refused: the peer may not control this computer"
            );
            desktop
                .control_state
                .send_replace((DesktopControlState::Viewing, current_epoch));
            return Ok(());
        }
        let status =
            if state.desktop_control == Some(desktop.stamp) || state.input_holder().is_none() {
                DesktopControlState::Controlling
            } else {
                DesktopControlState::Contended
            };
        tracing::info!(
            target: crate::trace::INPUT,
            service = desktop.info.service.id,
            state = ?status,
            epoch = current_epoch,
            "control asked for"
        );
        let next_epoch = if status == DesktopControlState::Controlling
            && desktop.control_state.borrow().0 != status
        {
            current_epoch.checked_add(1).ok_or(Error::Busy)?
        } else {
            current_epoch
        };
        if status == DesktopControlState::Controlling {
            state.desktop_control = Some(desktop.stamp);
        }
        desktop.control_state.send_replace((status, next_epoch));
        return Ok(());
    }
    if body.len() == 9 && body[0] == YIELD_CONTROL {
        tracing::info!(
            target: crate::trace::INPUT,
            service = desktop.info.service.id,
            current = epoch == current_epoch,
            "control yielded"
        );
        if epoch == current_epoch {
            release_desktop_input(shared, desktop);
            if state.desktop_control == Some(desktop.stamp) {
                state.desktop_control = None;
            }
            drop(state);
            wait_input_cleanup(shared);
            // A concurrent reacquisition/revoke can replace this lease.
            let state = lock(&shared.state);
            if state.desktop_control != Some(desktop.stamp)
                && desktop.control_state.borrow().1 == current_epoch
            {
                desktop
                    .control_state
                    .send_replace((DesktopControlState::Viewing, current_epoch));
            }
        }
        return Ok(());
    }
    if body[0] != COMMAND {
        return Err(Error::Malformed);
    }
    let command = command.ok_or(Error::Malformed)??;
    runtime(
        &state,
        desktop.info.service.id,
        desktop.info.service.generation,
        true,
    )?;
    let input = match command {
        ControlMessage::Input { stamp, event } if stamp == desktop.stamp => {
            Some(HostInput::Event(event))
        }
        ControlMessage::Text { stamp, ref text } if stamp == desktop.stamp => {
            Some(HostInput::Text(text.0.clone()))
        }
        ControlMessage::Chord { stamp, ref text } if stamp == desktop.stamp => {
            Some(HostInput::Chord(text.0.clone()))
        }
        _ => None,
    };
    if let Some(input) = input {
        if dropped(shared, DropPoint::BeforeAdmission) {
            input_dropped(
                desktop.info.service.id,
                input.kind(),
                DropPoint::BeforeAdmission,
            );
            return Ok(());
        }
        let outcome = match admit_host_input(shared, desktop, &state, epoch, &input)? {
            Err(reason) => InputOutcome::Refused { reason },
            Ok(_) if dropped(shared, DropPoint::AfterAdmission) => {
                input_dropped(
                    desktop.info.service.id,
                    input.kind(),
                    DropPoint::AfterAdmission,
                );
                return Ok(());
            }
            Ok(pointer) => {
                drop(state);
                apply_host_input(shared, desktop, epoch, pointer, &input)?
            }
        };
        super::host_input::record(
            shared,
            &desktop.info.service,
            &desktop.host_input,
            input.kind(),
            outcome,
        );
        return Ok(());
    }
    match command {
        ControlMessage::SelectDisplays { stamp, displays } if stamp == desktop.stamp => {
            let snapshot = lock(&shared.topology).clone();
            match super::topology::apply_selection(&snapshot.displays, &displays) {
                Ok(ids) => {
                    let changed = {
                        let mut selected = lock(&desktop.selected);
                        let changed = *selected != ids;
                        *selected = ids;
                        changed
                    };
                    *lock(&desktop.display_fault) = None;
                    if changed {
                        desktop.keyframe.request();
                    }
                }
                Err(error) => super::keys::note_input_failure(
                    shared,
                    desktop.info.service.connection_id,
                    &error,
                ),
            }
        }
        ControlMessage::Keyframe { stamp } if stamp == desktop.stamp => desktop.keyframe.request(),
        ControlMessage::Received { stamp, picture } if stamp == desktop.stamp => {
            lock(&desktop.stream_flight).acknowledged(picture, std::time::Instant::now());
        }
        ControlMessage::VideoReport { stamp, report } if stamp == desktop.stamp => {
            lock(&desktop.viewer_loss).observe(&report, std::time::Instant::now());
        }
        ControlMessage::Quality { stamp, quality } if stamp == desktop.stamp => {
            let caps = lock(&shared.video_caps).clone();
            // No keyframe here: the encoder takes a new rate as it runs, and
            // a new size opens it again, which starts from a keyframe.
            *lock(&desktop.quality) = crate::session::agree_quality(
                &quality,
                (desktop.info.monitor_width, desktop.info.monitor_height),
                &caps,
            );
        }
        _ => return Err(Error::StaleSession),
    }
    Ok(())
}

pub(super) async fn write_control_state(
    send: &mut SendStream,
    status: (DesktopControlState, u64),
) -> Result<(), Error> {
    let mut bytes = vec![CONTROL_STATE, status.0.encode()];
    bytes.extend_from_slice(&status.1.to_be_bytes());
    write(send, &bytes).await
}
