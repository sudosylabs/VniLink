use std::sync::{Arc, Mutex};
use std::time::Duration;

use iroh::RelayMode;

use crate::core::{Core, CoreConfig, Listen};
use crate::error::Error;
use crate::events::EventSubscription;
use crate::input::{HostDesktop, InputEvent};
use crate::keyboard::LocalKey;
use crate::session::Grants;

pub(super) fn start_direct() -> Core {
    let core = Core::boot(
        CoreConfig {
            listen: Listen::Loopback,
            ..CoreConfig::default()
        },
        RelayMode::Disabled,
    )
    .expect("loopback core");
    core.set_h264_support(true, true).expect("flags");
    core
}

pub(super) enum Seen {
    Event(InputEvent),
    Text(String),
    Chord(String),
}

pub(super) struct RecordingDesktop {
    pub(super) seen: Arc<Mutex<Vec<Seen>>>,
    pub(super) fail_text: bool,
    pub(super) fail_chord: bool,
}

impl HostDesktop for RecordingDesktop {
    fn apply(&mut self, event: InputEvent) -> Result<(), Error> {
        self.seen
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .push(Seen::Event(event));
        Ok(())
    }

    fn type_text(&mut self, text: &str) -> Result<(), Error> {
        if self.fail_text {
            return Err(Error::InputFailed(
                "the operating system rejected the event",
            ));
        }
        self.seen
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .push(Seen::Text(text.to_owned()));
        Ok(())
    }

    fn type_chord(&mut self, text: &str) -> Result<(), Error> {
        if self.fail_chord {
            return Err(Error::InputFailed("translated keys are not available"));
        }
        self.seen
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .push(Seen::Chord(text.to_owned()));
        Ok(())
    }

    fn monitor(&self) -> (u16, u16) {
        (1920, 1080)
    }
}

pub(super) fn pair(host: &Core, controller: &Core) {
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

pub(super) fn wait_until(mut ready: impl FnMut() -> bool) {
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        if ready() {
            return;
        }
        if std::time::Instant::now() > deadline {
            panic!("condition was not met");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

pub(super) fn seen(slot: &Mutex<Vec<Seen>>) -> Vec<Seen> {
    slot.lock()
        .unwrap_or_else(|poison| poison.into_inner())
        .clone()
}

impl Clone for Seen {
    fn clone(&self) -> Self {
        match self {
            Self::Event(event) => Self::Event(*event),
            Self::Text(text) => Self::Text(text.clone()),
            Self::Chord(text) => Self::Chord(text.clone()),
        }
    }
}

impl PartialEq for Seen {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Event(left), Self::Event(right)) => left == right,
            (Self::Text(left), Self::Text(right)) => left == right,
            (Self::Chord(left), Self::Chord(right)) => left == right,
            _ => false,
        }
    }
}

pub(super) fn wait_seen(slot: &Mutex<Vec<Seen>>, expected: &[Seen]) {
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        let got = seen(slot);
        if got == expected {
            return;
        }
        if std::time::Instant::now() > deadline {
            panic!(
                "desktop saw {} items, expected {}",
                got.len(),
                expected.len()
            );
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

pub(super) fn hides(subscription: &EventSubscription, sample: &str) {
    let deadline = std::time::Instant::now() + Duration::from_millis(200);
    while std::time::Instant::now() < deadline {
        if let Some(event) = subscription.recv_timeout(Duration::from_millis(20)) {
            assert!(!format!("{event:?}").contains(sample), "{event:?}");
        }
    }
}

pub(super) fn local<'a>(usage: u16, down: bool, text: &'a str) -> LocalKey<'a> {
    LocalKey { usage, down, text }
}

pub(super) fn recording(seen: Arc<Mutex<Vec<Seen>>>, fail_chord: bool) -> RecordingDesktop {
    RecordingDesktop {
        seen,
        fail_text: false,
        fail_chord,
    }
}
