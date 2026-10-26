//! What one engine says of sessions it does not have.

use vnilink::Error;
use vnilink_testkit::{traced, Engine, TestResult};

#[test]
fn an_unknown_session_has_no_paths() -> TestResult {
    let _trace = traced();
    let engine = Engine::start("engine");
    assert!(
        matches!(engine.core.session_paths(999), Err(Error::NoSession)),
        "the paths of a session the engine never had are no session"
    );
    Ok(())
}
