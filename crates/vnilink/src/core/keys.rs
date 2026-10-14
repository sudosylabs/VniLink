//! Key, pointer, keyboard mode, and finished text on the control stream.

use super::*;

use super::link::remember_peer_addresses;
use super::wire::{read_session_frame, write_frame};

use std::sync::atomic::Ordering;
use std::sync::Arc;

use iroh::EndpointId;

use crate::address::Announcement;
use crate::error::Error;
use crate::events::CoreEvent;
use crate::input::{self, HeldInput, HostDesktop, InputEvent, InputOutbox};
use crate::keyboard::{self, KeyboardMode, LocalKey};
use crate::os_input::OsInput;
use crate::protocol::{error_code, ControlMessage, SessionStamp};
use crate::session::{self, Grant, SessionPhase};

impl Core {
    /// Installs the adapter that applies keyboard and pointer events on this host.
    ///
    /// An idle neutral connection may configure its adapter before Desktop Open.
    /// Refused while Desktop is active or releasing held input.
    pub fn set_desktop(&self, desktop: Box<dyn HostDesktop>) -> Result<(), Error> {
        let monitor = desktop.monitor();
        // The candidate lives outside the admission scope, so rejection also
        // destroys native resources only after the global State guard is gone.
        let mut candidate = Some(desktop);
        let (_gate, _adapter, old) = (|| {
            let state = self.alive_state()?;
            if state
                .incoming
                .values()
                .any(|session| !finished(&session.phase) && !session.services_only)
                || self.shared.desktop_streams.load(Ordering::Acquire) != 0
            {
                return Err(Error::Busy);
            }
            let gate = self
                .shared
                .desktop_gate
                .try_lock()
                .map_err(|_| Error::Busy)?;
            if self.shared.input_cleanup.load(Ordering::Acquire) != 0 {
                return Err(Error::Busy);
            }
            let mut adapter = self.shared.desktop.try_lock().map_err(|_| Error::Busy)?;
            let old = adapter.replace(candidate.take().unwrap());
            *lock(&self.shared.desktop_info) = Some(monitor);
            Ok((gate, adapter, old))
        })()?;
        // Keep both native fences until teardown releases the old adapter's
        // keys. Core operations can proceed, but new input cannot overtake it.
        drop(old);
        Ok(())
    }

    /// Inject authorized keyboard and pointer events into this operating system.
    ///
    /// On macOS this can prompt for Accessibility permission. It does not capture
    /// the screen. Linux support is the X11 backend; Wayland is not claimed.
    pub fn enable_os_input_without_prompt(&self) -> Result<(), Error> {
        self.set_desktop(Box::new(OsInput::open_with_prompt(false)?))
    }

    pub fn enable_os_input(&self) -> Result<(), Error> {
        self.set_desktop(Box::new(OsInput::open()?))
    }

    /// Queues one keyboard or pointer event for the active controlling session.
    ///
    /// Returns when the event is queued, not when the host has applied it.
    /// Consecutive pointer positions collapse to the latest one. A later call
    /// returns the host's rejection if input was refused.
    pub fn submit_input(
        &self,
        session_id: u64,
        generation: u64,
        event: InputEvent,
    ) -> Result<(), Error> {
        let outbox = self.live_outbox(session_id, generation)?;
        let controller = self.controller_state(session_id, generation)?;
        let _plan = lock(&controller.keyboard);
        let event = if controller.swap.load(Ordering::Relaxed) {
            input::swap_command_key(event)
        } else {
            event
        };
        let mut held = lock(&controller.held);
        let mut pointer = lock(&controller.pointer);
        let mut trial = pointer.clone();
        if let Some(event) = trial.apply(event)? {
            outbox.push(event)?;
            held.observe(event);
        }
        *pointer = trial;
        Ok(())
    }

    /// Applies application defaults to all controller owners and future connections.
    /// Validation and held-input checks precede every mutation.
    pub fn set_input_defaults(
        &self,
        mode: KeyboardMode,
        swap: bool,
        pointer: crate::PointerSettings,
    ) -> Result<(), Error> {
        pointer.validate()?;
        let state = self.alive_state()?;
        let controllers: Vec<_> = state
            .outgoing
            .values()
            .filter(|s| !finished(&s.phase))
            .filter_map(|s| s.controller.clone())
            .collect();
        let desktops: Vec<_> = state
            .services
            .values()
            .filter_map(|s| s.desktop.clone())
            .collect();
        let plans: Vec<_> = controllers.iter().map(|c| lock(&c.keyboard)).collect();
        let held: Vec<_> = controllers.iter().map(|c| lock(&c.held)).collect();
        let mut desktop_plans: Vec<_> = desktops.iter().map(|d| lock(&d.keyboard)).collect();
        if plans.iter().any(|p| p.is_held())
            || held.iter().any(|h| !h.release_events().is_empty())
            || desktop_plans.iter().any(|p| !p.idle())
        {
            return Err(Error::InvalidSetting(
                "release held keys and buttons before changing input defaults",
            ));
        }
        for controller in &controllers {
            controller.mode.store(mode as u8, Ordering::Relaxed);
            controller.swap.store(swap, Ordering::Relaxed);
            *lock(&controller.pointer) = crate::input::PointerPlan::with_settings(pointer);
        }
        for keyboard in &mut desktop_plans {
            keyboard.mode = mode;
            keyboard.swap = swap;
            keyboard.pointer = crate::input::PointerPlan::with_settings(pointer);
        }
        self.shared
            .keyboard_mode
            .store(mode as u8, Ordering::Relaxed);
        self.shared
            .swap_control_command
            .store(swap, Ordering::Relaxed);
        *lock(&self.shared.pointer_defaults) = pointer;
        Ok(())
    }

    /// Pointer policy for one controlling connection; never changes another tab.
    pub fn set_session_pointer_settings(
        &self,
        id: u64,
        generation: u64,
        settings: crate::PointerSettings,
    ) -> Result<(), Error> {
        let controller = self.controller_state(id, generation)?;
        let _plan = lock(&controller.keyboard);
        let held = lock(&controller.held);
        let result = lock(&controller.pointer).set(settings, held.buttons_held());
        result
    }
    pub fn session_pointer_settings(
        &self,
        id: u64,
        generation: u64,
    ) -> Result<crate::PointerSettings, Error> {
        let controller = self.controller_state(id, generation)?;
        let settings = lock(&controller.pointer).settings;
        Ok(settings)
    }

    /// Choose the one display this legacy desktop stream shows and controls.
    ///
    /// The id must be in the host topology. A later removal clears the
    /// selection instead of substituting another display.
    pub fn select_displays(
        &self,
        session_id: u64,
        generation: u64,
        ids: &[u64],
    ) -> Result<(), Error> {
        let ids = super::topology::normalize_selection(ids)?;
        let outbox = self.live_outbox(session_id, generation)?;
        outbox.push_displays(ids)
    }

    /// Exchanges Control and Command on events this controller sends.
    ///
    /// Off by default. A Mac Command shortcut can be sent as Control for a
    /// Windows or Linux host, and the reverse. The host injects the physical
    /// key it receives. The choice belongs to this process and is not saved.
    /// Release held modifiers before changing it, so the key that goes down
    /// is the key that comes up.
    pub fn set_swap_control_and_command(&self, enabled: bool) -> Result<(), Error> {
        let state = self.alive_state()?;
        if state.outgoing.values().any(|s| {
            !finished(&s.phase)
                && s.controller.as_ref().is_some_and(|c| {
                    lock(&c.keyboard).is_held() || !lock(&c.held).release_events().is_empty()
                })
        }) {
            return Err(Error::InvalidSetting(
                "release held keys before changing the Control/Command swap",
            ));
        }
        for session in state.outgoing.values() {
            if let Some(controller) = &session.controller {
                controller.swap.store(enabled, Ordering::Relaxed);
            }
        }
        self.shared
            .swap_control_command
            .store(enabled, Ordering::Relaxed);
        Ok(())
    }

    /// Whether this controller exchanges Control and Command.
    pub fn swap_control_and_command(&self) -> Result<bool, Error> {
        drop(self.alive_state()?);
        Ok(self.shared.swap_control_command.load(Ordering::Relaxed))
    }

    /// Chooses how a local key becomes host input.
    ///
    /// [`KeyboardMode::Character`] is the default. The character the controller
    /// operating system produced is what the host inserts. [`KeyboardMode::Map`]
    /// sends the physical key. [`KeyboardMode::Translate`] asks the host to
    /// press the chord in its own layout. The choice belongs to this process
    /// and is not saved. Release held keys before changing it.
    pub fn set_keyboard_mode(&self, mode: KeyboardMode) -> Result<(), Error> {
        let state = self.alive_state()?;
        if state.outgoing.values().any(|s| {
            !finished(&s.phase)
                && s.controller.as_ref().is_some_and(|c| {
                    lock(&c.keyboard).is_held() || !lock(&c.held).release_events().is_empty()
                })
        }) {
            return Err(Error::InvalidSetting(
                "release held keys before changing the keyboard mode",
            ));
        }
        for session in state.outgoing.values() {
            if let Some(controller) = &session.controller {
                controller.mode.store(mode as u8, Ordering::Relaxed);
            }
        }
        self.shared
            .keyboard_mode
            .store(mode as u8, Ordering::Relaxed);
        Ok(())
    }

    /// How this controller turns a local key into host input.
    pub fn keyboard_mode(&self) -> Result<KeyboardMode, Error> {
        drop(self.alive_state()?);
        Ok(KeyboardMode::from_u8(
            self.shared.keyboard_mode.load(Ordering::Relaxed),
        ))
    }

    /// Queues one local key for the controlled computer.
    ///
    /// The mode from [`Core::set_keyboard_mode`] decides whether the host
    /// inserts `key.text`, receives the physical key, or is asked to press
    /// its own chord for that text. Returns when the result is queued, not
    /// when the host has applied it. The text is not logged. A dropped session
    /// does not send it again.
    pub fn submit_local_key(
        &self,
        session_id: u64,
        generation: u64,
        key: LocalKey<'_>,
    ) -> Result<(), Error> {
        let outbox = self.live_outbox(session_id, generation)?;
        let controller = self.controller_state(session_id, generation)?;
        let mut plan = lock(&controller.keyboard);
        let mode = KeyboardMode::from_u8(controller.mode.load(Ordering::Relaxed));
        let swap = controller.swap.load(Ordering::Relaxed);
        plan.reset_if_generation(generation);
        let mut trial = plan.clone();
        let planned = trial.apply(mode, swap, key)?;
        let items = planned
            .into_iter()
            .map(keyboard::Planned::outbound)
            .collect::<Vec<_>>();
        let mut held = lock(&controller.held);
        outbox.push_batch(&items)?;
        for item in &items {
            if let crate::input::Outbound::Event(event) = item {
                held.observe(*event);
            }
        }
        *plan = trial;
        Ok(())
    }

    /// Releases this tab's held keys/buttons without disconnecting it.
    pub fn release_session_input(&self, id: u64, generation: u64) -> Result<(), Error> {
        let outbox = self.live_outbox(id, generation)?;
        let controller = self.controller_state(id, generation)?;
        let mut plan = lock(&controller.keyboard);
        let mut held = lock(&controller.held);
        let releases: Vec<_> = held
            .release_events()
            .into_iter()
            .map(crate::input::Outbound::Event)
            .collect();
        outbox.push_batch(&releases)?;
        *held = HeldInput::default();
        lock(&controller.pointer).reset();
        plan.clear();
        Ok(())
    }

    pub fn session_keyboard_mode(&self, id: u64, generation: u64) -> Result<KeyboardMode, Error> {
        Ok(KeyboardMode::from_u8(
            self.controller_state(id, generation)?
                .mode
                .load(Ordering::Relaxed),
        ))
    }

    pub fn session_swap_control_and_command(
        &self,
        id: u64,
        generation: u64,
    ) -> Result<bool, Error> {
        Ok(self
            .controller_state(id, generation)?
            .swap
            .load(Ordering::Relaxed))
    }

    pub fn set_session_keyboard_mode(
        &self,
        id: u64,
        generation: u64,
        mode: KeyboardMode,
    ) -> Result<(), Error> {
        let controller = self.controller_state(id, generation)?;
        let plan = lock(&controller.keyboard);
        if plan.is_held() || !lock(&controller.held).release_events().is_empty() {
            return Err(Error::InvalidSetting(
                "release held keys before changing the keyboard mode",
            ));
        }
        controller.mode.store(mode as u8, Ordering::Relaxed);
        Ok(())
    }

    pub fn set_session_swap_control_and_command(
        &self,
        id: u64,
        generation: u64,
        enabled: bool,
    ) -> Result<(), Error> {
        let controller = self.controller_state(id, generation)?;
        let plan = lock(&controller.keyboard);
        if plan.is_held() || !lock(&controller.held).release_events().is_empty() {
            return Err(Error::InvalidSetting(
                "release held keys before changing the Control/Command swap",
            ));
        }
        controller.swap.store(enabled, Ordering::Relaxed);
        Ok(())
    }

    /// Queues finished text for the controlled computer.
    ///
    /// This is not a clipboard paste and does not use the clipboard grant.
    /// The host types the characters, so the result does not depend on which
    /// physical key was pressed. The text is UTF-8, at most
    /// [`MAX_KEY_TEXT`](crate::MAX_KEY_TEXT) bytes, and is not logged.
    /// Returns when the text is queued, not when the host has typed it.
    /// A dropped session does not send it again.
    pub fn submit_text(&self, session_id: u64, generation: u64, text: &str) -> Result<(), Error> {
        input::check_key_text(text)?;
        let outbox = self.live_outbox(session_id, generation)?;
        outbox.push_text(text.to_owned())
    }

    /// The queue of the active controlling session. Planning, mode, and swap
    /// all read the same liveness check.
    fn live_outbox(&self, session_id: u64, generation: u64) -> Result<Arc<InputOutbox>, Error> {
        let state = self.alive_state()?;
        let session = state.outgoing.get(&session_id).ok_or(Error::NoSession)?;
        if session.phase != SessionPhase::Active || session.id != session_id {
            return Err(Error::NoSession);
        }
        if session.generation != generation {
            return Err(Error::StaleSession);
        }
        if session.files_only || *session.desktop_stop.borrow() {
            return Err(Error::Unsupported("desktop input on a service connection"));
        }
        session.outbox.clone().ok_or(Error::NoSession)
    }
}

/// A control message belongs to the live session only while its generation is
/// current, the session is active, and the owner still grants control.
/// Clipboard and transfer re-check their grants per message; control does the
/// same here, so `authorize` during the session takes effect on the next
/// message and held keys release when the loop ends.
fn check_input_allowed(state: &State, stamp: SessionStamp) -> Result<(), Error> {
    let active = state
        .incoming
        .values()
        .filter(|session| {
            session.phase == SessionPhase::Active
                && !session.files_only
                && !*session.desktop_stop.borrow()
                && state.trust.granted(&session.peer_key, Grant::Control)
        })
        .map(|session| SessionStamp::new(session.id, session.generation))
        .find(|active| session::check_input(Some(*active), stamp).is_ok());
    session::check_input(active, stamp)
}

pub(super) async fn host_input_loop(
    shared: &Arc<Shared>,
    session_id: u64,
    peer: EndpointId,
    send: &mut iroh::endpoint::SendStream,
    recv: &mut iroh::endpoint::RecvStream,
    stop_rx: &mut tokio::sync::watch::Receiver<bool>,
) -> bool {
    let held = lock(&shared.state)
        .incoming
        .get(&session_id)
        .unwrap()
        .host_held
        .clone();
    let mut input_open = true;
    let mut desktop_stop = lock(&shared.state)
        .incoming
        .get(&session_id)
        .expect("admitted session")
        .desktop_stop
        .subscribe();
    let mut retired = false;
    let mut topology_rx = shared.topology_watch.subscribe();
    let stamp = lock(&shared.state)
        .incoming
        .get(&session_id)
        .map(|session| session.wire_stamp);
    if let Some(stamp) = stamp {
        if super::topology::write_topology(shared, send, stamp)
            .await
            .is_err()
        {
            schedule_legacy_release(shared, session_id, &held);
            return false;
        }
    }
    let local_stop = loop {
        if *stop_rx.borrow() || lock(&shared.state).shut_down {
            break true;
        }
        tokio::select! {
            biased;
            _ = desktop_stop.changed(), if !retired => {
                retired = true;
                schedule_legacy_release(shared, session_id, &held);
                input_open = false;
            }
            changed = stop_rx.changed() => {
                if changed.is_err() || *stop_rx.borrow() {
                    break true;
                }
            }
            changed = topology_rx.changed() => {
                if changed.is_err() {
                    break false;
                }
                let stamp = lock(&shared.state)
                    .incoming
                    .get(&session_id)
                    .map(|session| session.wire_stamp);
                if let Some(stamp) = stamp {
                    if super::topology::write_topology(shared, send, stamp)
                        .await
                        .is_err()
                    {
                        break false;
                    }
                }
            }
            frame = read_session_frame(recv) => {
                let message = match frame {
                    Ok(message) => message,
                    Err(Error::Malformed) => {
                        let _ = write_frame(send, &ControlMessage::Reject(error_code(&Error::Malformed))).await;
                        break false;
                    }
                    Err(_) => break false,
                };
                match message {
                    ControlMessage::Addresses { relays, direct } => {
                        let _ = remember_peer_addresses(
                            shared,
                            peer.as_bytes(),
                            Announcement { relays, direct },
                        );
                    }
                    ControlMessage::Close => break false,
                    ControlMessage::Keyframe { stamp } => {
                        match check_authorized(shared, send, stamp, false).await {
                            Some(true) => {},
                            None => continue,
                            Some(false) => break false,
                        }
                        shared
                            .events
                            .publish(CoreEvent::KeyframeRequested { id: session_id });
                    }
                    ControlMessage::Quality { stamp, quality } => {
                        match check_authorized(shared, send, stamp, false).await {
                            Some(true) => {},
                            None => continue,
                            Some(false) => break false,
                        }
                        let mut state = lock(&shared.state);
                        if let Some(session) = state.incoming.get_mut(&session_id).filter(|s| s.phase == SessionPhase::Active && !*s.desktop_stop.borrow()) {
                            session.quality = Some(quality);
                            shared.events.publish(CoreEvent::QualityUpdated { id: session_id, quality });
                        }
                    }
                    ControlMessage::Input { stamp, event } => {
                        match check_authorized(shared, send, stamp, true).await {
                            Some(true) => {},
                            None => continue,
                            Some(false) => break false,
                        }
                        apply_host_input(shared, session_id, stamp, send, &mut input_open,
                            LegacyInput::Event(event)).await;
                    }
                    ControlMessage::SelectDisplays { stamp, displays } => {
                        match check_authorized(shared, send, stamp, false).await {
                            Some(true) => {},
                            None => continue,
                            Some(false) => break false,
                        }
                        let snapshot = lock(&shared.topology).clone();
                        match super::topology::apply_selection(&snapshot.displays, &displays) {
                            Ok(ids) => {
                                let mut state = lock(&shared.state);
                                if let Some(session) = state.incoming.get_mut(&session_id) {
                                    if session.wire_stamp == stamp && !*session.desktop_stop.borrow()
                                    {
                                        session.selected_displays = ids;
                                        session.display_fault = None;
                                    }
                                }
                            }
                            Err(error) => {
                                note_input_failure(shared, session_id, &error);
                            }
                        }
                    }
                    ControlMessage::Text { stamp, text } => {
                        match check_authorized(shared, send, stamp, true).await {
                            Some(true) => {},
                            None => continue,
                            Some(false) => break false,
                        }
                        apply_host_input(shared, session_id, stamp, send, &mut input_open,
                            LegacyInput::Text(text.0, false)).await;
                    }
                    ControlMessage::Chord { stamp, text } => {
                        match check_authorized(shared, send, stamp, true).await {
                            Some(true) => {},
                            None => continue,
                            Some(false) => break false,
                        }
                        apply_host_input(shared, session_id, stamp, send, &mut input_open,
                            LegacyInput::Text(text.0, true)).await;
                    }
                    _ => {
                        let _ = write_frame(send, &ControlMessage::Reject(error_code(&Error::Malformed))).await;
                        break false;
                    }
                }
            }
        }
    };
    schedule_legacy_release(shared, session_id, &held);
    local_stop
}

pub(super) fn note_input_failure(shared: &Shared, session_id: u64, error: &Error) {
    shared.events.publish(CoreEvent::InputFailed {
        id: session_id,
        message: input::InputFault::from_error(error).message(),
    });
}

/// Generation-check one control message. Every keyframe, input, text, and
/// chord frame passes here before it is applied. None discards late desktop
/// messages after retirement; Some(false) ends the connection for a bad stamp.
async fn check_authorized(
    shared: &Shared,
    send: &mut iroh::endpoint::SendStream,
    stamp: SessionStamp,
    contended_input: bool,
) -> Option<bool> {
    let decision = {
        let state = lock(&shared.state);
        let Some(owner) = state
            .incoming
            .values()
            .find(|session| session.wire_stamp == stamp)
        else {
            return Some(false);
        };
        if contended_input && !state.trust.granted(&owner.peer_key, Grant::Control) {
            return None;
        }
        // Contended input from a session without the lease is discarded, like
        // late messages after retirement: the viewer stays connected and its
        // later input succeeds once the lease passes. A Reject would fault
        // the sender's outbox permanently, so it is never used for contention.
        // Display selection, quality, and keyframe requests are per viewer
        // and stay applied when another session holds the input lease.
        if contended_input
            && state.input_holder().is_some_and(|holder| {
                state
                    .incoming
                    .values()
                    .any(|s| s.wire_stamp == stamp && s.id != holder)
            })
        {
            return None;
        }
        if state
            .incoming
            .values()
            .any(|s| s.wire_stamp == stamp && *s.desktop_stop.borrow())
        {
            return None;
        }
        let active = state
            .incoming
            .values()
            .filter(|session| {
                session.phase == SessionPhase::Active
                    && !session.files_only
                    && !*session.desktop_stop.borrow()
                    && state.trust.granted(&session.peer_key, Grant::View)
            })
            .map(|session| session.wire_stamp)
            .find(|active| session::check_input(Some(*active), stamp).is_ok());
        session::check_input(active, stamp)
    };
    if let Err(error) = decision {
        let _ = write_frame(send, &ControlMessage::Reject(error_code(&error))).await;
        return Some(false);
    }
    Some(true)
}

/// Owned input commands cross the blocking-worker boundary; none retain state
/// or borrow the authenticated receiver while native input is parked.
enum LegacyInput {
    Event(InputEvent),
    Text(String, bool),
}

async fn apply_host_input(
    shared: &Arc<Shared>,
    session_id: u64,
    stamp: SessionStamp,
    send: &mut iroh::endpoint::SendStream,
    input_open: &mut bool,
    command: LegacyInput,
) -> bool {
    if !*input_open {
        return false;
    }
    let (epoch, mut stop, mut desktop_stop, slot, link) = {
        let state = lock(&shared.state);
        let Some(session) = state.incoming.get(&session_id) else {
            return false;
        };
        (
            session.input_epoch.load(Ordering::Acquire),
            session.stop.subscribe(),
            session.desktop_stop.subscribe(),
            session.incoming_slot.clone(),
            session.link.clone(),
        )
    };
    let worker = shared.clone();
    let worker = tokio::task::spawn_blocking(move || {
        let _slot = slot;
        apply_legacy_input(&worker, session_id, stamp, epoch, command)
    });
    // Dropping the async join wait leaves the bounded native worker alive.
    // Its slot and late-release fence remain until actual completion.
    let result = tokio::select! {
        biased;
        _ = wait_native_stop(&mut stop) => return false,
        _ = wait_native_stop(&mut desktop_stop) => return false,
        _ = async move {
            if let Some(link) = link { link.closed().await; }
            else { std::future::pending::<()>().await; }
        } => return false,
        result = worker => result.unwrap_or(Err(Error::NoSession)),
    };
    match result {
        Ok(()) => true,
        Err(error) => {
            note_input_failure(shared, session_id, &error);
            if matches!(error, Error::Unsupported(_)) {
                *input_open = false;
                let _ = write_frame(send, &ControlMessage::Reject(error_code(&error))).await;
            }
            false
        }
    }
}

async fn wait_native_stop(stop: &mut tokio::sync::watch::Receiver<bool>) {
    while !*stop.borrow() {
        if stop.changed().await.is_err() {
            break;
        }
    }
}

fn apply_legacy_input(
    shared: &Shared,
    id: u64,
    stamp: SessionStamp,
    epoch: u64,
    command: LegacyInput,
) -> Result<(), Error> {
    // Capture the lease before waiting. A revoke/regrant while queued cannot
    // turn an old command into input for a newly admitted lease.
    let epoch_watch = {
        let state = lock(&shared.state);
        let Some(session) = state.incoming.get(&id) else {
            return Ok(());
        };
        session.input_epoch.clone()
    };
    loop {
        let mut adapter = lock(&shared.desktop);
        let state = match shared.state.try_lock() {
            Ok(state) => state,
            Err(std::sync::TryLockError::Poisoned(error)) => error.into_inner(),
            Err(std::sync::TryLockError::WouldBlock) => {
                drop(adapter);
                drop(lock(&shared.state));
                continue;
            }
        };
        let Some(session) = state.incoming.get(&id) else {
            return Ok(());
        };
        if state.shut_down
            || session.phase != SessionPhase::Active
            || session.wire_stamp != stamp
            || *session.stop.borrow()
            || *session.desktop_stop.borrow()
            || epoch_watch.load(Ordering::Acquire) != epoch
            || !state.trust.granted(&session.peer_key, Grant::View)
            || !state.trust.granted(&session.peer_key, Grant::Control)
        {
            return Ok(());
        }
        if shared.input_cleanup.load(Ordering::Acquire) != 0 {
            drop(state);
            drop(adapter);
            std::thread::yield_now();
            continue;
        }
        if state.input_holder() != Some(id) {
            return Ok(());
        }
        if let LegacyInput::Event(event) = &command {
            super::topology::accept_pointer(shared, *event, &session.selected_displays)?;
        }
        let held = session.host_held.clone();
        let stop = session.stop.subscribe();
        let desktop_stop = session.desktop_stop.subscribe();
        drop(state);
        let adapter = adapter
            .as_mut()
            .ok_or(Error::Unsupported("no desktop adapter is installed"))?;
        let result = match &command {
            LegacyInput::Event(event) => adapter.apply(*event),
            LegacyInput::Text(text, chord) => {
                if let Some(problem) = input::key_text_problem(text) {
                    Err(Error::InputFailed(problem))
                } else if text.is_empty() {
                    Ok(())
                } else if *chord {
                    adapter.type_chord(text)
                } else {
                    adapter.type_text(text)
                }
            }
        };
        let late_releases = {
            let mut held = lock(&held);
            if result.is_ok() {
                if let LegacyInput::Event(event) = command {
                    held.observe(event);
                }
            }
            // Only bookkeeping holds the held-set mutex. A native release
            // may park too, so state transactions must never wait on that mutex
            // while a worker performs release OS calls.
            if *stop.borrow()
                || *desktop_stop.borrow()
                || epoch_watch.load(Ordering::Acquire) != epoch
            {
                Some(std::mem::take(&mut *held))
            } else {
                None
            }
        };
        if let Some(held) = late_releases {
            for event in held.release_events() {
                if adapter.apply(event).is_err() {
                    note_input_failure(
                        shared,
                        id,
                        &Error::InputFailed("a held key or button could not be released"),
                    );
                }
            }
        }
        return result;
    }
}

fn schedule_legacy_release(shared: &Shared, id: u64, held: &Arc<Mutex<HeldInput>>) {
    // Reserve before taking the snapshot. In-flight work observes the epoch
    // change and releases any later key-down itself.
    let state = lock(&shared.state);
    if let Some(session) = state.incoming.get(&id) {
        session.input_epoch.fetch_add(1, Ordering::AcqRel);
    }
    let slot = state
        .incoming
        .get(&id)
        .and_then(|session| session.incoming_slot.clone());
    let held = std::mem::take(&mut *lock(held));
    if held.release_events().is_empty() {
        return;
    }
    shared.input_cleanup.fetch_add(1, Ordering::AcqRel);
    let adapter = shared.desktop.clone();
    let pending = shared.input_cleanup.clone();
    let events = shared.events.clone();
    drop(state);
    let reservation = super::input_cleanup::Reservation {
        pending,
        releasing: None,
    };
    shared.cleanup_executor.submit(move || {
        let _slot = slot;
        let mut adapter = lock(&adapter);
        if let Some(adapter) = adapter.as_mut() {
            for event in held.release_events() {
                if adapter.apply(event).is_err() {
                    events.publish(CoreEvent::InputFailed {
                        id,
                        message: "a held key or button could not be released",
                    });
                }
            }
        }
        drop(reservation);
    });
}

#[cfg(test)]
mod review_v9_legacy_input {
    use super::*;
    use std::sync::atomic::AtomicBool;
    use std::time::{Duration, Instant};

    struct Parked {
        once: AtomicBool,
        entered: std::sync::mpsc::SyncSender<()>,
        proceed: Mutex<std::sync::mpsc::Receiver<()>>,
        events: Arc<Mutex<Vec<InputEvent>>>,
        completed: Arc<AtomicBool>,
    }
    impl Parked {
        fn park(&self) {
            if self.once.swap(false, Ordering::SeqCst) {
                self.entered.send(()).unwrap();
                self.proceed
                    .lock()
                    .unwrap()
                    .recv_timeout(Duration::from_secs(5))
                    .unwrap();
                self.completed.store(true, Ordering::Release);
            }
        }
    }
    impl HostDesktop for Parked {
        fn monitor(&self) -> (u16, u16) {
            (64, 64)
        }
        fn apply(&mut self, event: InputEvent) -> Result<(), Error> {
            self.park();
            lock(&self.events).push(event);
            Ok(())
        }
        fn type_text(&mut self, _: &str) -> Result<(), Error> {
            self.park();
            Ok(())
        }
        fn type_chord(&mut self, _: &str) -> Result<(), Error> {
            self.park();
            Ok(())
        }
    }
    #[test]
    fn review_v9_legacy_input_keeps_core_responsive_and_settles_late_keys() {
        run(0);
    }
    #[test]
    fn review_v9_legacy_text_keeps_core_responsive() {
        run(1);
    }
    #[test]
    fn review_v9_legacy_chord_keeps_core_responsive() {
        run(2);
    }
    #[test]
    fn review_v9_legacy_regrant_does_not_adopt_late_input() {
        run(3);
    }
    fn run(mode: u8) {
        let text = mode == 1 || mode == 2;
        let host = crate::tests::support::start_direct();
        let peer = crate::tests::support::start_direct();
        let sibling = crate::tests::support::start_direct();
        let (entered, ready) = std::sync::mpsc::sync_channel(1);
        let (proceed, wait) = std::sync::mpsc::channel();
        let events = Arc::new(Mutex::new(Vec::new()));
        let completed = Arc::new(AtomicBool::new(false));
        host.set_desktop(Box::new(Parked {
            once: AtomicBool::new(true),
            entered,
            proceed: Mutex::new(wait),
            events: events.clone(),
            completed: completed.clone(),
        }))
        .unwrap();
        crate::tests::support::pair_for_control(&host, &peer);
        let parent = peer.connect(&host.endpoint_id()).unwrap();
        if mode == 2 {
            peer.live_outbox(parent.id, parent.generation)
                .unwrap()
                .push_batch(&[crate::input::Outbound::Chord("a".into())])
                .unwrap();
        } else if text {
            peer.submit_text(parent.id, parent.generation, "hello")
                .unwrap();
        } else {
            peer.submit_input(
                parent.id,
                parent.generation,
                InputEvent::key_down(input::usage::A),
            )
            .unwrap();
        }
        ready
            .recv_timeout(Duration::from_secs(5))
            .expect("legacy command did not reach backend");
        let until = Instant::now() + Duration::from_millis(100);
        let mut available = false;
        while Instant::now() < until {
            if host.shared.state.try_lock().is_ok() {
                available = true;
                break;
            }
            std::thread::yield_now();
        }
        let mut sibling_desktop = None;
        // Do not turn a red responsiveness result into an unbounded test.
        if available {
            host.snapshot().unwrap();
            host.capability_availability(None, crate::Capability::DesktopControl)
                .unwrap();
            host.capture_displays().unwrap();
            crate::tests::support::pair_for_control(&host, &sibling);
            let sibling_parent = sibling.begin_connect_services(&host.endpoint_id()).unwrap();
            let until = Instant::now() + Duration::from_secs(3);
            while sibling.controller_session(sibling_parent.id).unwrap().phase
                != SessionPhase::Active
            {
                assert!(Instant::now() < until, "sibling parent admission stalled");
                std::thread::yield_now();
            }
            let sibling_parent = sibling.controller_session(sibling_parent.id).unwrap();
            let owner = sibling
                .attach_controller_service(
                    sibling_parent.id,
                    sibling_parent.generation,
                    crate::ServiceKind::Desktop,
                )
                .unwrap();
            sibling_desktop = Some(
                sibling
                    .open_desktop_service(owner.id, owner.generation)
                    .unwrap(),
            );
            host.authorize(
                &peer.endpoint_id(),
                crate::Grants {
                    view: true,
                    control: false,
                    clipboard: false,
                    transfer: false,
                },
            )
            .unwrap();
            if mode == 3 {
                host.authorize(
                    &peer.endpoint_id(),
                    crate::Grants {
                        view: true,
                        control: true,
                        clipboard: false,
                        transfer: false,
                    },
                )
                .unwrap();
            } else {
                let legacy_id = lock(&host.shared.state)
                    .incoming
                    .values()
                    .find(|session| {
                        session.peer_key == parse_endpoint_id(&peer.endpoint_id()).unwrap()
                    })
                    .unwrap()
                    .id;
                host.stop_hosted_peer(legacy_id).unwrap();
            }
        }
        proceed.send(()).unwrap();
        let until = Instant::now() + Duration::from_secs(5);
        if !text && available {
            while lock(&events).len() < 2 && Instant::now() < until {
                std::thread::yield_now();
            }
        }
        let applied = lock(&events).clone();
        drop(sibling_desktop);
        sibling.shutdown();
        peer.shutdown();
        host.shutdown();
        assert!(available, "legacy native work held Core state");
        assert!(
            completed.load(Ordering::Acquire),
            "parked native command did not finish normally"
        );
        if !text {
            assert_eq!(
                applied,
                vec![
                    InputEvent::key_down(input::usage::A),
                    InputEvent::key_up(input::usage::A)
                ]
            );
        }
    }
}

#[cfg(test)]
mod v11_legacy_cancel_probe {
    use super::*;
    struct Parked {
        applied: Arc<Mutex<Vec<InputEvent>>>,
        once: std::sync::atomic::AtomicBool,
        entered: std::sync::mpsc::SyncSender<()>,
        proceed: Mutex<std::sync::mpsc::Receiver<()>>,
    }
    impl HostDesktop for Parked {
        fn monitor(&self) -> (u16, u16) {
            (640, 480)
        }
        fn apply(&mut self, event: InputEvent) -> Result<(), Error> {
            if self.once.swap(false, Ordering::SeqCst) {
                self.entered.send(()).unwrap();
                lock(&self.proceed)
                    .recv_timeout(std::time::Duration::from_secs(30))
                    .unwrap();
            }
            lock(&self.applied).push(event);
            Ok(())
        }
    }
    #[test]
    fn v11_parent_stop_must_settle_while_native_ack_is_parked() {
        use crate::tests::support::{pair_for_control, start_direct};
        use std::time::{Duration, Instant};
        let host = start_direct();
        let peer = start_direct();
        let (entered, ready) = std::sync::mpsc::sync_channel(1);
        let (proceed, wait) = std::sync::mpsc::channel();
        let applied = Arc::new(Mutex::new(Vec::new()));
        host.set_desktop(Box::new(Parked {
            applied: applied.clone(),
            once: std::sync::atomic::AtomicBool::new(true),
            entered,
            proceed: Mutex::new(wait),
        }))
        .unwrap();
        pair_for_control(&host, &peer);
        let parent = peer.connect(&host.endpoint_id()).unwrap();
        peer.submit_input(
            parent.id,
            parent.generation,
            InputEvent::key_down(input::usage::A),
        )
        .unwrap();
        ready.recv_timeout(Duration::from_secs(5)).unwrap();
        let id = lock(&host.shared.state)
            .incoming
            .values()
            .find(|s| s.peer_key == parse_endpoint_id(&peer.endpoint_id()).unwrap())
            .unwrap()
            .id;
        host.stop_hosted_peer(id).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut settled = false;
        while Instant::now() < deadline {
            settled = lock(&host.shared.state)
                .incoming
                .get(&id)
                .is_none_or(|s| finished(&s.phase));
            if settled {
                break;
            }
            std::thread::yield_now();
        }
        assert!(
            settled,
            "parent must close independently of native completion"
        );
        assert_eq!(
            lock(&host.shared.state)
                .incoming_slots
                .load(Ordering::Acquire),
            1
        );
        let deadline = Instant::now() + Duration::from_secs(5);
        while !finished(&peer.controller_session(parent.id).unwrap().phase) {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(2));
        }
        let mut replacements = Vec::new();
        for _ in 0..3 {
            let replacement = start_direct();
            pair_for_control(&host, &replacement);
            replacement.connect(&host.endpoint_id()).unwrap();
            replacements.push(replacement);
        }
        assert_eq!(
            lock(&host.shared.state)
                .incoming_slots
                .load(Ordering::Acquire),
            4
        );
        let rejected = start_direct();
        pair_for_control(&host, &rejected);
        assert!(rejected.connect(&host.endpoint_id()).is_err());
        proceed.send(()).unwrap();
        while lock(&applied).len() != 2
            || lock(&host.shared.state)
                .incoming_slots
                .load(Ordering::Acquire)
                != 3
        {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(2));
        }
        assert_eq!(
            *lock(&applied),
            vec![
                InputEvent::key_down(input::usage::A),
                InputEvent::key_up(input::usage::A)
            ]
        );
        let replacement = rejected.connect(&host.endpoint_id()).unwrap();
        assert!(replacement.generation > parent.generation);
        host.shutdown();
        peer.shutdown();
        rejected.shutdown();
        for replacement in replacements {
            replacement.shutdown();
        }
        assert!(settled,"parent remained Stopping until native acknowledgement; stop watcher cannot cancel apply_host_input await");
    }
}

#[cfg(test)]
mod v12_replacement_drop_probe {
    use super::*;
    struct Old {
        entered: std::sync::mpsc::SyncSender<()>,
        proceed: std::sync::mpsc::Receiver<()>,
    }
    impl HostDesktop for Old {
        fn monitor(&self) -> (u16, u16) {
            (640, 480)
        }
        fn apply(&mut self, _: InputEvent) -> Result<(), Error> {
            Ok(())
        }
    }
    impl Drop for Old {
        fn drop(&mut self) {
            self.entered.send(()).unwrap();
            self.proceed
                .recv_timeout(std::time::Duration::from_secs(5))
                .unwrap();
        }
    }
    struct New;
    impl HostDesktop for New {
        fn monitor(&self) -> (u16, u16) {
            (640, 480)
        }
        fn apply(&mut self, _: InputEvent) -> Result<(), Error> {
            Ok(())
        }
    }
    #[test]
    fn v12_adapter_replacement_must_drop_old_backend_outside_core_state() {
        let core = Arc::new(crate::tests::support::start_direct());
        let (entered, ready) = std::sync::mpsc::sync_channel(1);
        let (proceed, wait) = std::sync::mpsc::channel();
        core.set_desktop(Box::new(Old {
            entered,
            proceed: wait,
        }))
        .unwrap();
        let worker = core.clone();
        let replacement = std::thread::spawn(move || worker.set_desktop(Box::new(New)));
        ready
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap();
        let available = core.shared.state.try_lock().is_ok();
        let fenced =
            core.shared.desktop.try_lock().is_err() && core.shared.desktop_gate.try_lock().is_err();
        if available {
            core.snapshot().unwrap();
            assert!(matches!(core.set_desktop(Box::new(New)), Err(Error::Busy)));
        }
        proceed.send(()).unwrap();
        replacement.join().unwrap().unwrap();
        assert!(core.shared.desktop.try_lock().is_ok());
        assert!(core.shared.desktop_gate.try_lock().is_ok());
        core.shutdown();
        assert!(fenced, "new input must wait for old adapter teardown");
        assert!(
            available,
            "old native adapter destructor runs while Core state is locked"
        );
    }
    #[test]
    fn v12_rejected_adapter_teardown_keeps_core_responsive() {
        let core = Arc::new(crate::tests::support::start_direct());
        core.set_desktop(Box::new(New)).unwrap();
        core.shared.input_cleanup.store(1, Ordering::Release);
        let (entered, ready) = std::sync::mpsc::sync_channel(1);
        let (proceed, wait) = std::sync::mpsc::channel();
        let worker = core.clone();
        let replacement = std::thread::spawn(move || {
            worker.set_desktop(Box::new(Old {
                entered,
                proceed: wait,
            }))
        });
        ready
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap();
        let available = core.shared.state.try_lock().is_ok();
        if available {
            core.snapshot().unwrap();
        }
        let intact = core.shared.desktop.try_lock().unwrap().is_some();
        proceed.send(()).unwrap();
        assert!(matches!(replacement.join().unwrap(), Err(Error::Busy)));
        core.shared.input_cleanup.store(0, Ordering::Release);
        core.shutdown();
        assert!(available, "rejected candidate destructor holds Core State");
        assert!(intact, "rejection removed the installed adapter");
    }
}
