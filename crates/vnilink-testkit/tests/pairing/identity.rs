//! An engine's identity toward other computers: what a stopped engine
//! still says and refuses, and the ids it will not take for a peer.

use std::time::Duration;

use vnilink::{EncodedFrame, Error, Grants, InputEvent, Quality, SessionRole};
use vnilink_testkit::desktop::{InputLog, MonitorCapture};
use vnilink_testkit::{traced, Engine, TestResult};

use super::support::SEEN_AND_CONTROLLED;

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
        core.set_capture(Box::new(MonitorCapture((1, 1)))),
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
