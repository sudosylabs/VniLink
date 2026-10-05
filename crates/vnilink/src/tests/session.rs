use crate::error::Error;
use crate::events::{CoreEvent, EventHub};
use crate::protocol::SessionStamp;
use crate::protocol::{Quality, PROTOCOL_VERSION};
use crate::session::{
    admit, agree_quality, announce_progression, check_input, controller_offer, phase_after_admit,
    SessionPhase, SessionRole,
};

#[test]
fn admission_checks_hosting_version_capacity_and_grant() {
    assert!(matches!(
        admit(false, false, true, PROTOCOL_VERSION, false, true, false),
        Err(Error::NotHosting)
    ));
    assert!(matches!(
        admit(true, false, true, PROTOCOL_VERSION + 1, false, true, false),
        Err(Error::IncompatibleVersion)
    ));
    assert!(matches!(
        admit(true, true, true, PROTOCOL_VERSION, false, true, false),
        Err(Error::Busy)
    ));
    assert!(matches!(
        admit(true, false, false, PROTOCOL_VERSION, false, true, false),
        Err(Error::Unauthorized)
    ));
    assert!(matches!(
        admit(true, false, true, PROTOCOL_VERSION, true, true, false),
        Err(Error::Unsupported(_))
    ));
    assert!(matches!(
        admit(true, false, true, PROTOCOL_VERSION, false, true, true),
        Err(Error::Unsupported(_))
    ));
    assert!(matches!(
        admit(true, false, true, PROTOCOL_VERSION, false, false, false),
        Err(Error::IncompatibleVersion)
    ));
    assert!(admit(true, false, true, PROTOCOL_VERSION, false, true, false).is_ok());
}

#[test]
fn input_from_an_old_generation_is_rejected() {
    let current = SessionStamp::new(4, 2);
    assert!(check_input(Some(current), current).is_ok());
    assert!(matches!(
        check_input(Some(SessionStamp::new(4, 3)), current),
        Err(Error::StaleSession)
    ));
    assert!(matches!(
        check_input(None, current),
        Err(Error::StaleSession)
    ));
    assert!(matches!(
        check_input(Some(current), SessionStamp::new(9, 2)),
        Err(Error::StaleSession)
    ));
}

#[test]
fn admission_order_prefers_hosting_and_the_control_channel_is_active() {
    assert!(matches!(
        admit(false, false, true, 0, false, true, false),
        Err(Error::NotHosting)
    ));
    assert_eq!(phase_after_admit(), SessionPhase::Active);
}

#[test]
fn the_offer_clamps_to_the_host_monitor() {
    let offer = controller_offer();
    assert_eq!(
        agree_quality(&offer, (800, 600)),
        Quality::new(2_000, 30, 800, 600).unwrap()
    );
    assert_eq!(agree_quality(&offer, (0, 0)), offer);
    assert_eq!(agree_quality(&offer, (4000, 3000)), offer);
}

#[test]
fn progression_emits_the_four_phases_in_order() {
    let hub = EventHub::new(8);
    let sub = hub.subscribe().unwrap();
    announce_progression(&hub, 7, 3, SessionRole::Host, "peer");
    let mut phases = Vec::new();
    while let Some(CoreEvent::SessionChanged { phase, .. }) = sub.try_recv() {
        phases.push(phase);
    }
    assert_eq!(
        phases,
        [
            SessionPhase::Authenticated,
            SessionPhase::Authorized,
            SessionPhase::Negotiating,
            SessionPhase::Active,
        ]
    );
}
