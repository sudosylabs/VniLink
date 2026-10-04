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
    /// Refused while a controlling session is active. The session releases held
    /// keys when it ends, so swap the adapter between sessions.
    pub fn set_desktop(&self, desktop: Box<dyn HostDesktop>) -> Result<(), Error> {
        let state = self.alive_state()?;
        if state
            .incoming
            .as_ref()
            .is_some_and(|session| !finished(&session.phase))
        {
            return Err(Error::Busy);
        }
        drop(state);
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
        let event = if self.shared.swap_control_command.load(Ordering::Relaxed) {
            input::swap_command_key(event)
        } else {
            event
        };
        outbox.push(event)
    }

    /// Exchanges Control and Command on events this controller sends.
    ///
    /// Off by default. A Mac Command shortcut can be sent as Control for a
    /// Windows or Linux host, and the reverse. The host injects the physical
    /// key it receives. The choice belongs to this process and is not saved.
    /// Release held modifiers before changing it, so the key that goes down
    /// is the key that comes up.
    pub fn set_swap_control_and_command(&self, enabled: bool) -> Result<(), Error> {
        let generation = self.controller_generation()?;
        let mut plan = lock(&self.shared.keyboard);
        plan.reset_if_generation(generation);
        if plan.is_held() {
            return Err(Error::InvalidSetting(
                "release held keys before changing the Control/Command swap",
            ));
        }
        drop(plan);
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
        let generation = self.controller_generation()?;
        let mut plan = lock(&self.shared.keyboard);
        plan.reset_if_generation(generation);
        if plan.is_held() {
            return Err(Error::InvalidSetting(
                "release held keys before changing the keyboard mode",
            ));
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
        let mode = KeyboardMode::from_u8(self.shared.keyboard_mode.load(Ordering::Relaxed));
        let swap = self.shared.swap_control_command.load(Ordering::Relaxed);
        let mut plan = lock(&self.shared.keyboard);
        plan.reset_if_generation(generation);
        let mut trial = plan.clone();
        let planned = trial.apply(mode, swap, key)?;
        let items = planned
            .into_iter()
            .map(keyboard::Planned::outbound)
            .collect::<Vec<_>>();
        outbox.push_batch(&items)?;
        *plan = trial;
        Ok(())
    }

    pub(super) fn controller_generation(&self) -> Result<u64, Error> {
        let state = self.alive_state()?;
        Ok(state
            .outgoing
            .as_ref()
            .filter(|session| session.phase == SessionPhase::Active)
            .map(|session| session.generation)
            .unwrap_or(0))
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
        let session = state.outgoing.as_ref().ok_or(Error::NoSession)?;
        if session.phase != SessionPhase::Active || session.id != session_id {
            return Err(Error::NoSession);
        }
        if session.generation != generation {
            return Err(Error::StaleSession);
        }
        session.outbox.clone().ok_or(Error::NoSession)
    }
}

/// A control message belongs to the live session only while its generation is
/// current, the session is active, and the owner still grants control.
/// Clipboard and transfer re-check their grants per message; control does the
/// same here, so `authorize` during the session takes effect on the next
/// message and held keys release when the loop ends.
pub(super) fn check_input_allowed(shared: &Shared, stamp: SessionStamp) -> Result<(), Error> {
    let state = lock(&shared.state);
    let active = state.incoming.as_ref().and_then(|session| {
        (session.phase == SessionPhase::Active
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
    let local_stop = loop {
        if *stop_rx.borrow() || lock(&shared.state).shut_down {
            break true;
        }
        tokio::select! {
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
                        if !check_authorized(shared, send, stamp).await {
                            break false;
                        }
                        shared
                            .events
                            .publish(CoreEvent::KeyframeRequested { id: session_id });
                    }
                    ControlMessage::Input { stamp, event } => {
                        if !check_authorized(shared, send, stamp).await {
                            break false;
                        }
                        if apply_host_input(shared, session_id, send, &mut input_open, || {
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
                        if !check_authorized(shared, send, stamp).await {
                            break false;
                        }
                        apply_host_input(shared, session_id, send, &mut input_open, || {
                            apply_key_text(shared, &text.0, false)
                        })
                        .await;
                    }
                    ControlMessage::Chord { stamp, text } => {
                        if !check_authorized(shared, send, stamp).await {
                            break false;
                        }
                        apply_host_input(shared, session_id, send, &mut input_open, || {
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
/// chord frame passes here before it is applied. Returns false when the loop
/// must end.
async fn check_authorized(
    shared: &Shared,
    send: &mut iroh::endpoint::SendStream,
    stamp: SessionStamp,
) -> bool {
    if let Err(error) = check_input_allowed(shared, stamp) {
        let _ = write_frame(send, &ControlMessage::Reject(error_code(&error))).await;
        return false;
    }
    true
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
