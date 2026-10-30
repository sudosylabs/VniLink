// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! System actions alone: a controller asking the host to lock, restart,
//! shut down or keep its screen private, under the host's policy; and the
//! host and controllers keeping their computers awake, or locking the host,
//! while a desktop is open.

#[path = "system/actions.rs"]
mod actions;
#[path = "system/power.rs"]
mod power;
#[path = "system/privacy.rs"]
mod privacy;
#[path = "system/restoring.rs"]
mod restoring;

use std::sync::Arc;

use vnilink::{Error, HostSystemActions, ServiceKind, SystemPolicy};
use vnilink_testkit::Pair;

/// A host sharing its system actions, with `system` as its operating
/// system, and a controller paired with it.
fn sharing(system: &Arc<impl HostSystemActions + 'static>) -> Result<Pair, Error> {
    let pair = Pair::for_services(&[ServiceKind::System]);
    pair.host
        .core
        .set_system_actions(Some(Arc::clone(system) as Arc<dyn HostSystemActions>))?;
    Ok(pair)
}

/// The host's policy allows privacy and nothing else.
const PRIVACY: SystemPolicy = SystemPolicy {
    allow_lock: true,
    allow_restart: false,
    allow_shutdown: false,
    allow_privacy: true,
};
