//! The host's input adapter, a controller's input defaults, and a session's
//! control stream on the host.
// trace: records every ignored failure

use super::*;

use super::link::remember_peer_addresses;
use super::wire::{read_session_frame, write_frame};

use std::sync::atomic::Ordering;
use std::sync::Arc;

use iroh::EndpointId;

use crate::address::Announcement;
use crate::error::Error;
use crate::events::CoreEvent;
use crate::input::{self, HostDesktop};
use crate::keyboard::KeyboardMode;
use crate::os_input::OsInput;
use crate::protocol::{error_code, ControlMessage};

impl Core {
    /// Installs the adapter that applies keyboard and pointer events on this host.
    ///
    /// A connection may configure its adapter before Desktop Open. Refused
    /// while Desktop is active or releasing held input.
    pub fn set_desktop(&self, desktop: Box<dyn HostDesktop>) -> Result<(), Error> {
        let _engine = self.engine();
        let monitor = desktop.monitor();
        // The candidate lives outside the admission scope, so rejection also
        // destroys native resources only after the global State guard is gone.
        let mut candidate = Some(desktop);
        let (_gate, _adapter, old) = (|| {
            let _state = self.alive_state()?;
            if self.shared.desktop_streams.load(Ordering::Acquire) != 0 {
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
        let _engine = self.engine();
        self.set_desktop(Box::new(OsInput::open_with_prompt(false)?))
    }

    pub fn enable_os_input(&self) -> Result<(), Error> {
        let _engine = self.engine();
        self.set_desktop(Box::new(OsInput::open()?))
    }

    /// Applies application defaults to every open desktop and to the
    /// desktops opened later. Validation and held-input checks precede
    /// every mutation.
    pub fn set_input_defaults(
        &self,
        mode: KeyboardMode,
        swap: bool,
        pointer: crate::PointerSettings,
    ) -> Result<(), Error> {
        let _engine = self.engine();
        pointer.validate()?;
        let state = self.alive_state()?;
        let desktops: Vec<_> = state
            .services
            .values()
            .filter_map(|s| s.desktop.clone())
            .collect();
        let mut desktop_plans: Vec<_> = desktops.iter().map(|d| lock(&d.keyboard)).collect();
        if desktop_plans.iter().any(|p| !p.idle()) {
            return Err(Error::InvalidSetting(
                "release held keys and buttons before changing input defaults",
            ));
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

    /// Exchanges Control and Command on what the desktops this controller
    /// opens from now on send.
    ///
    /// Off by default. A Mac Command shortcut can be sent as Control for a
    /// Windows or Linux host, and the reverse. The host injects the physical
    /// key it receives. The choice belongs to this process and is not saved;
    /// an open desktop keeps its own.
    pub fn set_swap_control_and_command(&self, enabled: bool) -> Result<(), Error> {
        let _engine = self.engine();
        drop(self.alive_state()?);
        self.shared
            .swap_control_command
            .store(enabled, Ordering::Relaxed);
        Ok(())
    }

    /// Whether this controller exchanges Control and Command.
    pub fn swap_control_and_command(&self) -> Result<bool, Error> {
        let _engine = self.engine();
        drop(self.alive_state()?);
        Ok(self.shared.swap_control_command.load(Ordering::Relaxed))
    }

    /// Chooses how a local key becomes host input on the desktops this
    /// controller opens from now on.
    ///
    /// [`KeyboardMode::Character`] is the default. The character the controller
    /// operating system produced is what the host inserts. [`KeyboardMode::Map`]
    /// sends the physical key. [`KeyboardMode::Translate`] asks the host to
    /// press the chord in its own layout. The choice belongs to this process
    /// and is not saved; an open desktop keeps its own.
    pub fn set_keyboard_mode(&self, mode: KeyboardMode) -> Result<(), Error> {
        let _engine = self.engine();
        drop(self.alive_state()?);
        self.shared
            .keyboard_mode
            .store(mode as u8, Ordering::Relaxed);
        Ok(())
    }

    /// How this controller turns a local key into host input.
    pub fn keyboard_mode(&self) -> Result<KeyboardMode, Error> {
        let _engine = self.engine();
        drop(self.alive_state()?);
        Ok(KeyboardMode::from_u8(
            self.shared.keyboard_mode.load(Ordering::Relaxed),
        ))
    }
}

/// How often a host re-evaluates what it reports to a linked peer.
const AVAILABILITY_INTERVAL: std::time::Duration = std::time::Duration::from_secs(1);

/// The host's side of a session's control stream: the peer's addresses
/// come in; the host's topology, and on a presence link what it allows, go
/// out. A Desktop service's pictures and input go on its own stream.
pub(super) async fn host_command_loop(
    shared: &Arc<Shared>,
    session_id: u64,
    peer: EndpointId,
    send: &mut iroh::endpoint::SendStream,
    recv: &mut iroh::endpoint::RecvStream,
    stop_rx: &mut tokio::sync::watch::Receiver<bool>,
) -> bool {
    let stamp = lock(&shared.state)
        .incoming
        .get(&session_id)
        .map(|session| session.wire_stamp);
    let mut topology_rx = shared.topology_watch.subscribe();
    if let Some(stamp) = stamp {
        if let Err(error) = super::topology::write_topology(shared, send, stamp).await {
            control_stream_ended(session_id, &error);
            return false;
        }
    }
    // A link tells the peer what this computer allows, now and whenever
    // that changes. Policy and backend setters publish no event, so the
    // report is re-evaluated on a short interval and sent on change.
    let mut reported = Vec::new();
    let mut availability_tick = tokio::time::interval(AVAILABILITY_INTERVAL);
    availability_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        if *stop_rx.borrow() || lock(&shared.state).shut_down {
            break true;
        }
        tokio::select! {
            biased;
            changed = stop_rx.changed() => {
                if changed.is_err() || *stop_rx.borrow() {
                    break true;
                }
            }
            _ = availability_tick.tick() => {
                let report = super::availability::remote_report(shared, peer.as_bytes());
                if report != reported {
                    if let Err(error) =
                        write_frame(send, &ControlMessage::Availability { entries: report.clone() })
                            .await
                    {
                        control_stream_ended(session_id, &error);
                        break false;
                    }
                    reported = report;
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
                    if let Err(error) = super::topology::write_topology(shared, send, stamp).await {
                        control_stream_ended(session_id, &error);
                        break false;
                    }
                }
            }
            frame = read_session_frame(recv) => {
                let message = match frame {
                    Ok(message) => message,
                    Err(Error::Malformed) => {
                        tracing::info!(target: crate::trace::SESSION, session = session_id, "a malformed control frame ended the session");
                        crate::ignored!(crate::trace::SESSION, "the session ends whether the peer hears why", write_frame(send, &ControlMessage::Reject(error_code(&Error::Malformed))).await);
                        break false;
                    }
                    Err(error) => {
                        control_stream_ended(session_id, &error);
                        break false;
                    }
                };
                match message {
                    ControlMessage::Addresses { relays, direct } => {
                        crate::ignored!(
                            crate::trace::RELAY,
                            "a peer's addresses are a hint; the next announcement retries",
                            remember_peer_addresses(
                                shared,
                                peer.as_bytes(),
                                Announcement { relays, direct },
                            )
                        );
                    }
                    ControlMessage::Close => break false,
                    _ => {
                        tracing::info!(target: crate::trace::SESSION, session = session_id, "an unexpected control message ended the session");
                        crate::ignored!(crate::trace::SESSION, "the session ends whether the peer hears why", write_frame(send, &ControlMessage::Reject(error_code(&Error::Malformed))).await);
                        break false;
                    }
                }
            }
        }
    }
}

/// Records that a session's control stream ended under `error`, whether a
/// read or a write met it first: a peer that leaves while the host is
/// writing ends the stream as surely as one that leaves while it reads.
fn control_stream_ended(session_id: u64, error: &Error) {
    tracing::debug!(
        target: crate::trace::SESSION,
        session = session_id,
        error = error.name(),
        "the session's control stream ended"
    );
}

pub(super) fn note_input_failure(shared: &Shared, session_id: u64, error: &Error) {
    let message = input::InputFault::from_error(error).message();
    tracing::warn!(
        target: crate::trace::INPUT,
        session = session_id,
        reason = %message,
        "input failed"
    );
    shared.events.publish(CoreEvent::InputFailed {
        id: session_id,
        message,
    });
}

#[cfg(test)]
mod v12_replacement_drop_probe {
    use super::*;
    use crate::input::InputEvent;
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
    /// Whether a Core operation can take State while a destructor is parked.
    ///
    /// Core's own tasks, such as the power ticker and the accept loop, take
    /// State for moments, so a failed `try_lock` proves nothing. A snapshot
    /// that completes within the bound shows the destructor does not hold
    /// State; one queued behind it cannot answer until the destructor is
    /// released. Join the probe after releasing it.
    fn state_available(core: &Arc<Core>) -> (bool, std::thread::JoinHandle<()>) {
        let (answered, answer) = std::sync::mpsc::channel();
        let prober = core.clone();
        let probe = std::thread::spawn(move || {
            let _ = answered.send(prober.snapshot().is_ok());
        });
        let available = answer.recv_timeout(std::time::Duration::from_secs(2)) == Ok(true);
        (available, probe)
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
        let core = Arc::new(crate::test_support::loopback_core());
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
        let (available, probe) = state_available(&core);
        let fenced =
            core.shared.desktop.try_lock().is_err() && core.shared.desktop_gate.try_lock().is_err();
        if available {
            assert!(matches!(core.set_desktop(Box::new(New)), Err(Error::Busy)));
        }
        proceed.send(()).unwrap();
        probe.join().unwrap();
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
        let core = Arc::new(crate::test_support::loopback_core());
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
        let (available, probe) = state_available(&core);
        let intact = core.shared.desktop.try_lock().unwrap().is_some();
        proceed.send(()).unwrap();
        probe.join().unwrap();
        assert!(matches!(replacement.join().unwrap(), Err(Error::Busy)));
        core.shared.input_cleanup.store(0, Ordering::Release);
        core.shutdown();
        assert!(available, "rejected candidate destructor holds Core State");
        assert!(intact, "rejection removed the installed adapter");
    }
}
