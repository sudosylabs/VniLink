use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::error::Error;
use crate::events::CoreEvent;
use crate::input::InputEvent;
use crate::protocol::Quality;
use crate::session::{Grants, SessionPhase, SessionRole};

use super::support::*;

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
fn accepting_a_new_invitation_neither_adds_nor_removes_grants() {
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
            control: false,
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
    assert!(info.grants.clipboard);
    assert!(!info.grants.transfer);
    let device = host
        .snapshot()
        .unwrap()
        .devices
        .into_iter()
        .find(|device| device.endpoint_id == controller.endpoint_id())
        .unwrap();
    assert!(!device.grants.control);
    assert!(device.grants.clipboard);
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
