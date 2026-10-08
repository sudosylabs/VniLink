use crate::error::Error;
use crate::events::{CoreEvent, EventHub};
use crate::protocol::SessionStamp;
use crate::protocol::{
    Quality, CODEC_H264, CODEC_H264_HIGH, MIN_PROTOCOL_VERSION, PROTOCOL_VERSION,
};
use crate::session::{
    admit, agree_quality, announce_progression, check_input, controller_offer, negotiate_codec,
    phase_after_admit, SessionPhase, SessionRole,
};

#[test]
fn automatic_quality_can_reach_retina_detail_and_handles_clamped_monitors() {
    let clamped = Quality::new(5_000, 30, 800, 600).unwrap();
    assert_eq!(
        crate::session::suggest_quality(&clamped, 10, false, 0, 0),
        Some(Quality::new(12_000, 60, 2560, 1440).unwrap())
    );
    let high = Quality::new(12_000, 60, 2560, 1440).unwrap();
    let best = Quality::new(24_000, 60, 3840, 2160).unwrap();
    assert_eq!(
        crate::session::suggest_quality(&high, 10, false, 0, 0),
        Some(best)
    );
    assert_eq!(
        crate::session::suggest_quality(&best, 10, false, 0, 0),
        None
    );
    assert_eq!(
        crate::session::suggest_quality(&best, 180, false, 6, 1),
        Some(high)
    );
}

#[test]
fn admission_checks_hosting_version_capacity_and_grant() {
    assert!(matches!(
        admit(
            false,
            false,
            true,
            false,
            false,
            PROTOCOL_VERSION,
            false,
            true,
            false
        ),
        Err(Error::NotHosting)
    ));
    assert!(matches!(
        admit(
            true,
            false,
            true,
            false,
            false,
            PROTOCOL_VERSION + 1,
            false,
            true,
            false
        ),
        Err(Error::IncompatibleVersion)
    ));
    assert!(matches!(
        admit(
            true,
            false,
            true,
            false,
            false,
            MIN_PROTOCOL_VERSION - 1,
            false,
            true,
            false
        ),
        Err(Error::IncompatibleVersion)
    ));
    assert!(admit(
        true,
        false,
        true,
        false,
        false,
        MIN_PROTOCOL_VERSION,
        false,
        true,
        false
    )
    .is_ok());
    assert!(matches!(
        admit(
            true,
            true,
            true,
            false,
            false,
            PROTOCOL_VERSION,
            false,
            true,
            false
        ),
        Err(Error::Busy)
    ));
    assert!(matches!(
        admit(
            true,
            false,
            false,
            false,
            false,
            PROTOCOL_VERSION,
            false,
            true,
            false
        ),
        Err(Error::Unauthorized)
    ));
    // Files-only: Transfer without Control is admitted without video checks.
    assert!(admit(
        true,
        false,
        false,
        true,
        true,
        PROTOCOL_VERSION,
        true,
        false,
        true
    )
    .is_ok());
    assert!(matches!(
        admit(
            true,
            false,
            true,
            false,
            false,
            PROTOCOL_VERSION,
            true,
            true,
            false
        ),
        Err(Error::Unsupported(_))
    ));
    assert!(matches!(
        admit(
            true,
            false,
            true,
            false,
            false,
            PROTOCOL_VERSION,
            false,
            true,
            true
        ),
        Err(Error::Unsupported(_))
    ));
    assert!(matches!(
        admit(
            true,
            false,
            true,
            false,
            false,
            PROTOCOL_VERSION,
            false,
            false,
            false
        ),
        Err(Error::IncompatibleVersion)
    ));
    assert!(admit(
        true,
        false,
        true,
        false,
        false,
        PROTOCOL_VERSION,
        false,
        true,
        false
    )
    .is_ok());
}

#[test]
fn codec_negotiation_prefers_high_and_falls_back_to_baseline() {
    assert_eq!(negotiate_codec(CODEC_H264, false), Some(CODEC_H264));
    assert_eq!(negotiate_codec(CODEC_H264, true), Some(CODEC_H264));
    assert_eq!(
        negotiate_codec(CODEC_H264_HIGH, true),
        Some(CODEC_H264_HIGH)
    );
    // Silent safe downgrade: every High decoder also decodes Baseline.
    assert_eq!(negotiate_codec(CODEC_H264_HIGH, false), Some(CODEC_H264));
    assert_eq!(negotiate_codec(9, true), None);
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
        admit(false, false, true, false, false, 0, false, true, false),
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

#[test]
fn neutral_admission_needs_pairing_host_enablement_and_v5_only() {
    use crate::session::admit_services;
    assert!(admit_services(true, false, true, 5).is_ok());
    assert!(matches!(
        admit_services(false, false, true, 5),
        Err(Error::NotHosting)
    ));
    assert!(matches!(
        admit_services(true, false, false, 5),
        Err(Error::NotPaired)
    ));
    assert!(matches!(
        admit_services(true, true, true, 5),
        Err(Error::Busy)
    ));
    for version in [1, 2, 3, 4, PROTOCOL_VERSION + 1] {
        assert!(matches!(
            admit_services(true, false, true, version),
            Err(Error::IncompatibleVersion)
        ));
    }
}
