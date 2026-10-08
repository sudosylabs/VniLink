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
        let state = self.alive_state()?;
        if state
            .incoming
            .as_ref()
            .is_some_and(|session| !finished(&session.phase) && !session.services_only)
        {
            return Err(Error::Busy);
        }
        let _gate = self
            .shared
            .desktop_gate
            .try_lock()
            .map_err(|_| Error::Busy)?;
        *lock(&self.shared.desktop) = Some(desktop);
        Ok(())
    }

    /// Inject authorized keyboard and pointer events into this operating system.
    ///
    /// On macOS this can prompt for Accessibility permission. It does not capture
    /// the screen. Linux support is the X11 backend; Wayland is not claimed.
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
        outbox.push(event)?;
        held.observe(event);
        Ok(())
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
    let active = state.incoming.as_ref().and_then(|session| {
        (session.phase == SessionPhase::Active
            && !session.files_only
            && !*session.desktop_stop.borrow()
            && state.trust.granted(&session.peer_key, Grant::Control))
        .then(|| SessionStamp::new(session.id, session.generation))
    });
    session::check_input(active, stamp)
}

pub(super) async fn host_input_loop(
    shared: &Shared,
    session_id: u64,
    peer: EndpointId,
    send: &mut iroh::endpoint::SendStream,
    recv: &mut iroh::endpoint::RecvStream,
    stop_rx: &mut tokio::sync::watch::Receiver<bool>,
) -> bool {
    let mut held = HeldInput::default();
    let mut input_open = true;
    let mut desktop_stop = lock(&shared.state)
        .incoming
        .as_ref()
        .expect("admitted session")
        .desktop_stop
        .subscribe();
    let mut retired = false;
    let local_stop = loop {
        if *stop_rx.borrow() || lock(&shared.state).shut_down {
            break true;
        }
        tokio::select! {
            biased;
            _ = desktop_stop.changed(), if !retired => {
                retired = true;
                release_held(shared, session_id, &held);
                held = HeldInput::default();
                input_open = false;
            }
            changed = stop_rx.changed() => {
                if changed.is_err() || *stop_rx.borrow() {
                    break true;
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
                        match check_authorized(shared, send, stamp).await {
                            Some(true) => {},
                            None => continue,
                            Some(false) => break false,
                        }
                        shared
                            .events
                            .publish(CoreEvent::KeyframeRequested { id: session_id });
                    }
                    ControlMessage::Quality { stamp, quality } => {
                        match check_authorized(shared, send, stamp).await {
                            Some(true) => {},
                            None => continue,
                            Some(false) => break false,
                        }
                        let mut state = lock(&shared.state);
                        if let Some(session) = state.incoming.as_mut().filter(|s| s.id == session_id && s.phase == SessionPhase::Active && !*s.desktop_stop.borrow()) {
                            session.quality = Some(quality);
                            shared.events.publish(CoreEvent::QualityUpdated { id: session_id, quality });
                        }
                    }
                    ControlMessage::Input { stamp, event } => {
                        match check_authorized(shared, send, stamp).await {
                            Some(true) => {},
                            None => continue,
                            Some(false) => break false,
                        }
                        if apply_host_input(shared, session_id, send, &mut input_open, || {
                            let state = lock(&shared.state);
                            if state.incoming.as_ref().is_none_or(|s| s.id != session_id || *s.desktop_stop.borrow()) { return Err(Error::NoSession); }
                            let mut desktop = lock(&shared.desktop);
                            match desktop.as_mut() {
                                None => Err(Error::Unsupported("no desktop adapter is installed")),
                                Some(desktop) => desktop.apply(event),
                            }
                        })
                        .await
                        {
                            held.observe(event);
                        }
                    }
                    ControlMessage::Text { stamp, text } => {
                        match check_authorized(shared, send, stamp).await {
                            Some(true) => {},
                            None => continue,
                            Some(false) => break false,
                        }
                        apply_host_input(shared, session_id, send, &mut input_open, || {
                            let state = lock(&shared.state);
                            if state.incoming.as_ref().is_none_or(|s| s.id != session_id || *s.desktop_stop.borrow()) { return Err(Error::NoSession); }
                            apply_key_text(shared, &text.0, false)
                        })
                        .await;
                    }
                    ControlMessage::Chord { stamp, text } => {
                        match check_authorized(shared, send, stamp).await {
                            Some(true) => {},
                            None => continue,
                            Some(false) => break false,
                        }
                        apply_host_input(shared, session_id, send, &mut input_open, || {
                            let state = lock(&shared.state);
                            if state.incoming.as_ref().is_none_or(|s| s.id != session_id || *s.desktop_stop.borrow()) { return Err(Error::NoSession); }
                            apply_key_text(shared, &text.0, true)
                        })
                        .await;
                    }
                    _ => {
                        let _ = write_frame(send, &ControlMessage::Reject(error_code(&Error::Malformed))).await;
                        break false;
                    }
                }
            }
        }
    };
    release_held(shared, session_id, &held);
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
) -> Option<bool> {
    let decision = {
        let state = lock(&shared.state);
        if state
            .incoming
            .as_ref()
            .is_some_and(|s| s.wire_stamp == stamp && *s.desktop_stop.borrow())
        {
            return None;
        }
        check_input_allowed(&state, stamp)
    };
    if let Err(error) = decision {
        let _ = write_frame(send, &ControlMessage::Reject(error_code(&error))).await;
        return Some(false);
    }
    Some(true)
}

/// Gate, apply, and failure note for one host message. The first
/// [`Error::Unsupported`] from the adapter closes input with a reject; later
/// messages are only noted. Returns whether the message was applied.
async fn apply_host_input(
    shared: &Shared,
    session_id: u64,
    send: &mut iroh::endpoint::SendStream,
    input_open: &mut bool,
    apply: impl FnOnce() -> Result<(), Error>,
) -> bool {
    if !*input_open {
        return false;
    }
    match apply() {
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

pub(super) fn apply_key_text(shared: &Shared, text: &str, chord: bool) -> Result<(), Error> {
    if text.is_empty() {
        return Ok(());
    }
    if let Some(message) = input::key_text_problem(text) {
        return Err(Error::InputFailed(message));
    }
    let mut desktop = lock(&shared.desktop);
    match desktop.as_mut() {
        None => Err(Error::Unsupported("no desktop adapter is installed")),
        Some(desktop) if chord => desktop.type_chord(text),
        Some(desktop) => desktop.type_text(text),
    }
}

pub(super) fn release_held(shared: &Shared, session_id: u64, held: &HeldInput) {
    let mut desktop = lock(&shared.desktop);
    let Some(desktop) = desktop.as_mut() else {
        return;
    };
    for event in held.release_events() {
        if desktop.apply(event).is_ok() {
            continue;
        }
        shared.events.publish(CoreEvent::InputFailed {
            id: session_id,
            message: "a held key or button could not be released",
        });
    }
}
