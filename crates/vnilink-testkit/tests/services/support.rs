// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! What the services scenarios share: a service's phase on the controller.
//! Waits on sessions, the closes an engine published and paths inside a
//! host's folder are the kit's ([`vnilink_testkit::session`],
//! [`vnilink_testkit::events`], [`vnilink_testkit::files::Location`]).

use vnilink::{ServiceInfo, ServicePhase};
use vnilink_testkit::Engine;

/// The phase of `service` on the controller now.
pub(super) fn phase(controller: &Engine, service: &ServiceInfo) -> ServicePhase {
    controller
        .core
        .controller_service(service.id)
        .expect("the controller keeps the service's record")
        .phase
}
