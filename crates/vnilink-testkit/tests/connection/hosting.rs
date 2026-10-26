//! What ends a hosted session and what it leaves: stopping hosting, the
//! controller disconnecting, the host revoking or unpairing it, and control
//! withdrawn mid-session; each says what a later connect meets, and each
//! releases the input the controller's desktop held.

use expect_test::expect;

use vnilink::{
    usage, DesktopControlState, Error, Grants, InputEvent, PointerButton, ServiceKind,
    SessionPhase, SessionRole,
};
use vnilink_testkit::desktop::{InputLog, MonitorCapture};
use vnilink_testkit::{traced, wait_for, Engine, TestResult, DEFAULT_WAIT};

use super::support::refusal;

/// The monitor of the desktops these scenarios install.
const MONITOR: (u16, u16) = (1920, 1080);

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
    host.pair_for_control(&controller)?;
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
    assert_eq!(
        refusal(&controller, &host)?,
        Error::NotHosting.to_string(),
        "a host that stopped hosting turns a connect away"
    );
    assert!(
        host.core.snapshot()?.devices[0].grants.control,
        "stopping hosting keeps the controller's grant"
    );
    Ok(())
}

/// Stopping hosting ends the live session on both sides; the controller
/// keeps its grant, and hosting again admits it.
#[test]
fn stopping_hosting_ends_the_active_session() -> TestResult {
    let trace = traced();
    let host = Engine::start("host");
    let controller = Engine::start("controller");
    InputLog::install(&host, MONITOR)?;
    host.pair_for_control(&controller)?;
    controller.connect_services(&host)?;
    host.core.stop_hosting()?;
    host.wait_closed(SessionRole::Host);
    controller.wait_closed(SessionRole::Controller);
    let snapshot = host.core.snapshot()?;
    assert!(!snapshot.hosting, "the host no longer hosts");
    assert!(
        snapshot.devices[0].grants.control,
        "stopping hosting keeps the controller's grant"
    );
    assert_eq!(
        refusal(&controller, &host)?,
        Error::NotHosting.to_string(),
        "a host that stopped hosting turns a connect away"
    );
    host.core.start_hosting()?;
    assert_eq!(
        controller.connect_services(&host)?.phase,
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

/// The controller disconnecting closes both sides; the host keeps hosting,
/// a second disconnect finds no session, and the controller connects again.
#[test]
fn disconnect_closes_both_sides() -> TestResult {
    let trace = traced();
    let host = Engine::start("host");
    let controller = Engine::start("controller");
    InputLog::install(&host, MONITOR)?;
    host.pair_for_control(&controller)?;
    controller.connect_services(&host)?;
    controller.core.disconnect()?;
    host.wait_closed(SessionRole::Host);
    controller.wait_closed(SessionRole::Controller);
    assert!(
        host.core.snapshot()?.hosting,
        "a controller disconnecting leaves the host hosting"
    );
    assert!(
        matches!(controller.core.disconnect(), Err(Error::NoSession)),
        "there is nothing left to disconnect"
    );
    assert_eq!(
        controller.connect_services(&host)?.phase,
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
          session: the session's control stream ended error=network session=1
          session: session phase from=active generation=1 role=Host session=1 to=closed
    "#]]
    .assert_eq(&trace.story(&["session", "hosting"]));
    Ok(())
}

/// Revoking the controller ends its session; the host forgets it, and a
/// connect is refused.
#[test]
fn revoke_stops_the_session_and_blocks_reconnect() -> TestResult {
    let trace = traced();
    let host = Engine::start("host");
    let controller = Engine::start("controller");
    InputLog::install(&host, MONITOR)?;
    host.pair_for_control(&controller)?;
    controller.connect_services(&host)?;
    host.core.revoke(&controller.endpoint())?;
    host.wait_closed(SessionRole::Host);
    controller.wait_closed(SessionRole::Controller);
    assert!(
        host.core.snapshot()?.devices.is_empty(),
        "a revoked controller is forgotten"
    );
    assert_eq!(
        refusal(&controller, &host)?,
        Error::Unauthorized.to_string(),
        "a revoked controller is refused"
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
/// no grants and ends its session; the controller still dials, and the host
/// refuses a peer it no longer knows.
#[test]
fn unpair_forgets_the_device_stops_the_session_and_blocks_reconnect() -> TestResult {
    let trace = traced();
    let host = Engine::start("host");
    let controller = Engine::start("controller");
    InputLog::install(&host, MONITOR)?;
    host.pair_for_control(&controller)?;
    controller.connect_services(&host)?;
    host.core.unpair(&controller.endpoint())?;
    host.wait_closed(SessionRole::Host);
    controller.wait_closed(SessionRole::Controller);
    assert!(
        host.core.snapshot()?.devices.is_empty(),
        "an unpaired controller is forgotten, not kept without grants"
    );
    assert_eq!(
        refusal(&controller, &host)?,
        Error::Unauthorized.to_string(),
        "the host refuses a controller it no longer knows"
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
    host.pair_for_control(&controller)?;
    assert_eq!(
        controller.connect_services(&host)?.phase,
        SessionPhase::Active,
        "pairing still works afterwards"
    );
    controller.core.unpair(&host.endpoint())?;
    controller.wait_closed(SessionRole::Controller);
    assert!(
        matches!(
            controller.core.begin_connect_services(&host.endpoint()),
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
    host.pair_for_control(&controller)?;
    controller.connect_services(&host)?;
    controller.core.unpair(&host.endpoint())?;
    assert!(
        controller.core.snapshot()?.devices.is_empty(),
        "the controller forgets its host"
    );
    controller.wait_closed(SessionRole::Controller);
    host.wait_closed(SessionRole::Host);
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
          session: the session's control stream ended error=network session=1
          session: session phase from=active generation=1 role=Host session=1 to=closed
    "#]]
    .assert_eq(&trace.story(&["session", "pairing"]));
    Ok(())
}

/// Withdrawing control mid-session keeps the connection on both sides and
/// its Desktop open, to be seen.
#[test]
fn removing_control_mid_session_keeps_the_session_to_be_seen() -> TestResult {
    let trace = traced();
    let host = Engine::start("host");
    let controller = Engine::start("controller");
    InputLog::install(&host, MONITOR)?;
    host.pair_for_control(&controller)?;
    let session = controller.connect_services(&host)?;
    let desktop = controller.open_desktop(&session)?;
    host.core.set_desktop_sharing(Grants {
        view: true,
        control: false,
        clipboard: true,
        transfer: false,
    })?;
    wait_for(
        "the desktop to learn it no longer controls",
        DEFAULT_WAIT,
        || (desktop.control_state().ok()? != DesktopControlState::Controlling).then_some(()),
    );
    assert!(!desktop.is_cancelled(), "the Desktop stays open to be seen");
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
        controller:
          input: control changed epoch=2 service=1 state=Viewing
    "#]]
    .assert_eq(&trace.story(&["input"]));
    Ok(())
}

/// How a connection's desktop loses its host.
#[derive(Clone, Copy, Debug)]
enum Ending {
    StopHosting,
    StopHostedSession,
    Disconnect,
    Revoke,
    Unpair,
    WithdrawControl,
    ControllerShutdown,
}

/// Every way a desktop's connection ends, or its control is withdrawn,
/// lifts what the desktop held on the host, the button then the key, and
/// the desktop takes no input after: none it sends applies.
#[test]
fn each_end_of_a_connection_releases_what_its_desktop_held() -> TestResult {
    let trace = traced();
    for ending in [
        Ending::StopHosting,
        Ending::StopHostedSession,
        Ending::Disconnect,
        Ending::Revoke,
        Ending::Unpair,
        Ending::WithdrawControl,
        Ending::ControllerShutdown,
    ] {
        let host = Engine::start("host");
        let controller = Engine::start("controller");
        let applied = InputLog::install(&host, MONITOR)?;
        host.pair_for_control(&controller)?;
        let connection = controller.connect_services(&host)?;
        let desktop = controller.open_desktop(&connection)?;
        let held = [
            InputEvent::key_down(usage::LEFT_SHIFT),
            InputEvent::button_down(PointerButton::Right),
        ];
        for event in held {
            desktop.submit_input(event)?;
        }
        assert_eq!(
            applied.wait_for(2),
            held,
            "{ending:?}: the desktop's input applies"
        );
        match ending {
            Ending::StopHosting => host.core.stop_hosting()?,
            Ending::StopHostedSession => host.core.stop_hosted_session()?,
            Ending::Disconnect => controller.core.disconnect_session(connection.id)?,
            Ending::Revoke => host.core.revoke(&controller.endpoint())?,
            Ending::Unpair => host.core.unpair(&controller.endpoint())?,
            Ending::WithdrawControl => host.core.set_desktop_sharing(Grants {
                view: true,
                control: false,
                clipboard: false,
                transfer: false,
            })?,
            Ending::ControllerShutdown => controller.core.shutdown(),
        }
        let released = [
            held[0],
            held[1],
            InputEvent::button_up(PointerButton::Right),
            InputEvent::key_up(usage::LEFT_SHIFT),
        ];
        assert_eq!(
            applied.wait_for(4),
            released,
            "{ending:?}: the held button, then the held key, are lifted"
        );
        match ending {
            // Control withdrawn leaves the desktop to be seen: what it
            // sends reaches the host, which discards it.
            Ending::WithdrawControl => {
                wait_for(
                    "the desktop to learn it no longer controls",
                    DEFAULT_WAIT,
                    || {
                        (desktop.control_state().ok()? != DesktopControlState::Controlling)
                            .then_some(())
                    },
                );
                // Counted from here: what earlier endings recorded is theirs.
                let refused = trace.named("input refused").len();
                desktop.submit_input(InputEvent::key_down(usage::B))?;
                trace.wait_for_records("input refused", refused + 1);
            }
            _ => {
                wait_for(
                    &format!("{ending:?}: the desktop to end"),
                    DEFAULT_WAIT,
                    || desktop.is_cancelled().then_some(()),
                );
                assert!(
                    desktop
                        .submit_input(InputEvent::key_down(usage::B))
                        .is_err(),
                    "{ending:?}: an ended desktop takes no input"
                );
            }
        }
        assert_eq!(
            applied.events(),
            released,
            "{ending:?}: nothing sent after the end applies"
        );
    }
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
        host.pair_for_control(peer)?;
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
    host.core
        .set_capture(Box::new(MonitorCapture::new(MONITOR)))?;
    for peer in &peers {
        host.pair_for_control(peer)?;
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
    assert_eq!(
        refusal(&peers[0], &host)?,
        Error::NotHosting.to_string(),
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
