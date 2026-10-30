// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

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
