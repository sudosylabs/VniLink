use crate::error::Error;
use crate::protocol::PROTOCOL_VERSION;
use crate::session::{admit, check_input, phase_after_admit, SessionPhase};

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
    assert!(check_input(Some((4, 2)), 4, 2).is_ok());
    assert!(matches!(
        check_input(Some((4, 3)), 4, 2),
        Err(Error::StaleSession)
    ));
    assert!(matches!(check_input(None, 4, 2), Err(Error::StaleSession)));
    assert!(matches!(
        check_input(Some((4, 2)), 9, 2),
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
