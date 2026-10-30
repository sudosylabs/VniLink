// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! The host's installation-wide consent to share each service.

use vnilink::{SharingPolicy, SharingPolicyUpdate};
use vnilink_testkit::{traced, Engine, TestResult};

/// Two partial changes made at once each change only their own switch, so
/// neither undoes the other.
#[test]
fn concurrent_partial_changes_keep_each_others_switches() -> TestResult {
    let _trace = traced();
    let host = Engine::start("host");
    let core = &host.core;
    std::thread::scope(|scope| -> TestResult {
        let terminal = scope.spawn(|| {
            core.change_sharing_policy(SharingPolicyUpdate {
                terminal: Some(true),
                ..Default::default()
            })
        });
        let camera = scope.spawn(|| {
            core.change_sharing_policy(SharingPolicyUpdate {
                camera: Some(true),
                ..Default::default()
            })
        });
        terminal.join().expect("a change")?;
        camera.join().expect("a change")?;
        Ok(())
    })?;
    assert_eq!(
        core.sharing_policy()?,
        SharingPolicy {
            terminal: true,
            camera: true,
            ..Default::default()
        },
        "both changes hold and nothing else changed"
    );
    Ok(())
}
