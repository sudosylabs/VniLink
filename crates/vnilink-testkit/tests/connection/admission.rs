//! Who a host admits: pairing alone admits a connection and grants
//! nothing, a grant runs one way, invitations are bounded and never widen
//! or narrow what was granted, a connection opens only to a hosting host,
//! a Desktop opens only between a host that can encode and a controller
//! that can decode.

use expect_test::expect;
use std::time::Duration;

use vnilink::{CoreEvent, Error, Grants, SessionPhase, SessionRole, VideoCodecCaps};
use vnilink_testkit::desktop::{InputLog, MonitorCapture};
use vnilink_testkit::events::next_event;
use vnilink_testkit::{traced, wait_for, Engine, TestResult, DEFAULT_WAIT, SEEN_AND_CONTROLLED};

use super::support::{introduce, refusal, INVITATION};

/// The monitor of the desktops these scenarios install.
const MONITOR: (u16, u16) = (1920, 1080);

/// Only clipboard: what a host shares that admits no session.
const CLIPBOARD_ONLY: Grants = Grants {
    view: false,
    control: false,
    clipboard: true,
    transfer: false,
};

/// Pairing grants no control, an invitation is accepted once, and a grant
/// runs one way: the controller's grant lets it see the host's desktop,
/// never the host the controller's. Pairing admits a connection, but
/// clipboard alone opens no Desktop; a connection the host stopped opens
/// nothing, and a revoked peer is refused.
#[test]
fn pairing_does_not_grant_control_and_a_grant_is_directional() -> TestResult {
    let trace = traced();
    let host = Engine::start("host");
    let controller = Engine::start("controller");
    let capabilities = host.core.capabilities();
    assert!(
        !capabilities.desktop_capture && !capabilities.keyboard,
        "a host without adapters reports neither capture nor keyboard"
    );
    InputLog::install(&host, MONITOR)?;
    InputLog::install(&controller, MONITOR)?;
    host.core.start_hosting()?;
    let token = host.core.create_invitation(INVITATION)?;
    let paired = controller.core.accept_invitation(&token)?;
    assert!(!paired.grants.control, "pairing grants no control");
    assert!(
        matches!(
            controller.core.accept_invitation(&token),
            Err(Error::InvitationReplayed)
        ),
        "an invitation is accepted once"
    );
    host.core
        .accept_invitation(&controller.core.create_invitation(INVITATION)?)?;
    let connection = controller.connect_services(&host)?;
    let denied = controller.open_desktop(&connection);
    assert!(
        matches!(denied, Err(Error::Unauthorized)),
        "a paired controller without a grant sees no desktop: {:?}",
        denied.err()
    );

    controller.core.start_hosting()?;
    let back = host.connect_services(&controller)?;
    assert!(
        matches!(host.open_desktop(&back), Err(Error::Unauthorized)),
        "a grant runs one way: the host may not see the controller"
    );

    host.core.set_desktop_sharing(CLIPBOARD_ONLY)?;
    assert!(
        matches!(
            controller.open_desktop(&connection),
            Err(Error::Unauthorized)
        ),
        "clipboard alone opens no Desktop"
    );

    host.core.set_desktop_sharing(SEEN_AND_CONTROLLED)?;
    let desktop = controller.open_desktop(&connection)?;
    assert!(!desktop.is_cancelled(), "view and control open a Desktop");
    desktop.close();
    host.wait_released(Some(vnilink::ServiceKind::Desktop));

    host.core.stop_hosted_session()?;
    host.wait_closed(SessionRole::Host);
    controller.wait_closed(SessionRole::Controller);
    assert!(
        controller.open_desktop(&connection).is_err(),
        "a stopped connection opens nothing"
    );

    host.core.revoke(&controller.endpoint())?;
    // Revoking also ends the host's own connection to the controller.
    controller.wait_closed(SessionRole::Host);
    assert_eq!(
        refusal(&controller, &host)?,
        Error::Unauthorized.to_string(),
        "a revoked controller is refused"
    );
    expect![[r#"
        controller:
          pairing: invitation accepted
          pairing: invitation accepted refused=invitation replayed
          pairing: invitation issued direct=1 relays=0 ttl_s=60
          session: session phase from=connecting generation=1 role=Controller session=1 to=active
          session: session phase from=active generation=1 role=Controller session=1 to=closed
          session: the session's control stream ended error=network session=2
          session: session phase from=active generation=1 role=Host session=2 to=closed
          session: session failed failure=unauthorized session=3
          session: session phase from=connecting generation=0 role=Controller session=3 to=failed
        host:
          pairing: invitation issued direct=1 relays=0 ttl_s=60
          pairing: invitation accepted
          session: session phase from=connecting generation=1 role=Controller session=2 to=active
          session: session phase from=active generation=1 role=Host session=1 to=stopping
          session: session phase from=stopping generation=1 role=Host session=1 to=closed
          pairing: unpair requested
          session: session phase from=active generation=1 role=Controller session=2 to=stopping
          session: session phase from=stopping generation=1 role=Controller session=2 to=closed
    "#]]
    .assert_eq(&trace.story(&["session", "hosting", "pairing"]));
    Ok(())
}

/// An invitation lives more than nothing and at most fifteen minutes, and a
/// token that is not one is refused as invalid.
#[test]
fn invitation_lifetimes_and_malformed_tokens_are_rejected() -> TestResult {
    let _trace = traced();
    let host = Engine::start("host");
    let controller = Engine::start("controller");
    assert!(
        matches!(
            host.core.create_invitation(Duration::ZERO),
            Err(Error::InvalidInvitation(_))
        ),
        "an invitation that lives no time is refused"
    );
    assert!(
        matches!(
            host.core
                .create_invitation(Duration::from_secs(15 * 60 + 1)),
            Err(Error::InvalidInvitation(_))
        ),
        "an invitation that lives past fifteen minutes is refused"
    );
    host.core.create_invitation(Duration::from_secs(15 * 60))?;
    for token in ["", "nope", "vnl1.zz"] {
        assert!(
            matches!(
                controller.core.accept_invitation(token),
                Err(Error::InvalidInvitation(_))
            ),
            "a malformed token is refused as invalid: {token:?}"
        );
    }
    Ok(())
}

/// Accepting a new invitation from a known controller keeps the grants the
/// host already shares: clipboard stays, control does not appear.
#[test]
fn accepting_a_new_invitation_neither_adds_nor_removes_grants() -> TestResult {
    let _trace = traced();
    let host = Engine::start("host");
    let controller = Engine::start("controller");
    host.core.start_hosting()?;
    introduce(&host, &controller)?;
    host.core.set_desktop_sharing(CLIPBOARD_ONLY)?;
    let info = host
        .core
        .accept_invitation(&controller.core.create_invitation(INVITATION)?)?;
    assert_eq!(
        (
            info.grants.control,
            info.grants.clipboard,
            info.grants.transfer
        ),
        (false, true, false),
        "a new invitation reports the grants already shared"
    );
    let device = host
        .core
        .snapshot()?
        .devices
        .into_iter()
        .find(|device| device.endpoint_id == controller.endpoint())
        .ok_or("the controller is known")?;
    assert!(
        !device.grants.control && device.grants.clipboard,
        "the host's record keeps clipboard without control"
    );
    let connection = controller.connect_services(&host)?;
    assert!(
        matches!(
            controller.open_desktop(&connection),
            Err(Error::Unauthorized)
        ),
        "the new invitation opens no Desktop"
    );
    Ok(())
}

/// Revoking a controller forgets who it is, not what the host shares: a new
/// invitation from it is granted the same control again.
#[test]
fn re_pairing_uses_unchanged_global_consent() -> TestResult {
    let _trace = traced();
    let host = Engine::start("host");
    let controller = Engine::start("controller");
    InputLog::install(&host, MONITOR)?;
    host.pair_for_control(&controller)?;
    host.core.revoke(&controller.endpoint())?;
    let info = host
        .core
        .accept_invitation(&controller.core.create_invitation(INVITATION)?)?;
    assert_eq!(
        (
            info.grants.control,
            info.grants.clipboard,
            info.grants.transfer
        ),
        (true, false, false),
        "the restored identity takes the host's unchanged sharing"
    );
    let device = host
        .core
        .snapshot()?
        .devices
        .into_iter()
        .find(|device| device.endpoint_id == controller.endpoint())
        .ok_or("the controller is known again")?;
    assert!(device.grants.control, "the host's record grants control");
    let connection = controller.connect_services(&host)?;
    assert!(
        !controller.open_desktop(&connection)?.is_cancelled(),
        "the re-paired controller opens a Desktop"
    );
    Ok(())
}

/// A controller must be paired and the host hosting before a connection
/// opens; a host without a desktop offers no Desktop, and connecting again
/// reuses the live connection.
#[test]
fn connect_requires_pairing_and_hosting() -> TestResult {
    let trace = traced();
    let host = Engine::start("host");
    let controller = Engine::start("controller");
    assert!(
        !host.core.capabilities().keyboard,
        "no desktop, no keyboard"
    );
    assert_eq!(
        host.core.capabilities().monitors,
        0,
        "no desktop, no monitor"
    );
    assert!(
        matches!(
            controller.core.begin_connect_services(&host.endpoint()),
            Err(Error::NotPaired)
        ),
        "an unpaired controller is refused before dialing"
    );
    introduce(&host, &controller)?;
    host.core.set_desktop_sharing(SEEN_AND_CONTROLLED)?;
    for attempt in ["first", "second"] {
        assert_eq!(
            refusal(&controller, &host)?,
            Error::NotHosting.to_string(),
            "a host that is not hosting refuses the {attempt} attempt"
        );
    }
    host.core.start_hosting()?;
    let session = controller.connect_services(&host)?;
    assert_eq!(
        session.phase,
        SessionPhase::Active,
        "a paired controller of a hosting host is admitted"
    );
    assert!(
        !controller
            .core
            .remote_service_support(session.id, session.generation)?
            .desktop,
        "a host without a desktop offers no Desktop"
    );
    assert_eq!(
        controller.core.begin_connect_services(&host.endpoint())?.id,
        session.id,
        "connecting again reuses the live connection"
    );
    expect![[r#"
        controller:
          session: session failed failure=not hosting session=1
          session: session phase from=connecting generation=0 role=Controller session=1 to=failed
          session: session failed failure=not hosting session=2
          session: session phase from=connecting generation=0 role=Controller session=2 to=failed
          session: session phase from=connecting generation=1 role=Controller session=3 to=active
    "#]]
    .assert_eq(&trace.story(&["session", "hosting"]));
    Ok(())
}

/// The host's desktop is fixed while a Desktop uses it, and its monitor
/// size reaches the controller; after the Desktop a desktop with no
/// picture is input only.
#[test]
fn the_desktop_adapter_cannot_change_during_a_session() -> TestResult {
    let _trace = traced();
    let host = Engine::start("host");
    let controller = Engine::start("controller");
    InputLog::install(&host, MONITOR)?;
    assert_eq!(
        host.core.capabilities().monitors,
        1,
        "a desktop with a monitor is one monitor"
    );
    host.pair_for_control(&controller)?;
    let connection = controller.connect_services(&host)?;
    let desktop = controller.open_desktop(&connection)?;
    let info = desktop.desktop_info()?;
    assert_eq!(
        (info.monitor_width, info.monitor_height),
        MONITOR,
        "the host's monitor size reaches the controller"
    );
    assert!(
        matches!(InputLog::install(&host, (0, 0)), Err(Error::Busy)),
        "the desktop does not change while a Desktop uses it"
    );
    desktop.close();
    host.wait_released(Some(vnilink::ServiceKind::Desktop));
    wait_for(
        "the ended Desktop to free the desktop",
        DEFAULT_WAIT,
        || InputLog::install(&host, (100, 0)).ok(),
    );
    let capabilities = host.core.capabilities();
    assert!(
        capabilities.keyboard && capabilities.pointer,
        "a desktop takes keyboard and pointer"
    );
    assert!(!capabilities.desktop_capture, "a desktop is not a capture");
    assert_eq!(
        capabilities.monitors, 0,
        "a monitor with no height is no monitor"
    );
    Ok(())
}

/// Admission publishes hosting, pairing and authorization, then each side's
/// session phases in order up to active, and the controller's revocation
/// closes both sides.
#[test]
fn admission_publishes_the_session_lifecycle() -> TestResult {
    let trace = traced();
    let host = Engine::start("host");
    let controller = Engine::start("controller");
    let host_events = host.core.subscribe()?;
    let controller_events = controller.core.subscribe()?;
    host.core.start_hosting()?;
    assert_eq!(
        next_event(
            &host_events,
            "the engine to publish the event",
            DEFAULT_WAIT,
            |event| matches!(event, CoreEvent::HostingChanged { .. })
        ),
        CoreEvent::HostingChanged { hosting: true },
        "hosting is published"
    );
    controller
        .core
        .accept_invitation(&host.core.create_invitation(INVITATION)?)?;
    next_event(
        &controller_events,
        "the engine to publish the event",
        DEFAULT_WAIT,
        |event| matches!(event, CoreEvent::DevicePaired { .. }),
    );
    host.core
        .accept_invitation(&controller.core.create_invitation(INVITATION)?)?;
    assert_eq!(
        next_event(
            &host_events,
            "the engine to publish the event",
            DEFAULT_WAIT,
            |event| matches!(event, CoreEvent::DevicePaired { .. })
        ),
        CoreEvent::DevicePaired {
            endpoint_id: controller.endpoint()
        },
        "the host publishes the controller paired"
    );
    host.core.set_desktop_sharing(SEEN_AND_CONTROLLED)?;
    assert_eq!(
        next_event(
            &host_events,
            "the engine to publish the event",
            DEFAULT_WAIT,
            |event| matches!(event, CoreEvent::DeviceAuthorized { .. })
        ),
        CoreEvent::DeviceAuthorized {
            endpoint_id: controller.endpoint()
        },
        "the host publishes the controller authorized"
    );
    let session = controller.connect_services(&host)?;
    let mut host_phases = Vec::new();
    while host_phases.last() != Some(&SessionPhase::Active) {
        match next_event(
            &host_events,
            "the engine to publish the event",
            DEFAULT_WAIT,
            |event| {
                matches!(
                    event,
                    CoreEvent::SessionChanged {
                        role: SessionRole::Host,
                        ..
                    }
                )
            },
        ) {
            CoreEvent::SessionChanged {
                id,
                generation,
                phase,
                peer_id,
                ..
            } => {
                assert_eq!(
                    (id, generation, peer_id.as_str()),
                    (
                        session.id,
                        session.generation,
                        controller.endpoint().as_str()
                    ),
                    "the host's session events name the controller's session"
                );
                host_phases.push(phase);
            }
            other => panic!("host session event was {other:?}"),
        }
    }
    assert_eq!(
        host_phases,
        [
            SessionPhase::Authenticated,
            SessionPhase::Authorized,
            SessionPhase::Negotiating,
            SessionPhase::Active,
        ],
        "the host publishes each phase up to active"
    );
    let mut controller_phases = Vec::new();
    while controller_phases.last() != Some(&SessionPhase::Active) {
        match next_event(
            &controller_events,
            "the engine to publish the event",
            DEFAULT_WAIT,
            |event| {
                matches!(
                    event,
                    CoreEvent::SessionChanged {
                        role: SessionRole::Controller,
                        ..
                    }
                )
            },
        ) {
            CoreEvent::SessionChanged { phase, .. } => controller_phases.push(phase),
            other => panic!("controller session event was {other:?}"),
        }
    }
    assert_eq!(
        controller_phases,
        [
            SessionPhase::Connecting,
            SessionPhase::Authenticated,
            SessionPhase::Authorized,
            SessionPhase::Negotiating,
            SessionPhase::Active,
        ],
        "the controller publishes each phase from connecting to active"
    );
    host.core.revoke(&controller.endpoint())?;
    assert_eq!(
        next_event(
            &host_events,
            "the engine to publish the event",
            DEFAULT_WAIT,
            |event| matches!(event, CoreEvent::DeviceUnpaired { .. })
        ),
        CoreEvent::DeviceUnpaired {
            endpoint_id: controller.endpoint()
        },
        "the host publishes the controller unpaired"
    );
    host.wait_closed(SessionRole::Host);
    controller.wait_closed(SessionRole::Controller);
    expect![[r#"
        controller:
          session: session phase from=connecting generation=1 role=Controller session=1 to=active
          session: session phase from=active generation=1 role=Controller session=1 to=closed
        host:
          session: session phase from=active generation=1 role=Host session=1 to=stopping
          session: session phase from=stopping generation=1 role=Host session=1 to=closed
    "#]]
    .assert_eq(&trace.story(&["session"]));
    Ok(())
}

/// A host whose capture has no encoder refuses a Desktop as unsupported
/// instead of opening one with no video; once it reports an encoder, the
/// same host opens it.
#[test]
fn a_host_without_an_encoder_reports_unsupported_instead_of_admitting() -> TestResult {
    let _trace = traced();
    let host = Engine::start("host");
    let controller = Engine::start("controller");
    host.core
        .set_capture(Box::new(MonitorCapture::new((1, 1))))?;
    host.core.set_video_codecs(VideoCodecCaps::default())?;
    assert!(
        host.core.capabilities().desktop_capture,
        "a capture is reported without an encoder"
    );
    assert!(
        !host.core.capabilities().h264_encode,
        "no codecs, no encoder"
    );
    host.pair_for_control(&controller)?;
    let connection = controller.connect_services(&host)?;
    assert!(
        matches!(
            controller.open_desktop(&connection),
            Err(Error::Unsupported(_))
        ),
        "a host that cannot encode refuses the Desktop as unsupported"
    );
    host.core
        .set_video_codecs(VideoCodecCaps::encode_only(4096, 4096, "test"))?;
    let capabilities = host.core.capabilities();
    assert!(
        capabilities.h264_encode && !capabilities.h264_decode,
        "an encoder alone is reported as one"
    );
    let desktop = controller.open_desktop(&connection)?;
    assert!(
        !desktop.is_cancelled(),
        "a host that can encode opens the Desktop"
    );
    desktop.close();
    host.wait_released(Some(vnilink::ServiceKind::Desktop));
    Ok(())
}

/// What decoders a controller has shows in its capabilities.
#[test]
fn controller_decode_availability_is_visible_in_capabilities() -> TestResult {
    let _trace = traced();
    let controller = Engine::start("controller");
    controller
        .core
        .set_video_codecs(VideoCodecCaps::default())?;
    assert!(
        !controller.core.capabilities().h264_decode,
        "no codecs, no decoder"
    );
    controller
        .core
        .set_video_codecs(VideoCodecCaps::decode_only("test"))?;
    let capabilities = controller.core.capabilities();
    assert!(
        capabilities.h264_decode && !capabilities.h264_encode,
        "a decoder alone is reported as one"
    );
    Ok(())
}

/// A controller that cannot decode is refused a Desktop as unsupported
/// before it asks the host.
#[test]
fn a_controller_without_a_decoder_is_rejected_before_dialing() -> TestResult {
    let _trace = traced();
    let host = Engine::start("host");
    let controller = Engine::start("controller");
    InputLog::install(&host, MONITOR)?;
    host.pair_for_control(&controller)?;
    let connection = controller.connect_services(&host)?;
    controller
        .core
        .set_video_codecs(VideoCodecCaps::default())?;
    assert!(
        matches!(
            controller.open_desktop(&connection),
            Err(Error::Unsupported(_))
        ),
        "a controller without a decoder is refused as unsupported"
    );
    assert!(
        host.hosted(None).is_empty(),
        "the host was never asked for the Desktop"
    );
    Ok(())
}
