// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! What the suites read of the events an engine publishes
//! ([`Core::subscribe`](vnilink::Core::subscribe)): the next one that
//! matters, or all it holds now.

use std::time::Duration;

use vnilink::{CloseCause, CoreEvent, EventSubscription, ServiceInfo, ServiceKind, ServicePhase};

use crate::{wait_for, DEFAULT_WAIT};

/// The first event `events` publishes within `within` that `pick` gives a
/// value for, and that value; fails naming `what`. Events it passes over
/// are taken.
pub fn next_picked<T>(
    events: &EventSubscription,
    what: &str,
    within: Duration,
    mut pick: impl FnMut(CoreEvent) -> Option<T>,
) -> T {
    wait_for(what, within, || {
        events
            .recv_timeout(Duration::from_millis(20))
            .and_then(&mut pick)
    })
}

/// The first event `events` publishes within `within` that `wanted` picks;
/// fails naming `what`.
pub fn next_event(
    events: &EventSubscription,
    what: &str,
    within: Duration,
    wanted: impl Fn(&CoreEvent) -> bool,
) -> CoreEvent {
    next_picked(events, what, within, |event| {
        wanted(&event).then_some(event)
    })
}

/// The events `events` holds now, taken.
pub fn drain(events: &EventSubscription) -> Vec<CoreEvent> {
    std::iter::from_fn(|| events.try_recv()).collect()
}

/// The controller-side service changes `events` holds now, in order.
pub fn service_changes(events: &EventSubscription) -> Vec<ServiceInfo> {
    drain(events)
        .into_iter()
        .filter_map(|event| match event {
            CoreEvent::ServiceChanged { service } => Some(service),
            _ => None,
        })
        .collect()
}

/// The hosted services of `kind` that `events` says closed for `cause`,
/// from what it holds now.
pub fn hosted_closes(
    events: &EventSubscription,
    kind: ServiceKind,
    cause: CloseCause,
) -> Vec<ServiceInfo> {
    drain(events)
        .into_iter()
        .filter_map(|event| match event {
            CoreEvent::HostedServiceChanged { service }
                if service.kind == kind && service.phase == ServicePhase::Closed(cause) =>
            {
                Some(service)
            }
            _ => None,
        })
        .collect()
}

/// Waits until `events` says a hosted service of `kind` closed for `cause`.
pub fn wait_hosted_close(
    events: &EventSubscription,
    kind: ServiceKind,
    cause: CloseCause,
) -> ServiceInfo {
    wait_for(
        &format!("the host to publish its {kind:?} close for {cause:?}"),
        DEFAULT_WAIT,
        || hosted_closes(events, kind, cause).into_iter().next(),
    )
}
