use std::str::FromStr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use iroh::{PublicKey, RelayMode};

use crate::core::{Core, CoreConfig, Listen};
use crate::error::Error;
use crate::events::{CoreEvent, EventSubscription};
use crate::input::{HostDesktop, InputEvent};
use crate::invitation;
use crate::protocol::Quality;
use crate::relay::RelaySettings;
use crate::session::{Grants, SessionPhase, SessionRole};

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

fn wait_closed(core: &Core, role: SessionRole) {
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

struct RecordingDesktop {
    events: Arc<Mutex<Vec<InputEvent>>>,
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

struct FixedDesktop {
    width: u16,
    height: u16,
    events: Arc<Mutex<Vec<InputEvent>>>,
    fail_key_down: Option<u16>,
    fail_release: bool,
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

fn pair_for_control(host: &Core, controller: &Core) {
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

fn take_matching(
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

fn boot_capacity(event_capacity: usize) -> Result<Core, Error> {
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

fn wait_applied(events: &Mutex<Vec<InputEvent>>, expected: &[InputEvent]) {
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

#[test]
fn pairing_does_not_grant_control_and_a_grant_is_directional() {
    let host = start_direct();
    let controller = start_direct();
    host.start_hosting().unwrap();
    let token = host.create_invitation(Duration::from_secs(60)).unwrap();
    let paired = controller.accept_invitation(&token).unwrap();
    assert!(!paired.grants.control);
    assert!(matches!(
        controller.accept_invitation(&token),
        Err(Error::InvitationReplayed)
    ));
    let controller_token = controller
        .create_invitation(Duration::from_secs(60))
        .unwrap();
    host.accept_invitation(&controller_token).unwrap();
    let denied = controller.connect(&host.endpoint_id());
    assert!(
        matches!(denied, Err(Error::Unauthorized)),
        "connect before a grant returned {denied:?}; host={:?}; controller={:?}",
        host.snapshot(),
        controller.snapshot()
    );

    controller.start_hosting().unwrap();
    assert!(matches!(
        host.connect(&controller.endpoint_id()),
        Err(Error::Unauthorized)
    ));

    host.authorize(
        &controller.endpoint_id(),
        Grants {
            control: false,
            clipboard: true,
            transfer: false,
        },
    )
    .unwrap();
    assert!(matches!(
        controller.connect(&host.endpoint_id()),
        Err(Error::Unauthorized)
    ));

    host.authorize(
        &controller.endpoint_id(),
        Grants {
            control: true,
            clipboard: false,
            transfer: false,
        },
    )
    .unwrap();
    let session = controller.connect(&host.endpoint_id()).unwrap();
    assert_eq!(session.phase, SessionPhase::Active);
    controller
        .submit_input(
            session.id,
            session.generation,
            InputEvent::key_down(crate::input::usage::A),
        )
        .unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    loop {
        match controller.submit_input(
            session.id,
            session.generation,
            InputEvent::key_up(crate::input::usage::A),
        ) {
            Err(Error::Unsupported(_)) => break,
            Ok(()) => {}
            Err(error) => panic!("input without a desktop adapter returned {error}"),
        }
        if std::time::Instant::now() > deadline {
            panic!("host did not refuse input while no desktop adapter was installed");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let quality = Quality::new(2_000, 30, 1920, 1080).unwrap();
    host.update_session_quality(session.id, quality).unwrap();
    assert_eq!(
        host.snapshot().unwrap().incoming.unwrap().quality,
        Some(quality)
    );

    host.stop_hosted_session().unwrap();
    wait_closed(&host, SessionRole::Host);
    wait_closed(&controller, SessionRole::Controller);
    assert!(matches!(
        controller.submit_input(
            session.id,
            session.generation,
            InputEvent::key_down(crate::input::usage::A),
        ),
        Err(Error::NoSession | Error::StaleSession)
    ));

    host.revoke(&controller.endpoint_id()).unwrap();
    assert!(matches!(
        controller.connect(&host.endpoint_id()),
        Err(Error::Unauthorized)
    ));
    assert!(!host.capabilities().desktop_capture);
    assert!(!host.capabilities().keyboard);
}

#[test]
fn keyboard_and_pointer_events_are_applied_and_released() {
    let host = start_direct();
    let controller = start_direct();
    let applied = Arc::new(Mutex::new(Vec::new()));
    host.set_desktop(Box::new(RecordingDesktop {
        events: Arc::clone(&applied),
    }))
    .unwrap();
    assert!(host.capabilities().keyboard);
    assert!(host.capabilities().pointer);
    assert_eq!(host.capabilities().monitors, 1);
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
    let session = controller.connect(&host.endpoint_id()).unwrap();
    assert_eq!(session.monitor_width, 1920);
    assert_eq!(session.monitor_height, 1080);
    for event in [
        InputEvent::key_down(crate::input::usage::A),
        InputEvent::pointer(1, 1),
        InputEvent::pointer(4, 5),
        InputEvent::pointer(9, 8),
        InputEvent::button_down(crate::input::PointerButton::Left),
        InputEvent::wheel(0, -1),
    ] {
        controller
            .submit_input(session.id, session.generation, event)
            .unwrap();
    }
    wait_applied(
        &applied,
        &[
            InputEvent::key_down(crate::input::usage::A),
            InputEvent::pointer(9, 8),
            InputEvent::button_down(crate::input::PointerButton::Left),
            InputEvent::wheel(0, -1),
        ],
    );
    host.stop_hosted_session().unwrap();
    wait_applied(
        &applied,
        &[
            InputEvent::key_down(crate::input::usage::A),
            InputEvent::pointer(9, 8),
            InputEvent::button_down(crate::input::PointerButton::Left),
            InputEvent::wheel(0, -1),
            InputEvent::button_up(crate::input::PointerButton::Left),
            InputEvent::key_up(crate::input::usage::A),
        ],
    );
}

#[test]
fn a_second_controller_is_rejected_while_a_session_is_active() {
    let host = start_direct();
    let first = start_direct();
    let second = start_direct();
    host.start_hosting().unwrap();
    for controller in [&first, &second] {
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
    let session = first.connect(&host.endpoint_id()).unwrap();
    assert_eq!(session.phase, SessionPhase::Active);
    let busy = second.connect(&host.endpoint_id());
    assert!(
        matches!(busy, Err(Error::Busy)),
        "second connect returned {busy:?}; host={:?}",
        host.snapshot()
    );
}

#[test]
fn a_fresh_loopback_core_starts_idle() {
    let core = start_direct();
    let snapshot = core.snapshot().unwrap();
    assert_eq!(snapshot.endpoint_id, core.endpoint_id());
    assert!(snapshot.public_relay);
    assert!(snapshot.relay_urls.is_empty());
    assert!(!snapshot.hosting);
    assert!(snapshot.devices.is_empty());
    assert!(snapshot.incoming.is_none());
    assert!(snapshot.outgoing.is_none());
    assert!(!snapshot.capabilities.keyboard);
    assert!(!snapshot.capabilities.pointer);
    assert!(!snapshot.capabilities.desktop_capture);
    assert_eq!(snapshot.capabilities.monitors, 0);
}

#[test]
fn shutdown_is_idempotent_and_later_calls_report_it() {
    let core = start_direct();
    let other = start_direct();
    let token = other.create_invitation(Duration::from_secs(60)).unwrap();
    let id = core.endpoint_id();
    let secret = core.export_identity();
    core.shutdown();
    core.shutdown();
    assert_eq!(core.endpoint_id(), id);
    assert_eq!(core.export_identity(), secret);
    assert!(matches!(core.snapshot(), Err(Error::ShutDown)));
    assert!(matches!(core.subscribe(), Err(Error::ShutDown)));
    assert!(matches!(core.start_hosting(), Err(Error::ShutDown)));
    assert!(matches!(core.stop_hosting(), Err(Error::ShutDown)));
    assert!(matches!(core.stop_hosted_session(), Err(Error::ShutDown)));
    assert!(matches!(
        core.create_invitation(Duration::from_secs(60)),
        Err(Error::ShutDown)
    ));
    assert!(matches!(
        core.accept_invitation(&token),
        Err(Error::ShutDown)
    ));
    assert!(matches!(
        core.connect(&other.endpoint_id()),
        Err(Error::ShutDown)
    ));
    assert!(matches!(core.disconnect(), Err(Error::ShutDown)));
    assert!(matches!(
        core.submit_input(1, 1, InputEvent::pointer(0, 0)),
        Err(Error::ShutDown)
    ));
    assert!(matches!(
        core.set_desktop(Box::new(FixedDesktop {
            width: 0,
            height: 0,
            events: Arc::new(Mutex::new(Vec::new())),
            fail_key_down: None,
            fail_release: false,
        })),
        Err(Error::ShutDown)
    ));
    assert!(matches!(
        core.authorize(&other.endpoint_id(), Grants::default()),
        Err(Error::ShutDown)
    ));
    assert!(matches!(
        core.revoke(&other.endpoint_id()),
        Err(Error::ShutDown)
    ));
    assert!(matches!(
        core.update_session_quality(1, Quality::new(1, 1, 1, 1).unwrap()),
        Err(Error::ShutDown)
    ));
}

#[test]
fn event_capacity_must_be_between_1_and_1024() {
    assert!(matches!(
        boot_capacity(0),
        Err(Error::InvalidSetting("event capacity must be at least 1"))
    ));
    assert!(matches!(
        boot_capacity(1025),
        Err(Error::InvalidSetting("event capacity must be at most 1024"))
    ));
    let core = boot_capacity(1).unwrap();
    let events = core.subscribe().unwrap();
    core.start_hosting().unwrap();
    core.stop_hosting().unwrap();
    assert!(!core.snapshot().unwrap().hosting);
    assert_eq!(events.lagged(), 1);
    assert_eq!(
        events.try_recv(),
        Some(CoreEvent::HostingChanged { hosting: true })
    );
    assert!(boot_capacity(1024).is_ok());
}

#[test]
fn restored_identity_keeps_the_same_endpoint_id() {
    let original = start_direct();
    let bytes = original.export_identity();
    let id = original.endpoint_id();
    drop(original);
    let restored = Core::boot(
        CoreConfig {
            identity: Some(bytes),
            listen: Listen::Loopback,
            ..CoreConfig::default()
        },
        RelayMode::Disabled,
        true,
    )
    .unwrap();
    assert_eq!(restored.export_identity(), bytes);
    assert_eq!(restored.endpoint_id(), id);
    assert_ne!(start_direct().endpoint_id(), id);
    let rendered = format!(
        "{:?}",
        CoreConfig {
            identity: Some([0x11; 32]),
            ..CoreConfig::default()
        }
    );
    assert!(rendered.contains("[redacted]"));
    assert!(!rendered.contains("17"));
}

#[test]
fn an_installation_cannot_pair_with_itself() {
    let core = start_direct();
    let token = core.create_invitation(Duration::from_secs(60)).unwrap();
    assert!(matches!(
        core.accept_invitation(&token),
        Err(Error::InvalidInvitation(
            "an installation cannot pair with itself"
        ))
    ));
}

#[test]
fn unknown_devices_and_garbage_ids_are_rejected() {
    let host = start_direct();
    let stranger = start_direct();
    let control = Grants {
        control: true,
        clipboard: false,
        transfer: false,
    };
    assert!(matches!(
        host.authorize(&stranger.endpoint_id(), control),
        Err(Error::NotPaired)
    ));
    assert!(matches!(
        host.revoke(&stranger.endpoint_id()),
        Err(Error::NotPaired)
    ));
    for id in ["", "not-a-key", "zzzz"] {
        assert!(
            matches!(
                host.authorize(id, Grants::default()),
                Err(Error::InvalidSetting(_))
            ),
            "{id}"
        );
        assert!(
            matches!(host.connect(id), Err(Error::InvalidSetting(_))),
            "{id}"
        );
        assert!(
            matches!(host.revoke(id), Err(Error::InvalidSetting(_))),
            "{id}"
        );
    }
}

#[test]
fn invitation_lifetimes_and_malformed_tokens_are_rejected() {
    let host = start_direct();
    let controller = start_direct();
    assert!(matches!(
        host.create_invitation(Duration::ZERO),
        Err(Error::InvalidInvitation(_))
    ));
    assert!(matches!(
        host.create_invitation(Duration::from_secs(15 * 60 + 1)),
        Err(Error::InvalidInvitation(_))
    ));
    assert!(host.create_invitation(Duration::from_secs(15 * 60)).is_ok());
    for token in ["", "nope", "vnl1.zz"] {
        assert!(
            matches!(
                controller.accept_invitation(token),
                Err(Error::InvalidInvitation(_))
            ),
            "{token}"
        );
    }
}

#[test]
fn accepting_a_new_invitation_resets_grants() {
    let host = start_direct();
    let controller = start_direct();
    host.start_hosting().unwrap();
    controller
        .accept_invitation(&host.create_invitation(Duration::from_secs(60)).unwrap())
        .unwrap();
    host.accept_invitation(
        &controller
            .create_invitation(Duration::from_secs(60))
            .unwrap(),
    )
    .unwrap();
    host.authorize(
        &controller.endpoint_id(),
        Grants {
            control: true,
            clipboard: true,
            transfer: false,
        },
    )
    .unwrap();
    let info = host
        .accept_invitation(
            &controller
                .create_invitation(Duration::from_secs(60))
                .unwrap(),
        )
        .unwrap();
    assert!(!info.grants.control);
    assert!(!info.grants.clipboard);
    assert!(!info.grants.transfer);
    let device = host
        .snapshot()
        .unwrap()
        .devices
        .into_iter()
        .find(|device| device.endpoint_id == controller.endpoint_id())
        .unwrap();
    assert!(!device.grants.control);
    assert!(matches!(
        controller.connect(&host.endpoint_id()),
        Err(Error::Unauthorized)
    ));
}

#[test]
fn connect_requires_pairing_and_hosting() {
    let host = start_direct();
    let controller = start_direct();
    assert!(!host.capabilities().keyboard);
    assert_eq!(host.capabilities().monitors, 0);
    assert!(matches!(
        controller.connect(&host.endpoint_id()),
        Err(Error::NotPaired)
    ));
    controller
        .accept_invitation(&host.create_invitation(Duration::from_secs(60)).unwrap())
        .unwrap();
    host.accept_invitation(
        &controller
            .create_invitation(Duration::from_secs(60))
            .unwrap(),
    )
    .unwrap();
    host.authorize(
        &controller.endpoint_id(),
        Grants {
            control: true,
            clipboard: false,
            transfer: false,
        },
    )
    .unwrap();
    assert!(matches!(
        controller.connect(&host.endpoint_id()),
        Err(Error::NotHosting)
    ));
    assert!(matches!(
        controller.connect(&host.endpoint_id()),
        Err(Error::NotHosting)
    ));
    host.start_hosting().unwrap();
    let session = controller.connect(&host.endpoint_id()).unwrap();
    assert_eq!(session.phase, SessionPhase::Active);
    assert_eq!(session.monitor_width, 0);
    assert_eq!(session.monitor_height, 0);
    assert!(matches!(
        controller.connect(&host.endpoint_id()),
        Err(Error::Busy)
    ));
}

#[test]
fn input_for_the_wrong_session_or_generation_is_rejected() {
    let host = start_direct();
    let controller = start_direct();
    pair_for_control(&host, &controller);
    let session = controller.connect(&host.endpoint_id()).unwrap();
    assert!(matches!(
        controller.submit_input(
            session.id + 1,
            session.generation,
            InputEvent::key_down(crate::input::usage::A),
        ),
        Err(Error::NoSession)
    ));
    assert!(matches!(
        controller.submit_input(
            session.id,
            session.generation + 1,
            InputEvent::key_down(crate::input::usage::A),
        ),
        Err(Error::StaleSession)
    ));
    controller
        .submit_input(
            session.id,
            session.generation,
            InputEvent::key_down(crate::input::usage::A),
        )
        .unwrap();
    controller.disconnect().unwrap();
    assert!(matches!(
        controller.submit_input(
            session.id,
            session.generation,
            InputEvent::key_up(crate::input::usage::A),
        ),
        Err(Error::NoSession)
    ));
}

#[test]
fn the_desktop_adapter_cannot_change_during_a_session() {
    let host = start_direct();
    let controller = start_direct();
    let applied = Arc::new(Mutex::new(Vec::new()));
    host.set_desktop(Box::new(FixedDesktop {
        width: 1920,
        height: 1080,
        events: Arc::clone(&applied),
        fail_key_down: None,
        fail_release: false,
    }))
    .unwrap();
    assert_eq!(host.capabilities().monitors, 1);
    pair_for_control(&host, &controller);
    let session = controller.connect(&host.endpoint_id()).unwrap();
    assert_eq!(session.monitor_width, 1920);
    assert_eq!(session.monitor_height, 1080);
    assert!(matches!(
        host.set_desktop(Box::new(FixedDesktop {
            width: 0,
            height: 0,
            events: Arc::clone(&applied),
            fail_key_down: None,
            fail_release: false,
        })),
        Err(Error::Busy)
    ));
    host.stop_hosted_session().unwrap();
    wait_closed(&host, SessionRole::Host);
    host.set_desktop(Box::new(FixedDesktop {
        width: 100,
        height: 0,
        events: applied,
        fail_key_down: None,
        fail_release: false,
    }))
    .unwrap();
    assert!(host.capabilities().keyboard);
    assert!(host.capabilities().pointer);
    assert!(!host.capabilities().desktop_capture);
    assert_eq!(host.capabilities().monitors, 0);
}

#[test]
fn stopping_hosting_disables_new_sessions_even_when_none_is_active() {
    let idle = start_direct();
    idle.stop_hosting().unwrap();
    assert!(!idle.snapshot().unwrap().hosting);

    let host = start_direct();
    let controller = start_direct();
    pair_for_control(&host, &controller);
    assert!(matches!(host.stop_hosted_session(), Err(Error::NoSession)));
    assert!(host.snapshot().unwrap().hosting);
    host.stop_hosting().unwrap();
    assert!(!host.snapshot().unwrap().hosting);
    assert!(matches!(
        controller.connect(&host.endpoint_id()),
        Err(Error::NotHosting)
    ));
    assert!(host.snapshot().unwrap().devices[0].grants.control);
}

#[test]
fn stopping_hosting_ends_the_active_session_and_releases_keys() {
    let host = start_direct();
    let controller = start_direct();
    let applied = Arc::new(Mutex::new(Vec::new()));
    host.set_desktop(Box::new(RecordingDesktop {
        events: Arc::clone(&applied),
    }))
    .unwrap();
    pair_for_control(&host, &controller);
    let session = controller.connect(&host.endpoint_id()).unwrap();
    controller
        .submit_input(
            session.id,
            session.generation,
            InputEvent::key_down(crate::input::usage::A),
        )
        .unwrap();
    controller
        .submit_input(
            session.id,
            session.generation,
            InputEvent::button_down(crate::input::PointerButton::Left),
        )
        .unwrap();
    wait_applied(
        &applied,
        &[
            InputEvent::key_down(crate::input::usage::A),
            InputEvent::button_down(crate::input::PointerButton::Left),
        ],
    );
    host.stop_hosting().unwrap();
    wait_applied(
        &applied,
        &[
            InputEvent::key_down(crate::input::usage::A),
            InputEvent::button_down(crate::input::PointerButton::Left),
            InputEvent::button_up(crate::input::PointerButton::Left),
            InputEvent::key_up(crate::input::usage::A),
        ],
    );
    wait_closed(&host, SessionRole::Host);
    wait_closed(&controller, SessionRole::Controller);
    assert!(!host.snapshot().unwrap().hosting);
    assert!(host.snapshot().unwrap().devices[0].grants.control);
    assert!(matches!(
        controller.connect(&host.endpoint_id()),
        Err(Error::NotHosting)
    ));
    host.start_hosting().unwrap();
    let again = controller.connect(&host.endpoint_id()).unwrap();
    assert_eq!(again.phase, SessionPhase::Active);
}

#[test]
fn disconnect_closes_both_sides_and_releases_held_input() {
    let host = start_direct();
    let controller = start_direct();
    let applied = Arc::new(Mutex::new(Vec::new()));
    host.set_desktop(Box::new(RecordingDesktop {
        events: Arc::clone(&applied),
    }))
    .unwrap();
    pair_for_control(&host, &controller);
    let session = controller.connect(&host.endpoint_id()).unwrap();
    controller
        .submit_input(
            session.id,
            session.generation,
            InputEvent::key_down(crate::input::usage::A),
        )
        .unwrap();
    controller
        .submit_input(
            session.id,
            session.generation,
            InputEvent::button_down(crate::input::PointerButton::Left),
        )
        .unwrap();
    wait_applied(
        &applied,
        &[
            InputEvent::key_down(crate::input::usage::A),
            InputEvent::button_down(crate::input::PointerButton::Left),
        ],
    );
    controller.disconnect().unwrap();
    wait_closed(&host, SessionRole::Host);
    wait_closed(&controller, SessionRole::Controller);
    wait_applied(
        &applied,
        &[
            InputEvent::key_down(crate::input::usage::A),
            InputEvent::button_down(crate::input::PointerButton::Left),
            InputEvent::button_up(crate::input::PointerButton::Left),
            InputEvent::key_up(crate::input::usage::A),
        ],
    );
    assert!(host.snapshot().unwrap().hosting);
    assert!(matches!(
        controller.submit_input(
            session.id,
            session.generation,
            InputEvent::key_down(crate::input::usage::A),
        ),
        Err(Error::NoSession)
    ));
    assert!(matches!(controller.disconnect(), Err(Error::NoSession)));
    let again = controller.connect(&host.endpoint_id()).unwrap();
    assert_eq!(again.phase, SessionPhase::Active);
}

#[test]
fn revoke_stops_the_session_releases_input_and_blocks_reconnect() {
    let host = start_direct();
    let controller = start_direct();
    let applied = Arc::new(Mutex::new(Vec::new()));
    host.set_desktop(Box::new(RecordingDesktop {
        events: Arc::clone(&applied),
    }))
    .unwrap();
    pair_for_control(&host, &controller);
    let session = controller.connect(&host.endpoint_id()).unwrap();
    controller
        .submit_input(
            session.id,
            session.generation,
            InputEvent::key_down(crate::input::usage::LEFT_SHIFT),
        )
        .unwrap();
    controller
        .submit_input(
            session.id,
            session.generation,
            InputEvent::button_down(crate::input::PointerButton::Right),
        )
        .unwrap();
    wait_applied(
        &applied,
        &[
            InputEvent::key_down(crate::input::usage::LEFT_SHIFT),
            InputEvent::button_down(crate::input::PointerButton::Right),
        ],
    );
    host.revoke(&controller.endpoint_id()).unwrap();
    wait_applied(
        &applied,
        &[
            InputEvent::key_down(crate::input::usage::LEFT_SHIFT),
            InputEvent::button_down(crate::input::PointerButton::Right),
            InputEvent::button_up(crate::input::PointerButton::Right),
            InputEvent::key_up(crate::input::usage::LEFT_SHIFT),
        ],
    );
    wait_closed(&host, SessionRole::Host);
    wait_closed(&controller, SessionRole::Controller);
    let devices = host.snapshot().unwrap().devices;
    assert_eq!(devices.len(), 1);
    assert_eq!(devices[0].endpoint_id, controller.endpoint_id());
    assert!(!devices[0].grants.control);
    assert!(matches!(
        controller.connect(&host.endpoint_id()),
        Err(Error::Unauthorized)
    ));
    assert!(matches!(
        controller.submit_input(
            session.id,
            session.generation,
            InputEvent::key_down(crate::input::usage::A),
        ),
        Err(Error::NoSession)
    ));
}

#[test]
fn key_up_and_each_button_are_applied_and_not_released_again() {
    let host = start_direct();
    let controller = start_direct();
    let applied = Arc::new(Mutex::new(Vec::new()));
    host.set_desktop(Box::new(RecordingDesktop {
        events: Arc::clone(&applied),
    }))
    .unwrap();
    pair_for_control(&host, &controller);
    let session = controller.connect(&host.endpoint_id()).unwrap();
    let expected = [
        InputEvent::key_down(crate::input::usage::A),
        InputEvent::key_up(crate::input::usage::A),
        InputEvent::button_down(crate::input::PointerButton::Right),
        InputEvent::button_up(crate::input::PointerButton::Right),
        InputEvent::button_down(crate::input::PointerButton::Middle),
        InputEvent::button_up(crate::input::PointerButton::Middle),
        InputEvent::wheel(2, -3),
    ];
    for event in expected {
        controller
            .submit_input(session.id, session.generation, event)
            .unwrap();
    }
    wait_applied(&applied, &expected);
    host.stop_hosted_session().unwrap();
    wait_closed(&host, SessionRole::Host);
    wait_closed(&controller, SessionRole::Controller);
    let got = applied.lock().unwrap_or_else(|poison| poison.into_inner());
    assert_eq!(got.as_slice(), expected.as_slice());
}

#[test]
fn an_apply_failure_is_reported_and_the_session_stays_open() {
    let host = start_direct();
    let controller = start_direct();
    let applied = Arc::new(Mutex::new(Vec::new()));
    host.set_desktop(Box::new(FixedDesktop {
        width: 640,
        height: 480,
        events: Arc::clone(&applied),
        fail_key_down: Some(crate::input::usage::B),
        fail_release: false,
    }))
    .unwrap();
    let events = host.subscribe().unwrap();
    pair_for_control(&host, &controller);
    let session = controller.connect(&host.endpoint_id()).unwrap();
    assert_eq!(session.monitor_width, 640);
    assert_eq!(session.monitor_height, 480);
    controller
        .submit_input(
            session.id,
            session.generation,
            InputEvent::key_down(crate::input::usage::B),
        )
        .unwrap();
    assert_eq!(
        take_matching(&events, |event| {
            matches!(event, CoreEvent::InputFailed { .. })
        }),
        CoreEvent::InputFailed {
            id: session.id,
            message: "the operating system rejected the event",
        }
    );
    controller
        .submit_input(
            session.id,
            session.generation,
            InputEvent::key_down(crate::input::usage::A),
        )
        .unwrap();
    controller
        .submit_input(session.id, session.generation, InputEvent::pointer(3, 4))
        .unwrap();
    wait_applied(
        &applied,
        &[
            InputEvent::key_down(crate::input::usage::A),
            InputEvent::pointer(3, 4),
        ],
    );
    assert_eq!(
        host.snapshot().unwrap().incoming.unwrap().phase,
        SessionPhase::Active
    );
    controller
        .submit_input(
            session.id,
            session.generation,
            InputEvent::key_up(crate::input::usage::A),
        )
        .unwrap();
}

#[test]
fn a_failed_release_is_reported_on_the_hosted_session() {
    let host = start_direct();
    let controller = start_direct();
    let applied = Arc::new(Mutex::new(Vec::new()));
    host.set_desktop(Box::new(FixedDesktop {
        width: 800,
        height: 600,
        events: Arc::clone(&applied),
        fail_key_down: None,
        fail_release: true,
    }))
    .unwrap();
    let events = host.subscribe().unwrap();
    pair_for_control(&host, &controller);
    let session = controller.connect(&host.endpoint_id()).unwrap();
    controller
        .submit_input(
            session.id,
            session.generation,
            InputEvent::key_down(crate::input::usage::A),
        )
        .unwrap();
    controller
        .submit_input(
            session.id,
            session.generation,
            InputEvent::button_down(crate::input::PointerButton::Left),
        )
        .unwrap();
    wait_applied(
        &applied,
        &[
            InputEvent::key_down(crate::input::usage::A),
            InputEvent::button_down(crate::input::PointerButton::Left),
        ],
    );
    host.stop_hosted_session().unwrap();
    assert_eq!(
        take_matching(&events, |event| {
            matches!(event, CoreEvent::InputFailed { .. })
        }),
        CoreEvent::InputFailed {
            id: session.id,
            message: "a held key or button could not be released",
        }
    );
    wait_closed(&host, SessionRole::Host);
    let got = applied.lock().unwrap_or_else(|poison| poison.into_inner());
    assert_eq!(
        got.as_slice(),
        &[
            InputEvent::key_down(crate::input::usage::A),
            InputEvent::button_down(crate::input::PointerButton::Left),
        ]
    );
}

#[test]
fn quality_updates_are_local_to_each_side() {
    let host = start_direct();
    let controller = start_direct();
    let host_events = host.subscribe().unwrap();
    let controller_events = controller.subscribe().unwrap();
    pair_for_control(&host, &controller);
    let session = controller.connect(&host.endpoint_id()).unwrap();
    let quality = Quality::new(1_500, 30, 1280, 720).unwrap();
    controller
        .update_session_quality(session.id, quality)
        .unwrap();
    assert_eq!(
        controller.snapshot().unwrap().outgoing.unwrap().quality,
        Some(quality)
    );
    assert!(host.snapshot().unwrap().incoming.unwrap().quality.is_none());
    let host_quality = Quality::new(800, 15, 640, 480).unwrap();
    host.update_session_quality(session.id, host_quality)
        .unwrap();
    assert_eq!(
        host.snapshot().unwrap().incoming.unwrap().quality,
        Some(host_quality)
    );
    assert_eq!(
        controller.snapshot().unwrap().outgoing.unwrap().quality,
        Some(quality)
    );
    assert!(matches!(
        host.update_session_quality(session.id + 1, quality),
        Err(Error::NoSession)
    ));
    assert!(matches!(
        controller.update_session_quality(0, quality),
        Err(Error::NoSession)
    ));
    assert_eq!(
        take_matching(&host_events, |event| {
            matches!(event, CoreEvent::QualityUpdated { .. })
        }),
        CoreEvent::QualityUpdated {
            id: session.id,
            quality: host_quality,
        }
    );
    assert_eq!(
        take_matching(&controller_events, |event| {
            matches!(event, CoreEvent::QualityUpdated { .. })
        }),
        CoreEvent::QualityUpdated {
            id: session.id,
            quality,
        }
    );
}

#[test]
fn admission_publishes_the_session_lifecycle() {
    let host = start_direct();
    let controller = start_direct();
    let host_events = host.subscribe().unwrap();
    let controller_events = controller.subscribe().unwrap();
    host.start_hosting().unwrap();
    assert_eq!(
        take_matching(&host_events, |event| {
            matches!(event, CoreEvent::HostingChanged { .. })
        }),
        CoreEvent::HostingChanged { hosting: true }
    );
    controller
        .accept_invitation(&host.create_invitation(Duration::from_secs(60)).unwrap())
        .unwrap();
    assert!(matches!(
        take_matching(&controller_events, |event| {
            matches!(event, CoreEvent::DevicePaired { .. })
        }),
        CoreEvent::DevicePaired { .. }
    ));
    host.accept_invitation(
        &controller
            .create_invitation(Duration::from_secs(60))
            .unwrap(),
    )
    .unwrap();
    match take_matching(&host_events, |event| {
        matches!(event, CoreEvent::DevicePaired { .. })
    }) {
        CoreEvent::DevicePaired { endpoint_id } => {
            assert_eq!(endpoint_id, controller.endpoint_id());
        }
        other => panic!("paired event was {other:?}"),
    }
    host.authorize(
        &controller.endpoint_id(),
        Grants {
            control: true,
            clipboard: false,
            transfer: false,
        },
    )
    .unwrap();
    match take_matching(&host_events, |event| {
        matches!(event, CoreEvent::DeviceAuthorized { .. })
    }) {
        CoreEvent::DeviceAuthorized { endpoint_id } => {
            assert_eq!(endpoint_id, controller.endpoint_id());
        }
        other => panic!("authorized event was {other:?}"),
    }
    let session = controller.connect(&host.endpoint_id()).unwrap();
    let mut host_phases = Vec::new();
    while host_phases.last() != Some(&SessionPhase::Active) {
        match take_matching(&host_events, |event| {
            matches!(
                event,
                CoreEvent::SessionChanged {
                    role: SessionRole::Host,
                    ..
                }
            )
        }) {
            CoreEvent::SessionChanged {
                id,
                generation,
                phase,
                peer_id,
                ..
            } => {
                assert_eq!(id, session.id);
                assert_eq!(generation, session.generation);
                assert_eq!(peer_id, controller.endpoint_id());
                host_phases.push(phase);
            }
            other => panic!("host session event was {other:?}"),
        }
    }
    assert_eq!(
        host_phases,
        vec![
            SessionPhase::Authenticated,
            SessionPhase::Authorized,
            SessionPhase::Negotiating,
            SessionPhase::Active,
        ]
    );
    let mut controller_phases = Vec::new();
    while controller_phases.last() != Some(&SessionPhase::Active) {
        match take_matching(&controller_events, |event| {
            matches!(
                event,
                CoreEvent::SessionChanged {
                    role: SessionRole::Controller,
                    ..
                }
            )
        }) {
            CoreEvent::SessionChanged { phase, .. } => controller_phases.push(phase),
            other => panic!("controller session event was {other:?}"),
        }
    }
    assert_eq!(
        controller_phases,
        vec![
            SessionPhase::Connecting,
            SessionPhase::Authenticated,
            SessionPhase::Authorized,
            SessionPhase::Negotiating,
            SessionPhase::Active,
        ]
    );
    host.revoke(&controller.endpoint_id()).unwrap();
    match take_matching(&host_events, |event| {
        matches!(event, CoreEvent::DeviceRevoked { .. })
    }) {
        CoreEvent::DeviceRevoked { endpoint_id } => {
            assert_eq!(endpoint_id, controller.endpoint_id());
        }
        other => panic!("revoked event was {other:?}"),
    }
    wait_closed(&host, SessionRole::Host);
    wait_closed(&controller, SessionRole::Controller);
}

#[test]
fn a_loopback_invitation_carries_a_loopback_address() {
    let host = start_direct();
    let token = host.create_invitation(Duration::from_secs(60)).unwrap();
    let (opened, _) = invitation::open(&token, SystemTime::now()).unwrap();
    assert!(opened.relays.is_empty());
    assert_eq!(opened.direct.len(), 1);
    assert_eq!(
        opened.direct[0].ip(),
        std::net::IpAddr::V4(std::net::Ipv4Addr::new(127, 0, 0, 1))
    );
    assert_ne!(opened.direct[0].port(), 0);
    let key = PublicKey::from_str(&host.endpoint_id()).unwrap();
    assert_eq!(opened.endpoint_id, *key.as_bytes());
}

#[test]
fn a_custom_relay_url_is_advertised_without_contacting_it() {
    let relay = crate::relay::CustomRelay::new("https://relay.example.test")
        .unwrap()
        .with_auth_token("super-secret-token")
        .unwrap();
    let settings = RelaySettings::custom(vec![relay]).unwrap();
    let urls = settings.relay_urls().unwrap();
    let rendered = format!("{settings:?}");
    assert!(rendered.contains("[redacted]"));
    assert!(!rendered.contains("super-secret-token"));
    let host = Core::boot(
        CoreConfig {
            relay: settings,
            listen: Listen::Loopback,
            ..CoreConfig::default()
        },
        RelayMode::Disabled,
        true,
    )
    .unwrap();
    let snapshot = host.snapshot().unwrap();
    assert!(!snapshot.public_relay);
    assert_eq!(snapshot.relay_urls, urls);
    let token = host.create_invitation(Duration::from_secs(60)).unwrap();
    assert!(!token.contains("super-secret-token"));
    let (opened, _) = invitation::open(&token, SystemTime::now()).unwrap();
    assert_eq!(opened.relays, urls);
}
