// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

use crate::MacSystemActions;
use vnilink::system::HostSystemActions;

#[test]
fn system_support_reports_lock_only_with_unknown_session_state() {
    // Never invokes lock_workstation here: that would lock the test machine.
    let actions = MacSystemActions::default();
    let support = actions.support();
    assert!(support.lock);
    assert!(support.restart && support.shutdown);
    assert!(support.privacy && support.input_block);
    let session = actions.session_state();
    assert_eq!(session.session, "aqua");
    assert_eq!(session.locked, None);
    assert_eq!(session.login_screen, None);
    assert!(!session.headless);
    assert_eq!(actions.observed_privacy(), Some((false, false)));
    // Never perform real restart, shutdown or privacy changes in a unit test.
}
