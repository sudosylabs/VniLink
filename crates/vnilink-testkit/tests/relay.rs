// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! Relayed connections alone: two engines with no direct route, so all
//! they exchange crosses a relay on this computer.

use expect_test::expect;

use vnilink_testkit::files::{check, Moving, Way};
use vnilink_testkit::relay::LocalRelay;
use vnilink_testkit::{traced, Pair, TestResult, FILES};

/// A connection between engines without a direct route goes through the
/// relay, says so in its paths, and the binding is recorded.
#[test]
fn a_connection_without_a_direct_route_goes_through_the_relay() -> TestResult {
    let trace = traced();
    let relay = LocalRelay::start();
    let (host, controller) = FILES;
    let pair = Pair::relayed(relay.url(), host, controller);
    let session = pair.controller.connect_services(&pair.host)?;
    let paths = pair.controller.core.controller_session_paths(session.id)?;
    assert!(!paths.is_empty(), "a live connection has a path");
    assert!(
        paths.iter().all(|path| path.relay),
        "without a direct route every path crosses the relay: {paths:?}"
    );
    assert!(relay.carried() > 0, "the relay carries the connection");
    vnilink_testkit::assert_story(
        expect![[r#"
        controller:
          relay: endpoint bound listen=RelayOnly relay=true
        host:
          relay: endpoint bound listen=RelayOnly relay=true
    "#]],
        || trace.story(&["relay"]),
    );
    Ok(())
}

/// A folder uploaded across the relay arrives byte for byte, both sides
/// completing it.
#[test]
fn files_cross_the_relay_exactly() -> TestResult {
    let trace = traced();
    let relay = LocalRelay::start();
    let (host, controller) = FILES;
    let pair = Pair::relayed(relay.url(), host, controller);
    std::fs::create_dir_all(pair.host.folder().join("inbox"))?;
    check(
        &trace,
        &pair,
        Moving {
            way: Way::Upload,
            files: &[("a.txt", 1024), ("b/c.bin", 3 * 1024 * 1024 + 11)],
            folders: &[],
            from: "set",
            to: "inbox",
            drivers: true,
        },
        expect![[r#"
            controller:
              files: job state from=preparing job=1 sender=true to=serving
              files: file served ended=sent from=0 sent=1024 size=1024
              files: file served ended=sent from=0 sent=3145739 size=3145739
              files: job state done_bytes=3146763 files_done=2 from=serving job=1 sender=true to=complete
            host:
              files: file received outcome=received resumed_at=0 size=1024 took=1024
              files: file received outcome=received resumed_at=0 size=3145739 took=3145739
              files: received files flushed job=1
              files: job state done_bytes=3146763 files_done=2 from=transferring job=1 sender=false to=complete
        "#]],
    )?;
    assert!(relay.carried() > 0, "the files crossed the relay");
    Ok(())
}
