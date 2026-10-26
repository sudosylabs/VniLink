//! Relayed connections alone: two engines with no direct route, so all
//! they exchange crosses a relay on this computer.

use expect_test::expect;
use std::time::Duration;

use vnilink::JobState;
use vnilink_testkit::files::{drive, finished, received, tree, write_files};
use vnilink_testkit::relay::LocalRelay;
use vnilink_testkit::{traced, Pair, FILES};

/// A connection between engines without a direct route goes through the
/// relay, says so in its paths, and the binding is recorded.
#[test]
fn a_connection_without_a_direct_route_goes_through_the_relay() {
    let trace = traced();
    let relay = LocalRelay::start();
    let (host, controller) = FILES;
    let pair = Pair::relayed(relay.url(), host, controller);
    let session = pair.connect_services();
    let paths = pair
        .controller
        .core
        .controller_session_paths(session.id)
        .unwrap();
    assert!(!paths.is_empty(), "no path");
    assert!(
        paths.iter().all(|path| path.relay),
        "a direct path: {paths:?}"
    );
    assert!(relay.carried() > 0, "the relay carried nothing");
    expect![[r#"
        engine:
          relay: endpoint bound listen=RelayOnly relay=true
          relay: endpoint bound listen=RelayOnly relay=true
    "#]]
    .assert_eq(&trace.story(&["relay"]));
}

/// Files cross the relay exactly.
#[test]
fn files_cross_the_relay_exactly() {
    let _trace = traced();
    let relay = LocalRelay::start();
    let (host, controller) = FILES;
    let pair = Pair::relayed(relay.url(), host, controller);
    drive(&pair);
    write_files(
        &pair.controller.root().join("set"),
        &[("a.txt", 1024), ("b/c.bin", 3 * 1024 * 1024 + 11)],
    );
    std::fs::create_dir_all(pair.host.root().join("inbox")).unwrap();
    pair.controller.core.connect(&pair.host.endpoint()).unwrap();
    pair.controller
        .core
        .job_submit_upload(
            &pair.host.endpoint(),
            pair.controller.root_id(),
            "set",
            pair.host.root_id(),
            "inbox",
        )
        .unwrap();
    let job = received(&pair.host.core);
    let done = finished(&pair.host.core, job, Duration::from_secs(120));
    assert_eq!(done.state, JobState::Complete, "{done:?}");
    assert_eq!(
        tree(&pair.host.root().join("inbox/set")),
        tree(&pair.controller.root().join("set"))
    );
}
