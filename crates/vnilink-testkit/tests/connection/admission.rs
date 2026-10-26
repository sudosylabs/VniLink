//! Who a host admits: pairing alone grants nothing, a grant runs one way,
//! invitations are bounded and never widen or narrow what was granted, and
//! a session opens only between a hosting host that can encode and a
//! controller that can decode, beside at most four others.

use expect_test::expect;
use std::time::Duration;

use vnilink::{
    usage, CoreEvent, Error, Grants, InputEvent, Quality, SessionPhase, SessionRole, VideoCodecCaps,
};
use vnilink_testkit::desktop::{InputLog, MonitorCapture};
use vnilink_testkit::events::next_event;
use vnilink_testkit::{traced, wait_for, Engine, TestResult, DEFAULT_WAIT, SEEN_AND_CONTROLLED};

use super::support::{introduce, INVITATION};

/// Only clipboard: what a host shares that admits no session.
const CLIPBOARD_ONLY: Grants = Grants {
    view: false,
    control: false,
    clipboard: true,
    transfer: false,
};

/// Pairing grants no control, an invitation is accepted once, and a grant
/// runs one way: the controller's grant lets it reach the host, never the
/// host it. Clipboard alone admits nothing; once admitted, input without a
/// desktop is refused, the host sets its session's quality, and a session
/// the host stopped or a revoked peer reaches nothing.
#[test]
fn pairing_does_not_grant_control_and_a_grant_is_directional() -> TestResult {
    let trace = traced();
    let host = Engine::start("host");
    let controller = Engine::start("controller");
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
    let denied = controller.core.connect(&host.endpoint());
    assert!(
        matches!(denied, Err(Error::Unauthorized)),
        "a paired controller without a grant is refused: {denied:?}"
    );

    controller.core.start_hosting()?;
    assert!(
        matches!(
            host.core.connect(&controller.endpoint()),
            Err(Error::Unauthorized)
        ),
        "a grant runs one way: the host may not reach the controller"
    );

    host.core.set_desktop_sharing(CLIPBOARD_ONLY)?;
    assert!(
        matches!(
            controller.core.connect(&host.endpoint()),
            Err(Error::Unauthorized)
        ),
        "clipboard alone admits no session"
    );

    host.core.set_desktop_sharing(SEEN_AND_CONTROLLED)?;
    let session = controller.core.connect(&host.endpoint())?;
    assert_eq!(
        session.phase,
        SessionPhase::Active,
        "view and control admit a session"
    );
    controller.core.submit_input(
        session.id,
        session.generation,
        InputEvent::key_down(usage::A),
    )?;
    let mut sent = 0;
    wait_for(
        "the host to refuse input without a desktop",
        Duration::from_secs(2),
        || match controller.core.submit_input(
            session.id,
            session.generation,
            InputEvent::key_up(usage::A),
        ) {
            Err(Error::Unsupported(_)) => Some(()),
            Ok(()) => {
                sent += 1;
                None
            }
            Err(error) => panic!("input without a desktop adapter returned {error}"),
        },
    );
    // Each key sent before the refusal arrived reaches the host, which
    // discards it, also once the host is stopping the session.
    let quality = Quality::new(2_000, 30, 1920, 1080)?;
    let hosted = host.core.snapshot()?.incoming.ok_or("a hosted session")?;
    host.core
        .update_session_quality(hosted.id, SessionRole::Host, quality)?;
    assert_eq!(
        host.core
            .snapshot()?
            .incoming
            .ok_or("a hosted session")?
            .quality,
        Some(quality),
        "the host sets its own session's quality"
    );

    host.core.stop_hosted_session()?;
    host.wait_closed(SessionRole::Host);
    controller.wait_closed(SessionRole::Controller);
    assert!(
        matches!(
            controller.core.submit_input(
                session.id,
                session.generation,
                InputEvent::key_down(usage::A),
            ),
            Err(Error::NoSession | Error::StaleSession)
        ),
        "a stopped session takes no input"
    );

    host.core.revoke(&controller.endpoint())?;
    assert!(
        matches!(
            controller.core.connect(&host.endpoint()),
            Err(Error::Unauthorized)
        ),
        "a revoked controller is refused"
    );
    let capabilities = host.core.capabilities();
    assert!(
        !capabilities.desktop_capture && !capabilities.keyboard,
        "a host without adapters reports neither capture nor keyboard"
    );
    expect![[r#"
        controller:
          pairing: invitation accepted
          pairing: invitation accepted refused=invitation replayed
          pairing: invitation issued direct=1 relays=0 ttl_s=60
          session: session failed failure=unauthorized session=1
          session: session phase from=connecting generation=0 role=Controller session=1 to=failed
          session: session failed failure=unauthorized session=2
          session: session phase from=connecting generation=0 role=Controller session=2 to=failed
          session: session phase from=connecting generation=1 role=Controller session=3 to=active
          session: session phase from=active generation=1 role=Controller session=3 to=closed
          session: session failed failure=unauthorized session=4
          session: session phase from=connecting generation=0 role=Controller session=4 to=failed
        host:
          pairing: invitation issued direct=1 relays=0 ttl_s=60
          pairing: invitation accepted
          session: session failed failure=unauthorized session=1
          session: session phase from=connecting generation=0 role=Controller session=1 to=failed
          session: session phase from=active generation=1 role=Host session=2 to=stopping
          session: session phase from=stopping generation=1 role=Host session=2 to=closed
          pairing: unpair requested
    "#]]
    .assert_eq(&trace.story(&["session", "hosting", "pairing"]));
    Ok(())
}

/// Four controllers are hosted at once; a fifth is refused as busy, and the
/// refusal leaves the four sessions as they were.
#[test]
fn peers_past_capacity_are_rejected_while_live_sessions_stay_active() -> TestResult {
    let trace = traced();
    let host = Engine::start("host");
    let peers: Vec<Engine> = (1..=5)
        .map(|n| Engine::start(&format!("controller-{n}")))
        .collect();
    for controller in &peers {
        host.pair_for_control(controller)?;
    }
    let mut sessions = Vec::new();
    for controller in &peers[..4] {
        let session = controller.core.connect(&host.endpoint())?;
        assert_eq!(
            session.phase,
            SessionPhase::Active,
            "four controllers are admitted"
        );
        sessions.push(session);
    }
    assert_eq!(
        host.core.hosted_sessions()?.len(),
        4,
        "four controllers are hosted"
    );
    let busy = peers[4].core.connect(&host.endpoint());
    assert!(
        matches!(busy, Err(Error::Busy)),
        "a fifth concurrent controller is refused as busy: {busy:?}"
    );
    assert_eq!(
        host.core.hosted_sessions()?.len(),
        4,
        "the refusal leaves the hosted sessions"
    );
    for (controller, session) in peers[..4].iter().zip(&sessions) {
        assert_eq!(
            controller.core.controller_session(session.id)?.phase,
            SessionPhase::Active,
            "the refusal leaves each live session active"
        );
    }
    expect![[r#"
        controller-1:
          session: session phase from=connecting generation=1 role=Controller session=1 to=active
        controller-2:
          session: session phase from=connecting generation=2 role=Controller session=1 to=active
        controller-3:
          session: session phase from=connecting generation=3 role=Controller session=1 to=active
        controller-4:
          session: session phase from=connecting generation=4 role=Controller session=1 to=active
        controller-5:
          session: session failed failure=busy session=1
          session: session phase from=connecting generation=0 role=Controller session=1 to=failed
    "#]]
    .assert_eq(&trace.story(&["session"]));
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
    assert!(
        matches!(
            controller.core.connect(&host.endpoint()),
            Err(Error::Unauthorized)
        ),
        "the new invitation admits no session"
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
    assert_eq!(
        controller.core.connect(&host.endpoint())?.phase,
        SessionPhase::Active,
        "the re-paired controller is admitted"
    );
    Ok(())
}

/// A controller must be paired and the host hosting before a session opens;
/// a host without a desktop reports a zero-sized monitor, and a second
/// session from the same controller is busy.
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
            controller.core.connect(&host.endpoint()),
            Err(Error::NotPaired)
        ),
        "an unpaired controller is refused before dialing"
    );
    introduce(&host, &controller)?;
    host.core.set_desktop_sharing(SEEN_AND_CONTROLLED)?;
    for attempt in ["first", "second"] {
        assert!(
            matches!(
                controller.core.connect(&host.endpoint()),
                Err(Error::NotHosting)
            ),
            "a host that is not hosting refuses the {attempt} attempt"
        );
    }
    host.core.start_hosting()?;
    let session = controller.core.connect(&host.endpoint())?;
    assert_eq!(
        session.phase,
        SessionPhase::Active,
        "a paired controller of a hosting host is admitted"
    );
    assert_eq!(
        (session.monitor_width, session.monitor_height),
        (0, 0),
        "a host without a desktop reports no monitor"
    );
    assert!(
        matches!(controller.core.connect(&host.endpoint()), Err(Error::Busy)),
        "a controller's second session is busy"
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

/// The host's desktop is fixed while a session uses it, and its monitor
/// size reaches the controller; after the session a desktop with no
/// picture is input only.
#[test]
fn the_desktop_adapter_cannot_change_during_a_session() -> TestResult {
    let _trace = traced();
    let host = Engine::start("host");
    let controller = Engine::start("controller");
    InputLog::install(&host, (1920, 1080))?;
    assert_eq!(
        host.core.capabilities().monitors,
        1,
        "a desktop with a monitor is one monitor"
    );
    host.pair_for_control(&controller)?;
    let session = controller.core.connect(&host.endpoint())?;
    assert_eq!(
        (session.monitor_width, session.monitor_height),
        (1920, 1080),
        "the host's monitor size reaches the controller"
    );
    assert!(
        matches!(InputLog::install(&host, (0, 0)), Err(Error::Busy)),
        "the desktop does not change during a session"
    );
    host.core.stop_hosted_session()?;
    host.wait_closed(SessionRole::Host);
    InputLog::install(&host, (100, 0))?;
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
    let session = controller.core.connect(&host.endpoint())?;
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

/// A host whose capture has no encoder refuses a session as unsupported
/// instead of admitting one with no video; once it reports an encoder, the
/// same host admits it.
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
    assert!(
        matches!(
            controller.core.connect(&host.endpoint()),
            Err(Error::Unsupported(_))
        ),
        "a host that cannot encode refuses the session as unsupported"
    );
    host.core
        .set_video_codecs(VideoCodecCaps::encode_only(4096, 4096, "test"))?;
    let capabilities = host.core.capabilities();
    assert!(
        capabilities.h264_encode && !capabilities.h264_decode,
        "an encoder alone is reported as one"
    );
    assert_eq!(
        controller.core.connect(&host.endpoint())?.phase,
        SessionPhase::Active,
        "a host that can encode admits the session"
    );
    controller.core.disconnect()?;
    controller.wait_closed(SessionRole::Controller);
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

/// A controller that cannot decode is refused as unsupported before it
/// dials.
#[test]
fn a_controller_without_a_decoder_is_rejected_before_dialing() -> TestResult {
    let _trace = traced();
    let host = Engine::start("host");
    let controller = Engine::start("controller");
    host.pair_for_control(&controller)?;
    controller
        .core
        .set_video_codecs(VideoCodecCaps::default())?;
    assert!(
        matches!(
            controller.core.connect(&host.endpoint()),
            Err(Error::Unsupported(_))
        ),
        "a controller without a decoder is refused as unsupported"
    );
    Ok(())
}
