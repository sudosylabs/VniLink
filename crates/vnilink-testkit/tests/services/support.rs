// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! What the services scenarios share: a service's phase on the controller.
//! Waits on sessions, the closes an engine published, paths inside a
//! host's folder and a host offering its shell are the kit's
//! ([`vnilink_testkit::session`], [`vnilink_testkit::events`],
//! [`vnilink_testkit::files::Location`], [`vnilink_testkit::terminal`]).

use vnilink::{Error, ServiceInfo, ServicePhase};
use vnilink_testkit::Engine;

/// The phase of `service` on the controller now.
pub(super) fn phase(controller: &Engine, service: &ServiceInfo) -> Result<ServicePhase, Error> {
    Ok(controller.core.controller_service(service.id)?.phase)
}
