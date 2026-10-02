use std::sync::{Arc, Mutex};
use std::time::Duration;

use iroh::RelayMode;

use crate::core::{Core, CoreConfig, Listen};
use crate::error::Error;
use crate::events::{CoreEvent, EventSubscription};
use crate::input::{usage, HostDesktop, InputEvent, MAX_KEY_TEXT};
use crate::session::{Grants, SessionPhase};

fn start_direct() -> Core {
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

enum Seen {
    Event(InputEvent),
    Text(String),
}

struct RecordingDesktop {
    seen: Arc<Mutex<Vec<Seen>>>,
    fail_text: bool,
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

    fn monitor(&self) -> (u16, u16) {
        (1920, 1080)
    }
}

fn pair(host: &Core, controller: &Core) {
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

fn wait_until(mut ready: impl FnMut() -> bool) {
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

fn seen(slot: &Mutex<Vec<Seen>>) -> Vec<Seen> {
    slot.lock()
        .unwrap_or_else(|poison| poison.into_inner())
        .clone()
}

impl Clone for Seen {
    fn clone(&self) -> Self {
        match self {
            Self::Event(event) => Self::Event(*event),
            Self::Text(text) => Self::Text(text.clone()),
        }
    }
}

impl PartialEq for Seen {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Event(left), Self::Event(right)) => left == right,
            (Self::Text(left), Self::Text(right)) => left == right,
            _ => false,
        }
    }
}

fn wait_seen(slot: &Mutex<Vec<Seen>>, expected: &[Seen]) {
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

fn hides(subscription: &EventSubscription, sample: &str) {
    let deadline = std::time::Instant::now() + Duration::from_millis(200);
    while std::time::Instant::now() < deadline {
        if let Some(event) = subscription.recv_timeout(Duration::from_millis(20)) {
            assert!(!format!("{event:?}").contains(sample), "{event:?}");
        }
    }
}

#[test]
fn the_command_swap_is_process_local_and_off_by_default() {
    let core = start_direct();
    assert!(!core.swap_control_and_command().unwrap());
    core.set_swap_control_and_command(true).unwrap();
    assert!(core.swap_control_and_command().unwrap());
    core.set_swap_control_and_command(false).unwrap();
    assert!(!core.swap_control_and_command().unwrap());
    core.shutdown();
    assert!(matches!(
        core.set_swap_control_and_command(true),
        Err(Error::ShutDown)
    ));
}

#[test]
fn a_swapped_command_is_the_key_the_host_releases() {
    let host = start_direct();
    let controller = start_direct();
    let applied = Arc::new(Mutex::new(Vec::new()));
    host.set_desktop(Box::new(RecordingDesktop {
        seen: Arc::clone(&applied),
        fail_text: false,
    }))
    .unwrap();
    pair(&host, &controller);
    controller.set_swap_control_and_command(true).unwrap();
    let session = controller.connect(&host.endpoint_id()).unwrap();
    controller
        .submit_input(
            session.id,
            session.generation,
            InputEvent::key_down(usage::LEFT_META),
        )
        .unwrap();
    controller
        .submit_input(
            session.id,
            session.generation,
            InputEvent::key_down(usage::LEFT_SHIFT),
        )
        .unwrap();
    wait_seen(
        &applied,
        &[
            Seen::Event(InputEvent::key_down(usage::LEFT_CONTROL)),
            Seen::Event(InputEvent::key_down(usage::LEFT_SHIFT)),
        ],
    );
    host.stop_hosted_session().unwrap();
    wait_seen(
        &applied,
        &[
            Seen::Event(InputEvent::key_down(usage::LEFT_CONTROL)),
            Seen::Event(InputEvent::key_down(usage::LEFT_SHIFT)),
            Seen::Event(InputEvent::key_up(usage::LEFT_SHIFT)),
            Seen::Event(InputEvent::key_up(usage::LEFT_CONTROL)),
        ],
    );
}

#[test]
fn finished_text_is_typed_in_order_and_is_not_logged() {
    let host = start_direct();
    let controller = start_direct();
    let applied = Arc::new(Mutex::new(Vec::new()));
    host.set_desktop(Box::new(RecordingDesktop {
        seen: Arc::clone(&applied),
        fail_text: false,
    }))
    .unwrap();
    pair(&host, &controller);
    let session = controller.connect(&host.endpoint_id()).unwrap();
    let events = controller.subscribe().unwrap();
    let host_events = host.subscribe().unwrap();
    assert!(matches!(
        controller.submit_text(session.id, session.generation, ""),
        Err(Error::InvalidSetting("text entry is empty"))
    ));
    assert!(matches!(
        controller.submit_text(
            session.id,
            session.generation,
            &"a".repeat(MAX_KEY_TEXT + 1)
        ),
        Err(Error::InvalidSetting("text entry is longer than 128 bytes"))
    ));
    assert!(matches!(
        controller.submit_text(session.id, session.generation, "a\0b"),
        Err(Error::InvalidSetting("text entry contains a null byte"))
    ));
    controller
        .submit_input(
            session.id,
            session.generation,
            InputEvent::key_down(usage::LEFT_META),
        )
        .unwrap();
    controller
        .submit_text(session.id, session.generation, "café")
        .unwrap();
    controller
        .submit_input(
            session.id,
            session.generation,
            InputEvent::key_down(usage::ENTER),
        )
        .unwrap();
    wait_seen(
        &applied,
        &[
            Seen::Event(InputEvent::key_down(usage::LEFT_META)),
            Seen::Text("café".to_owned()),
            Seen::Event(InputEvent::key_down(usage::ENTER)),
        ],
    );
    hides(&events, "café");
    hides(&host_events, "café");
    assert_eq!(
        host.snapshot().unwrap().incoming.unwrap().phase,
        SessionPhase::Active
    );
}

#[test]
fn a_refused_text_leaves_the_keys_working() {
    let host = start_direct();
    let controller = start_direct();
    let applied = Arc::new(Mutex::new(Vec::new()));
    host.set_desktop(Box::new(RecordingDesktop {
        seen: Arc::clone(&applied),
        fail_text: true,
    }))
    .unwrap();
    pair(&host, &controller);
    let session = controller.connect(&host.endpoint_id()).unwrap();
    let events = host.subscribe().unwrap();
    controller
        .submit_text(session.id, session.generation, "nope")
        .unwrap();
    wait_until(|| {
        events
            .recv_timeout(Duration::from_millis(20))
            .is_some_and(|event| {
                matches!(
                    event,
                    CoreEvent::InputFailed {
                        message: "the operating system rejected the event",
                        ..
                    }
                )
            })
    });
    controller
        .submit_input(
            session.id,
            session.generation,
            InputEvent::key_down(usage::A),
        )
        .unwrap();
    wait_seen(&applied, &[Seen::Event(InputEvent::key_down(usage::A))]);
    assert_eq!(
        host.snapshot().unwrap().incoming.unwrap().phase,
        SessionPhase::Active
    );
}

#[test]
fn a_dropped_session_does_not_replay_finished_text() {
    let host = start_direct();
    let controller = start_direct();
    let applied = Arc::new(Mutex::new(Vec::new()));
    host.set_desktop(Box::new(RecordingDesktop {
        seen: Arc::clone(&applied),
        fail_text: false,
    }))
    .unwrap();
    pair(&host, &controller);
    let session = controller.connect(&host.endpoint_id()).unwrap();
    controller
        .submit_text(session.id, session.generation, "one")
        .unwrap();
    wait_seen(&applied, &[Seen::Text("one".to_owned())]);
    controller.drop_link().unwrap();
    wait_until(|| {
        controller
            .snapshot()
            .unwrap()
            .outgoing
            .as_ref()
            .is_some_and(|outgoing| outgoing.phase != SessionPhase::Active)
    });
    assert!(matches!(
        controller.submit_text(session.id, session.generation, "replayed"),
        Err(Error::NoSession | Error::StaleSession)
    ));
    wait_until(|| {
        let outgoing = controller.snapshot().unwrap().outgoing.unwrap();
        outgoing.phase == SessionPhase::Active && outgoing.generation != session.generation
    });
    let again = controller.snapshot().unwrap().outgoing.unwrap();
    controller
        .submit_text(again.id, again.generation, "two")
        .unwrap();
    wait_seen(
        &applied,
        &[Seen::Text("one".to_owned()), Seen::Text("two".to_owned())],
    );
}
