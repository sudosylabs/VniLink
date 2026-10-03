use vnilink_media::Error;

use crate::portal::{PortalPhase, PortalSession, ONE_PORTAL_SESSION_IS_UNIVERSAL};

#[test]
fn portal_phases_follow_the_screen_cast_sequence() {
    assert!(!ONE_PORTAL_SESSION_IS_UNIVERSAL);
    let mut session = PortalSession::new();
    assert_eq!(session.phase(), PortalPhase::Idle);
    assert!(matches!(
        session.select_monitor(),
        Err(Error::InvalidSetting(
            "portal sources were selected before the session existed"
        ))
    ));
    assert!(matches!(
        session.start(1),
        Err(Error::InvalidSetting(
            "portal stream started before a monitor was selected"
        ))
    ));
    assert!(matches!(
        session.close(),
        Err(Error::InvalidSetting("portal session is not open"))
    ));
    session.create_session().unwrap();
    assert!(matches!(
        session.create_session(),
        Err(Error::InvalidSetting("portal session was already created"))
    ));
    session.select_monitor().unwrap();
    session.start(7).unwrap();
    assert_eq!(session.phase(), PortalPhase::Started);
    assert_eq!(session.pipewire_node(), Some(7));
    session.close().unwrap();
    assert_eq!(session.phase(), PortalPhase::Closed);
    assert!(session.pipewire_node().is_none());
    assert!(matches!(
        session.start(7),
        Err(Error::InvalidSetting(
            "portal stream started before a monitor was selected"
        ))
    ));
}

#[test]
fn a_restore_token_is_kept_out_of_debug() {
    let mut session = PortalSession::new();
    assert!(matches!(
        session.set_restore_token(""),
        Err(Error::InvalidSetting("restore token is empty"))
    ));
    session.set_restore_token("secret-token").unwrap();
    assert_eq!(session.restore_token(), Some("secret-token"));
    let rendered = format!("{session:?}");
    assert!(rendered.contains("has_restore_token"));
    assert!(!rendered.contains("secret-token"));
}
