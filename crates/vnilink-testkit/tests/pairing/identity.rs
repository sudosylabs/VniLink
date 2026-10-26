//! An engine's identity toward other computers: how a new one starts, the
//! identity it can be started from, what a stopped engine still says and
//! refuses, and the ids it will not take for a peer, its own included.

use expect_test::expect;
use std::time::Duration;

use vnilink::{
    CoreConfig, CoreEvent, EncodedFrame, Error, Grants, InputEvent, Quality, SessionRole,
};
use vnilink_testkit::desktop::{InputLog, MonitorCapture};
use vnilink_testkit::relay::LocalRelay;
use vnilink_testkit::store::start_storeless;
use vnilink_testkit::{traced, Engine, TestResult, SEEN_AND_CONTROLLED};

/// Shutting down twice is the same as once: the engine still says who it
/// is, and every call that would reach another computer or the desktop
/// says it is shut down.
#[test]
fn shutdown_is_idempotent_and_later_calls_report_it() -> TestResult {
    let _trace = traced();
    let engine = Engine::start("host");
    let other = Engine::start("controller");
    let core = &engine.core;
    let token = other.core.create_invitation(Duration::from_secs(60))?;
    let id = core.endpoint_id();
    let secret = core.export_identity();
    core.shutdown();
    core.shutdown();
    assert_eq!(core.endpoint_id(), id, "a stopped engine keeps its id");
    assert!(
        core.export_identity() == secret,
        "a stopped engine keeps its secret"
    );
    let shut = |result: Result<(), Error>, call: &str| {
        assert!(
            matches!(result, Err(Error::ShutDown)),
            "{call} after shutdown says so: {result:?}"
        );
    };
    shut(core.snapshot().map(drop), "snapshot");
    shut(core.subscribe().map(drop), "subscribe");
    shut(core.start_hosting(), "start_hosting");
    shut(core.stop_hosting(), "stop_hosting");
    shut(core.stop_hosted_session(), "stop_hosted_session");
    shut(
        core.create_invitation(Duration::from_secs(60)).map(drop),
        "create_invitation",
    );
    shut(
        core.accept_invitation(&token).map(drop),
        "accept_invitation",
    );
    shut(core.connect(&other.endpoint()).map(drop), "connect");
    shut(core.disconnect(), "disconnect");
    shut(
        core.submit_input(1, 1, InputEvent::pointer(0, 0, 1, 1)),
        "submit_input",
    );
    shut(InputLog::install(&engine, (0, 0)).map(drop), "set_desktop");
    shut(
        core.set_desktop_sharing(Grants::default()),
        "set_desktop_sharing",
    );
    shut(core.revoke(&other.endpoint()), "revoke");
    shut(
        core.update_session_quality(1, SessionRole::Host, Quality::new(1, 1, 1, 1)?),
        "update_session_quality",
    );
    shut(
        core.publish_video(1, EncodedFrame::h264(true, 0, vec![1])?),
        "publish_video",
    );
    shut(core.subscribe_video().map(drop), "subscribe_video");
    shut(
        core.set_capture(Box::new(MonitorCapture::new((1, 1)))),
        "set_capture",
    );
    Ok(())
}

/// A host knows nobody it was not paired with, and refuses ids that are
/// not ones, to connect to or to revoke.
#[test]
fn unknown_devices_and_garbage_ids_are_rejected() -> TestResult {
    let _trace = traced();
    let host = Engine::start("host");
    let stranger = Engine::start("stranger");
    host.core.set_desktop_sharing(SEEN_AND_CONTROLLED)?;
    assert!(
        host.core.snapshot()?.devices.is_empty(),
        "sharing pairs nobody"
    );
    assert!(
        matches!(
            host.core.revoke(&stranger.endpoint()),
            Err(Error::NotPaired)
        ),
        "a stranger cannot be revoked"
    );
    for id in ["", "not-a-key", "zzzz"] {
        assert!(
            matches!(host.core.connect(id), Err(Error::InvalidSetting(_))),
            "connecting to {id:?} is refused"
        );
        assert!(
            matches!(host.core.revoke(id), Err(Error::InvalidSetting(_))),
            "revoking {id:?} is refused"
        );
    }
    Ok(())
}

/// A new engine is idle: it hosts nothing, knows nobody, has no session,
/// uses the public relays, and offers no keyboard, pointer or capture.
#[test]
fn a_fresh_engine_starts_idle() -> TestResult {
    let _trace = traced();
    let engine = Engine::start("host");
    let snapshot = engine.core.snapshot()?;
    assert_eq!(snapshot.endpoint_id, engine.endpoint(), "it says who it is");
    assert!(snapshot.public_relay, "it uses the public relays");
    assert!(snapshot.relay_urls.is_empty(), "and no relay of its own");
    assert!(!snapshot.hosting, "it hosts nothing");
    assert!(snapshot.devices.is_empty(), "it knows nobody");
    assert!(snapshot.incoming.is_none(), "it has no incoming session");
    assert!(snapshot.outgoing.is_none(), "nor an outgoing one");
    let capabilities = snapshot.capabilities;
    assert!(!capabilities.keyboard, "it offers no keyboard");
    assert!(!capabilities.pointer, "nor a pointer");
    assert!(!capabilities.desktop_capture, "nor capture");
    assert_eq!(capabilities.monitors, 0, "nor a display");
    Ok(())
}

/// An engine holds between 1 and 1024 events for a reader; one that holds
/// one keeps the first it published and counts the next as lost.
#[test]
fn event_capacity_must_be_between_1_and_1024() -> TestResult {
    let _trace = traced();
    let relay = LocalRelay::start();
    assert!(
        matches!(
            start_storeless("host", &relay, None, 0).map(drop),
            Err(Error::InvalidSetting("event capacity must be at least 1"))
        ),
        "no engine holds no events"
    );
    assert!(
        matches!(
            start_storeless("host", &relay, None, 1025).map(drop),
            Err(Error::InvalidSetting("event capacity must be at most 1024"))
        ),
        "no engine holds more than 1024"
    );
    let core = start_storeless("host", &relay, None, 1)?;
    let events = core.subscribe()?;
    core.start_hosting()?;
    core.stop_hosting()?;
    assert!(!core.snapshot()?.hosting, "hosting stopped");
    assert_eq!(events.lagged(), 1, "the second event did not fit");
    assert_eq!(
        events.try_recv(),
        Some(CoreEvent::HostingChanged { hosting: true }),
        "the first event is kept"
    );
    core.shutdown();
    start_storeless("host", &relay, None, 1024)?.shutdown();
    Ok(())
}

/// An engine started from an exported identity is the same computer to
/// its peers; a new engine is another; the identity is never shown.
#[test]
fn restored_identity_keeps_the_same_endpoint_id() -> TestResult {
    let _trace = traced();
    let relay = LocalRelay::start();
    let original = start_storeless("host", &relay, None, 64)?;
    let bytes = original.export_identity();
    let id = original.endpoint_id();
    original.shutdown();
    drop(original);
    let restored = start_storeless("host", &relay, Some(bytes), 64)?;
    assert!(
        restored.export_identity() == bytes,
        "the restored engine holds the identity it was given"
    );
    assert_eq!(restored.endpoint_id(), id, "and is the same computer");
    let other = start_storeless("other", &relay, None, 64)?;
    assert_ne!(other.endpoint_id(), id, "a new engine is another computer");
    let rendered = format!(
        "{:?}",
        CoreConfig {
            identity: Some([0x11; 32]),
            ..CoreConfig::default()
        }
    );
    assert!(
        rendered.contains("[redacted]"),
        "an identity shows redacted"
    );
    assert!(!rendered.contains("17"), "and never its bytes: {rendered}");
    restored.shutdown();
    other.shutdown();
    Ok(())
}

/// A computer cannot accept its own invitation.
#[test]
fn an_installation_cannot_pair_with_itself() -> TestResult {
    let trace = traced();
    let engine = Engine::start("host");
    let token = engine.core.create_invitation(Duration::from_secs(60))?;
    assert!(
        matches!(
            engine.core.accept_invitation(&token),
            Err(Error::InvalidInvitation(
                "an installation cannot pair with itself"
            ))
        ),
        "its own invitation is refused"
    );
    assert!(
        engine.core.snapshot()?.devices.is_empty(),
        "and remembers nobody"
    );
    expect![[r#"
        host:
          pairing: invitation issued direct=1 relays=0 ttl_s=60
          pairing: invitation accepted reason=an installation cannot pair with itself refused=invalid invitation
    "#]].assert_eq(&trace.story(&["pairing"]));
    Ok(())
}
