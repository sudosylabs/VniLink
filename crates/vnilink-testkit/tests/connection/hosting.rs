//! What ends a hosted session and what it leaves: stopping hosting, the
//! controller disconnecting, the host revoking or unpairing it, and control
//! withdrawn mid-session each release the input the controller held, and
//! each says what a later connect meets.

use expect_test::expect;
use std::time::Duration;

use vnilink::{
    usage, Error, Grants, InputEvent, PointerButton, Quality, ServiceKind, SessionPhase,
    SessionRole,
};
use vnilink_testkit::desktop::{InputLog, MonitorCapture};
use vnilink_testkit::{traced, wait_for, Engine, TestResult, DEFAULT_WAIT};

use super::support::{pair, wait_closed};

/// The monitor of the desktops these scenarios install.
const MONITOR: (u16, u16) = (1920, 1080);

/// Input names its session and generation: another session's id or a newer
/// generation is refused, and once disconnected the session takes none.
#[test]
fn input_for_the_wrong_session_or_generation_is_rejected() -> TestResult {
    let _trace = traced();
    let host = Engine::start("host");
    let controller = Engine::start("controller");
    pair(&host, &controller)?;
    let session = controller.core.connect(&host.endpoint())?;
    let key = InputEvent::key_down(usage::A);
    assert!(
        matches!(
            controller
                .core
                .submit_input(session.id + 1, session.generation, key),
            Err(Error::NoSession)
        ),
        "input for another session is refused"
    );
    assert!(
        matches!(
            controller
                .core
                .submit_input(session.id, session.generation + 1, key),
            Err(Error::StaleSession)
        ),
        "input for another generation is refused as stale"
    );
    controller
        .core
        .submit_input(session.id, session.generation, key)?;
    controller.core.disconnect()?;
    assert!(
        matches!(
            controller.core.submit_input(
                session.id,
                session.generation,
                InputEvent::key_up(usage::A)
            ),
            Err(Error::NoSession)
        ),
        "a disconnected session takes no input"
    );
    Ok(())
}

/// Stopping hosting turns new sessions away even when none is live, while
/// the host keeps what it granted its peers; stopping a hosted session that
/// does not exist changes nothing.
#[test]
fn stopping_hosting_disables_new_sessions_even_when_none_is_active() -> TestResult {
    let _trace = traced();
    let idle = Engine::start("idle");
    idle.core.stop_hosting()?;
    assert!(
        !idle.core.snapshot()?.hosting,
        "an engine that never hosted stops hosting"
    );

    let host = Engine::start("host");
    let controller = Engine::start("controller");
    pair(&host, &controller)?;
    assert!(
        matches!(host.core.stop_hosted_session(), Err(Error::NoSession)),
        "there is no hosted session to stop"
    );
    assert!(
        host.core.snapshot()?.hosting,
        "a failed stop leaves the host hosting"
    );
    host.core.stop_hosting()?;
    assert!(
        !host.core.snapshot()?.hosting,
        "stopping hosting with no session stops hosting"
    );
    assert!(
        matches!(
            controller.core.connect(&host.endpoint()),
            Err(Error::NotHosting)
        ),
        "a host that stopped hosting turns a connect away"
    );
    assert!(
        host.core.snapshot()?.devices[0].grants.control,
        "stopping hosting keeps the controller's grant"
    );
    Ok(())
}

/// Stopping hosting ends the live session on both sides and lifts what the
/// controller held, the button then the key; the controller keeps its
/// grant, and hosting again admits it.
#[test]
fn stopping_hosting_ends_the_active_session_and_releases_keys() -> TestResult {
    let trace = traced();
    let host = Engine::start("host");
    let controller = Engine::start("controller");
    let applied = InputLog::install(&host, MONITOR)?;
    pair(&host, &controller)?;
    let session = controller.core.connect(&host.endpoint())?;
    let held = [
        InputEvent::key_down(usage::A),
        InputEvent::button_down(PointerButton::Left),
    ];
    for event in held {
        controller
            .core
            .submit_input(session.id, session.generation, event)?;
    }
    assert_eq!(applied.wait_for(2), held, "the controller's input applies");
    host.core.stop_hosting()?;
    assert_eq!(
        applied.wait_for(4),
        [
            held[0],
            held[1],
            InputEvent::button_up(PointerButton::Left),
            InputEvent::key_up(usage::A),
        ],
        "stopping hosting lifts the held button, then the held key"
    );
    wait_closed(&host, SessionRole::Host);
    wait_closed(&controller, SessionRole::Controller);
    let snapshot = host.core.snapshot()?;
    assert!(!snapshot.hosting, "the host no longer hosts");
    assert!(
        snapshot.devices[0].grants.control,
        "stopping hosting keeps the controller's grant"
    );
    assert!(
        matches!(
            controller.core.connect(&host.endpoint()),
            Err(Error::NotHosting)
        ),
        "a host that stopped hosting turns a connect away"
    );
    host.core.start_hosting()?;
    assert_eq!(
        controller.core.connect(&host.endpoint())?.phase,
        SessionPhase::Active,
        "hosting again admits the controller"
    );
    expect![[r#"
        controller:
          session: session phase from=connecting generation=1 role=Controller session=1 to=active
          session: session phase from=active generation=1 role=Controller session=1 to=closed
          session: session failed failure=not hosting session=2
          session: session phase from=connecting generation=0 role=Controller session=2 to=failed
          session: session phase from=connecting generation=3 role=Controller session=3 to=active
        host:
          session: session phase from=active generation=1 role=Host session=1 to=stopping
          session: session phase from=stopping generation=1 role=Host session=1 to=closed
    "#]]
    .assert_eq(&trace.story(&["session", "hosting"]));
    Ok(())
}

/// The controller disconnecting closes both sides and lifts what it held;
/// the host keeps hosting, the old session takes nothing more, a second
/// disconnect finds no session, and the controller connects again.
#[test]
fn disconnect_closes_both_sides_and_releases_held_input() -> TestResult {
    let trace = traced();
    let host = Engine::start("host");
    let controller = Engine::start("controller");
    let applied = InputLog::install(&host, MONITOR)?;
    pair(&host, &controller)?;
    let session = controller.core.connect(&host.endpoint())?;
    let held = [
        InputEvent::key_down(usage::A),
        InputEvent::button_down(PointerButton::Left),
    ];
    for event in held {
        controller
            .core
            .submit_input(session.id, session.generation, event)?;
    }
    assert_eq!(applied.wait_for(2), held, "the controller's input applies");
    controller.core.disconnect()?;
    wait_closed(&host, SessionRole::Host);
    wait_closed(&controller, SessionRole::Controller);
    assert_eq!(
        applied.wait_for(4),
        [
            held[0],
            held[1],
            InputEvent::button_up(PointerButton::Left),
            InputEvent::key_up(usage::A),
        ],
        "disconnecting lifts the held button, then the held key"
    );
    assert!(
        host.core.snapshot()?.hosting,
        "a controller disconnecting leaves the host hosting"
    );
    assert!(
        matches!(
            controller.core.submit_input(
                session.id,
                session.generation,
                InputEvent::key_down(usage::A)
            ),
            Err(Error::NoSession)
        ),
        "a disconnected session takes no input"
    );
    assert!(
        matches!(controller.core.disconnect(), Err(Error::NoSession)),
        "there is nothing left to disconnect"
    );
    assert_eq!(
        controller.core.connect(&host.endpoint())?.phase,
        SessionPhase::Active,
        "the controller connects again"
    );
    expect![[r#"
        controller:
          session: session phase from=connecting generation=1 role=Controller session=1 to=active
          session: session phase from=active generation=1 role=Controller session=1 to=stopping
          session: session phase from=stopping generation=1 role=Controller session=1 to=closed
          session: session phase from=connecting generation=3 role=Controller session=2 to=active
        host:
          session: session phase from=active generation=1 role=Host session=1 to=closed
    "#]]
    .assert_eq(&trace.story(&["session", "hosting"]));
    Ok(())
}

/// Revoking the controller ends its session and lifts what it held; the
/// host forgets it, a connect is refused, and the old session takes no
/// input.
#[test]
fn revoke_stops_the_session_releases_input_and_blocks_reconnect() -> TestResult {
    let trace = traced();
    let host = Engine::start("host");
    let controller = Engine::start("controller");
    let applied = InputLog::install(&host, MONITOR)?;
    pair(&host, &controller)?;
    let session = controller.core.connect(&host.endpoint())?;
    let held = [
        InputEvent::key_down(usage::LEFT_SHIFT),
        InputEvent::button_down(PointerButton::Right),
    ];
    for event in held {
        controller
            .core
            .submit_input(session.id, session.generation, event)?;
    }
    assert_eq!(applied.wait_for(2), held, "the controller's input applies");
    host.core.revoke(&controller.endpoint())?;
    assert_eq!(
        applied.wait_for(4),
        [
            held[0],
            held[1],
            InputEvent::button_up(PointerButton::Right),
            InputEvent::key_up(usage::LEFT_SHIFT),
        ],
        "revoking lifts the held button, then the held key"
    );
    wait_closed(&host, SessionRole::Host);
    wait_closed(&controller, SessionRole::Controller);
    assert!(
        host.core.snapshot()?.devices.is_empty(),
        "a revoked controller is forgotten"
    );
    assert!(
        matches!(
            controller.core.connect(&host.endpoint()),
            Err(Error::Unauthorized)
        ),
        "a revoked controller is refused"
    );
    assert!(
        matches!(
            controller.core.submit_input(
                session.id,
                session.generation,
                InputEvent::key_down(usage::A)
            ),
            Err(Error::NoSession)
        ),
        "the revoked session takes no input"
    );
    expect![[r#"
        controller:
          pairing: invitation accepted
          pairing: invitation issued direct=1 relays=0 ttl_s=60
          session: session phase from=connecting generation=1 role=Controller session=1 to=active
          session: session phase from=active generation=1 role=Controller session=1 to=closed
          session: session failed failure=unauthorized session=2
          session: session phase from=connecting generation=0 role=Controller session=2 to=failed
        host:
          pairing: invitation issued direct=1 relays=0 ttl_s=60
          pairing: invitation accepted
          pairing: unpair requested
          session: session phase from=active generation=1 role=Host session=1 to=stopping
          session: session phase from=stopping generation=1 role=Host session=1 to=closed
    "#]]
    .assert_eq(&trace.story(&["session", "pairing"]));
    Ok(())
}

/// The host unpairing the controller forgets it rather than keeping it with
/// no grants, ends its session and lifts what it held; the controller still
/// dials, and the host refuses a peer it no longer knows.
#[test]
fn unpair_forgets_the_device_stops_the_session_and_blocks_reconnect() -> TestResult {
    let trace = traced();
    let host = Engine::start("host");
    let controller = Engine::start("controller");
    let applied = InputLog::install(&host, MONITOR)?;
    pair(&host, &controller)?;
    let session = controller.core.connect(&host.endpoint())?;
    let shift = InputEvent::key_down(usage::LEFT_SHIFT);
    controller
        .core
        .submit_input(session.id, session.generation, shift)?;
    assert_eq!(applied.wait_for(1), [shift], "the controller's key applies");
    host.core.unpair(&controller.endpoint())?;
    assert_eq!(
        applied.wait_for(2),
        [shift, InputEvent::key_up(usage::LEFT_SHIFT)],
        "unpairing lifts the held key, as any local stop does"
    );
    wait_closed(&host, SessionRole::Host);
    wait_closed(&controller, SessionRole::Controller);
    assert!(
        host.core.snapshot()?.devices.is_empty(),
        "an unpaired controller is forgotten, not kept without grants"
    );
    assert!(
        matches!(
            controller.core.connect(&host.endpoint()),
            Err(Error::Unauthorized)
        ),
        "the host refuses a controller it no longer knows"
    );
    assert!(
        matches!(
            controller.core.submit_input(
                session.id,
                session.generation,
                InputEvent::key_down(usage::A)
            ),
            Err(Error::NoSession)
        ),
        "the ended session takes no input"
    );
    expect![[r#"
        controller:
          pairing: invitation accepted
          pairing: invitation issued direct=1 relays=0 ttl_s=60
          session: session phase from=connecting generation=1 role=Controller session=1 to=active
          session: session phase from=active generation=1 role=Controller session=1 to=closed
          session: session failed failure=unauthorized session=2
          session: session phase from=connecting generation=0 role=Controller session=2 to=failed
        host:
          pairing: invitation issued direct=1 relays=0 ttl_s=60
          pairing: invitation accepted
          pairing: unpair requested
          session: session phase from=active generation=1 role=Host session=1 to=stopping
          session: session phase from=stopping generation=1 role=Host session=1 to=closed
    "#]]
    .assert_eq(&trace.story(&["session", "pairing"]));
    Ok(())
}

/// Unpairing a peer the engine does not know reports it and disturbs
/// nothing; a controller that forgets its host ends its session and is
/// refused before dialing.
#[test]
fn unpairing_an_unknown_device_reports_not_paired() -> TestResult {
    let _trace = traced();
    let host = Engine::start("host");
    let controller = Engine::start("controller");
    assert!(
        matches!(
            host.core.unpair(&controller.endpoint()),
            Err(Error::NotPaired)
        ),
        "an unknown peer is not paired"
    );
    pair(&host, &controller)?;
    assert_eq!(
        controller.core.connect(&host.endpoint())?.phase,
        SessionPhase::Active,
        "pairing still works afterwards"
    );
    controller.core.unpair(&host.endpoint())?;
    wait_closed(&controller, SessionRole::Controller);
    assert!(
        matches!(
            controller.core.connect(&host.endpoint()),
            Err(Error::NotPaired)
        ),
        "a controller that forgot its host is refused before dialing"
    );
    Ok(())
}

/// A controller unpairing its host forgets it and ends its outgoing
/// session; unpairing is local, so the host still knows the controller and
/// closes its side because the session's stream ended.
#[test]
fn a_controller_unpair_ends_its_outgoing_session() -> TestResult {
    let trace = traced();
    let host = Engine::start("host");
    let controller = Engine::start("controller");
    pair(&host, &controller)?;
    controller.core.connect(&host.endpoint())?;
    controller.core.unpair(&host.endpoint())?;
    assert!(
        controller.core.snapshot()?.devices.is_empty(),
        "the controller forgets its host"
    );
    wait_closed(&controller, SessionRole::Controller);
    wait_closed(&host, SessionRole::Host);
    assert_eq!(
        host.core.snapshot()?.devices.len(),
        1,
        "unpairing is local: the host still knows the controller"
    );
    expect![[r#"
        controller:
          pairing: invitation accepted
          pairing: invitation issued direct=1 relays=0 ttl_s=60
          session: session phase from=connecting generation=1 role=Controller session=1 to=active
          pairing: unpair requested
          session: session phase from=active generation=1 role=Controller session=1 to=stopping
          session: session phase from=stopping generation=1 role=Controller session=1 to=closed
        host:
          pairing: invitation issued direct=1 relays=0 ttl_s=60
          pairing: invitation accepted
          session: session phase from=active generation=1 role=Host session=1 to=closed
    "#]]
    .assert_eq(&trace.story(&["session", "pairing"]));
    Ok(())
}

/// Withdrawing control mid-session lifts the held key and discards what
/// the controller sends afterwards, while the session stays active and
/// still carries quality changes.
#[test]
fn removing_control_mid_session_stops_applying_input() -> TestResult {
    let trace = traced();
    let host = Engine::start("host");
    let controller = Engine::start("controller");
    let applied = InputLog::install(&host, MONITOR)?;
    pair(&host, &controller)?;
    let session = controller.core.connect(&host.endpoint())?;
    let pressed = InputEvent::key_down(usage::A);
    controller
        .core
        .submit_input(session.id, session.generation, pressed)?;
    assert_eq!(
        applied.wait_for(1),
        [pressed],
        "the controller's key applies"
    );
    host.core.set_desktop_sharing(Grants {
        view: true,
        control: false,
        clipboard: true,
        transfer: false,
    })?;
    controller.core.submit_input(
        session.id,
        session.generation,
        InputEvent::key_down(usage::B),
    )?;
    let released = [pressed, InputEvent::key_up(usage::A)];
    assert_eq!(
        applied.wait_for(2),
        released,
        "withdrawing control lifts the held key"
    );
    // A quality change the host applies follows the key on the session, so
    // once it lands the key had its turn.
    let quality = Quality {
        max_bitrate_kbps: 1500,
        max_frame_rate: 20,
        max_width: 1280,
        max_height: 720,
    };
    controller
        .core
        .update_session_quality(session.id, SessionRole::Controller, quality)?;
    wait_for(
        "the host to apply the controller's quality",
        Duration::from_secs(5),
        || (host.core.snapshot().ok()?.incoming?.quality == Some(quality)).then_some(()),
    );
    assert_eq!(
        applied.events(),
        released,
        "a key sent after control was withdrawn is not applied"
    );
    assert_eq!(
        host.core
            .snapshot()?
            .incoming
            .ok_or("a hosted session")?
            .phase,
        SessionPhase::Active,
        "the host keeps the session to be seen"
    );
    assert_eq!(
        controller.core.controller_session(session.id)?.phase,
        SessionPhase::Active,
        "the controller keeps the session to see"
    );
    expect![[r#"
        host:
          input: input discarded stamp=1 what=key down why=the peer may not control this computer
    "#]]
    .assert_eq(&trace.story(&["input"]));
    Ok(())
}

/// Stopping hosting fences every live peer at once, not only the first;
/// their transports may close later.
#[test]
fn stopping_hosting_stops_every_peer() -> TestResult {
    let _trace = traced();
    let host = Engine::start("host");
    let peers = [Engine::start("viewer-a"), Engine::start("viewer-b")];
    for peer in &peers {
        pair(&host, peer)?;
        peer.connect_services(&host)?;
    }
    let live = || -> Result<usize, Error> {
        Ok(host
            .core
            .hosted_sessions()?
            .iter()
            .filter(|session| session.phase == SessionPhase::Active)
            .count())
    };
    wait_for("both peers to be hosted", DEFAULT_WAIT, || {
        (live().ok()? == 2).then_some(())
    });
    host.core.stop_hosting()?;
    assert_eq!(
        live()?,
        0,
        "stopping hosting leaves no peer active, at once"
    );
    Ok(())
}

/// Stopping hosting cancels each peer's open desktop and refuses a new
/// session.
#[test]
fn stop_hosting_cancels_live_owners_and_refuses_new_sessions() -> TestResult {
    let trace = traced();
    let host = Engine::start("host");
    let peers = [Engine::start("viewer-a"), Engine::start("viewer-b")];
    host.core.set_capture(Box::new(MonitorCapture(MONITOR)))?;
    for peer in &peers {
        pair(&host, peer)?;
    }
    host.core.set_desktop_sharing(Grants {
        view: true,
        ..Grants::default()
    })?;
    let mut desktops = Vec::new();
    for peer in &peers {
        let connection = peer.connect_services(&host)?;
        desktops.push(peer.open_desktop(&connection)?);
    }
    host.core.stop_hosting()?;
    for desktop in &desktops {
        wait_for("the peer's desktop to be cancelled", DEFAULT_WAIT, || {
            desktop.is_cancelled().then_some(())
        });
    }
    assert!(
        matches!(
            peers[0].core.connect(&host.endpoint()),
            Err(Error::NotHosting)
        ),
        "a host that stopped hosting refuses a new session"
    );
    host.wait_released(Some(ServiceKind::Desktop));
    // Each side records its desktop closed just after closing it.
    let closed = |event: &serde_json::Value| {
        event["to"]
            .as_str()
            .is_some_and(|to| to.starts_with("closed"))
    };
    wait_for(
        "both sides to record both desktops closed",
        DEFAULT_WAIT,
        || {
            (trace
                .named("service phase")
                .iter()
                .filter(|e| closed(e))
                .count()
                == 4)
                .then_some(())
        },
    );
    // The host says it ended the connection, so each viewer's desktop ends
    // with its connection, never as closed by the host on its own.
    expect![[r#"
        host:
          service: service phase from=active generation=1 kind=Desktop service=1 to=closed: ParentLoss
          service: service phase from=active generation=2 kind=Desktop service=2 to=closed: ParentLoss
        viewer-a:
          service: service phase from=active generation=1 kind=Desktop service=1 to=closed: ParentLoss
        viewer-b:
          service: service phase from=active generation=2 kind=Desktop service=1 to=closed: ParentLoss
    "#]].assert_eq(
        &trace.tell(&["service"]).concurrent().to_string(),
    );
    Ok(())
}
