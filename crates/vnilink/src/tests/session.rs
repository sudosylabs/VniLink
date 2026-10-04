use crate::error::Error;
use crate::events::{CoreEvent, EventHub};
use crate::protocol::SessionStamp;
use crate::protocol::PROTOCOL_VERSION;
use crate::session::{
    admit, announce_progression, check_input, phase_after_admit, SessionPhase, SessionRole,
};

#[test]
fn admission_checks_hosting_version_capacity_and_grant() {
    assert!(matches!(
        admit(false, false, true, PROTOCOL_VERSION),
        Err(Error::NotHosting)
    ));
    assert!(matches!(
        admit(true, false, true, PROTOCOL_VERSION + 1),
        Err(Error::IncompatibleVersion)
    ));
    assert!(matches!(
        admit(true, true, true, PROTOCOL_VERSION),
        Err(Error::Busy)
    ));
    assert!(matches!(
        admit(true, false, false, PROTOCOL_VERSION),
        Err(Error::Unauthorized)
    ));
    assert!(admit(true, false, true, PROTOCOL_VERSION).is_ok());
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
        admit(false, false, true, 0),
        Err(Error::NotHosting)
    ));
    assert_eq!(phase_after_admit(), SessionPhase::Active);
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
