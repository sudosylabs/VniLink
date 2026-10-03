use std::sync::{Arc, Mutex};
use std::time::Duration;

use iroh::RelayMode;

use crate::core::{Core, CoreConfig, Listen};
use crate::error::Error;
use crate::events::{CoreEvent, EventSubscription};
use crate::input::{HostDesktop, InputEvent};
use crate::session::{Grants, SessionPhase, SessionRole};
use crate::video::HostCapture;

pub(super) fn start_direct() -> Core {
    Core::boot(
        CoreConfig {
            listen: Listen::Loopback,
            ..CoreConfig::default()
        },
        RelayMode::Disabled,
        true,
    )
    .expect("loopback core")
}

pub(super) fn wait_closed(core: &Core, role: SessionRole) {
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        let snapshot = core.snapshot().unwrap();
        let session = match role {
            SessionRole::Host => snapshot.incoming,
            SessionRole::Controller => snapshot.outgoing,
        };
        if session
            .as_ref()
            .is_some_and(|session| session.phase == SessionPhase::Closed)
        {
            return;
        }
        if std::time::Instant::now() > deadline {
            panic!("session did not close: {session:?}");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

pub(super) struct RecordingDesktop {
    pub(super) events: Arc<Mutex<Vec<InputEvent>>>,
}

impl HostDesktop for RecordingDesktop {
    fn apply(&mut self, event: InputEvent) -> Result<(), Error> {
        self.events
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .push(event);
        Ok(())
    }

    fn monitor(&self) -> (u16, u16) {
        (1920, 1080)
    }
}

pub(super) struct FixedDesktop {
    pub(super) width: u16,
    pub(super) height: u16,
    pub(super) events: Arc<Mutex<Vec<InputEvent>>>,
    pub(super) fail_key_down: Option<u16>,
    pub(super) fail_release: bool,
}

impl HostDesktop for FixedDesktop {
    fn apply(&mut self, event: InputEvent) -> Result<(), Error> {
        match event {
            InputEvent::Key { down: false, .. } | InputEvent::Button { down: false, .. }
                if self.fail_release =>
            {
                return Err(Error::InputFailed(
                    "the operating system rejected the event",
                ));
            }
            InputEvent::Key { usage, down: true } if self.fail_key_down == Some(usage) => {
                return Err(Error::InputFailed(
                    "the operating system rejected the event",
                ));
            }
            _ => {}
        }
        self.events
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .push(event);
        Ok(())
    }

    fn monitor(&self) -> (u16, u16) {
        (self.width, self.height)
    }
}

pub(super) struct ShutdownCapture;

impl HostCapture for ShutdownCapture {
    fn monitor(&self) -> (u16, u16) {
        (1, 1)
    }
}

pub(super) fn pair_for_control(host: &Core, controller: &Core) {
    host.start_hosting().unwrap();
    let token = host.create_invitation(Duration::from_secs(60)).unwrap();
    controller.accept_invitation(&token).unwrap();
    let back = controller
        .create_invitation(Duration::from_secs(60))
        .unwrap();
    host.accept_invitation(&back).unwrap();
    host.authorize(
        &controller.endpoint_id(),
        Grants {
            control: true,
            clipboard: false,
            transfer: false,
        },
    )
    .unwrap();
}

pub(super) fn take_matching(
    subscription: &EventSubscription,
    mut pred: impl FnMut(&CoreEvent) -> bool,
) -> CoreEvent {
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    loop {
        if std::time::Instant::now() > deadline {
            panic!("timed out waiting for an event");
        }
        if let Some(event) = subscription.recv_timeout(Duration::from_millis(20)) {
            if pred(&event) {
                return event;
            }
        }
    }
}

pub(super) fn boot_capacity(event_capacity: usize) -> Result<Core, Error> {
    Core::boot(
        CoreConfig {
            listen: Listen::Loopback,
            event_capacity,
            ..CoreConfig::default()
        },
        RelayMode::Disabled,
        true,
    )
}

pub(super) fn wait_applied(events: &Mutex<Vec<InputEvent>>, expected: &[InputEvent]) {
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    loop {
        let got = events.lock().unwrap_or_else(|poison| poison.into_inner());
        if got.len() >= expected.len() {
            assert_eq!(got.as_slice(), expected);
            return;
        }
        drop(got);
        if std::time::Instant::now() > deadline {
            let got = events.lock().unwrap_or_else(|poison| poison.into_inner());
            panic!("applied events did not settle: {got:?}");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}
