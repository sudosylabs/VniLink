// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! Presence links: one idle neutral connection to every paired computer.
//!
//! A computer is reachable when this side holds a live neutral link to it.
//! The link is the connection services open their streams on, so
//! "connected" means "I can use this computer now". It carries no service
//! by itself, and the host does not count it as someone connected.
//! The supervisor redials with backoff, sooner when the peer dials us or the
//! user asks for the link, and stops for unpaired computers.
//!
//! The link's session phase maps to [`DevicePresence`]: active is connected,
//! reconnecting is reconnecting, and a link that ended is disconnected. A
//! host that refused the link because it stopped hosting reports
//! [`DevicePresence::NotAccepting`] until a link is active again.
// trace: records every ignored failure

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use rand::RngExt;
use tokio::sync::Notify;

use super::connect::start_outgoing;
use super::{finished, lock, Core, Shared};
use crate::error::Error;
use crate::events::CoreEvent;
use crate::session::SessionPhase;
use crate::DevicePresence;

/// Waits between dials to an unreachable computer, the last one repeating.
/// A computer that is off for a night is dialled every ten minutes, not
/// every minute; it is dialled at once when it dials this one or the user
/// asks for the link.
const BACKOFF: [Duration; 8] = [
    Duration::from_secs(2),
    Duration::from_secs(5),
    Duration::from_secs(15),
    Duration::from_secs(30),
    Duration::from_secs(60),
    Duration::from_secs(150),
    Duration::from_secs(300),
    Duration::from_secs(600),
];

/// Share of each wait that is random, so computers that lost the same link
/// together do not dial together.
const JITTER: f64 = 0.2;

/// The wait before the next dial after `failures` failures in a row.
/// `spread` is a number in 0..1 that places the wait within its jitter.
fn backoff(failures: usize, spread: f64) -> Duration {
    let base = BACKOFF[failures.saturating_sub(1).min(BACKOFF.len() - 1)];
    base.mul_f64(1.0 - JITTER + 2.0 * JITTER * spread.clamp(0.0, 1.0))
}

/// The longest the paired-device list goes without being reconciled with
/// running links. A change to it, or to whether links run, is noticed at once.
const RECONCILE: Duration = Duration::from_secs(60);

impl Core {
    /// Keeps a neutral link to every paired computer so presence means
    /// reachability. The application turns this on; embedders and tests that
    /// dial explicitly leave it off. Turning it off stops supervising but
    /// leaves open links as ordinary neutral sessions.
    pub fn set_presence_links(&self, enabled: bool) -> Result<(), Error> {
        let _engine = self.engine();
        {
            let mut state = self.alive_state()?;
            if state.presence_links == enabled {
                return Ok(());
            }
            state.presence_links = enabled;
            if !enabled {
                state.presence.clear();
            }
        }
        if enabled {
            let shared = Arc::clone(&self.shared);
            let handle = self.runtime.handle().clone();
            self.runtime.spawn(supervise(shared, handle));
        } else {
            self.shared.presence_reconcile.notify_one();
        }
        Ok(())
    }
}

/// Dials the link to `peer` now instead of waiting out its backoff.
pub(super) fn wake_link(shared: &Shared, peer: &[u8; 32]) {
    if let Some(wake) = lock(&shared.presence_wake).get(peer) {
        wake.notify_one();
    }
}

fn running(shared: &Shared) -> bool {
    let state = lock(&shared.state);
    state.presence_links && !state.shut_down
}

async fn supervise(shared: Arc<Shared>, handle: tokio::runtime::Handle) {
    let mut links: HashMap<[u8; 32], tokio::task::JoinHandle<()>> = HashMap::new();
    while running(&shared) {
        let paired: Vec<[u8; 32]> = lock(&shared.state)
            .trust
            .devices()
            .map(|device| device.endpoint_id)
            .collect();
        links.retain(|peer, task| {
            let keep = paired.contains(peer) && !task.is_finished();
            if !keep {
                task.abort();
            }
            keep
        });
        for peer in paired {
            links.entry(peer).or_insert_with(|| {
                handle.spawn(keep_link(Arc::clone(&shared), handle.clone(), peer))
            });
        }
        tokio::select! {
            _ = shared.presence_reconcile.notified() => {}
            _ = tokio::time::sleep(RECONCILE) => {}
        }
    }
    for (_, task) in links {
        task.abort();
    }
    lock(&shared.presence_wake).clear();
}

/// Holds the link to one paired computer for as long as it stays paired.
async fn keep_link(shared: Arc<Shared>, handle: tokio::runtime::Handle, peer: [u8; 32]) {
    let wake = Arc::new(Notify::new());
    lock(&shared.presence_wake).insert(peer, Arc::clone(&wake));
    let _ended = LinkEnded {
        shared: Arc::clone(&shared),
        peer,
        wake: Arc::clone(&wake),
    };
    let mut failures = 0usize;
    let mut reached = false;
    while running(&shared) {
        let Some(display_id) = lock(&shared.state)
            .trust
            .get(&peer)
            .map(|device| device.display_id.clone())
        else {
            break;
        };
        if !reached && failures == 0 {
            set_presence(&shared, &display_id, DevicePresence::Connecting);
        }
        match start_outgoing(&shared, &handle, &display_id) {
            Ok(info) => {
                if watch_link(&shared, info.id, &peer, &display_id).await {
                    reached = true;
                    failures = 0;
                }
                release_link(&shared, info.id);
            }
            Err(Error::NotPaired | Error::ShutDown) => break,
            Err(error) => tracing::debug!(
                target: crate::trace::RELAY,
                peer = crate::diagnostics::fingerprint(&peer),
                failure = error.name(),
                reason = error.reason(),
                "a link to a paired computer could not start"
            ),
        }
        if !running(&shared) {
            break;
        }
        // A computer forgotten while its link was up is not tried again.
        if lock(&shared.state).trust.get(&peer).is_none() {
            tracing::debug!(
                target: crate::trace::RELAY,
                peer = crate::diagnostics::fingerprint(&peer),
                "a link to a computer no longer paired ended"
            );
            break;
        }
        let (refused, unauthorized) = {
            let state = lock(&shared.state);
            (
                state.link_refused.contains(&peer),
                state.link_unauthorized.contains(&peer),
            )
        };
        set_presence(
            &shared,
            &display_id,
            if refused {
                DevicePresence::NotAccepting
            } else {
                DevicePresence::Disconnected
            },
        );
        if unauthorized {
            // The peer answered that this computer may not link to it. Asking
            // again changes nothing until the peer or the user does.
            tracing::debug!(
                target: crate::trace::RELAY,
                peer = crate::diagnostics::fingerprint(&peer),
                "a paired computer refused the link: not dialling it until it dials or the user asks"
            );
            wake.notified().await;
            lock(&shared.state).link_unauthorized.remove(&peer);
            failures = 0;
            continue;
        }
        failures += 1;
        let wait = backoff(failures, rand::rng().random::<f64>());
        tracing::debug!(
            target: crate::trace::RELAY,
            peer = crate::diagnostics::fingerprint(&peer),
            refused,
            failures,
            retry_ms = u64::try_from(wait.as_millis()).unwrap_or(u64::MAX),
            "a link to a paired computer is down; trying again"
        );
        tokio::select! {
            _ = tokio::time::sleep(wait) => {}
            _ = wake.notified() => {
                failures = 0;
            }
        }
    }
}

/// What a computer's link leaves behind when its task ends, however it
/// ends: the supervisor aborts the task of a computer that is unpaired.
struct LinkEnded {
    shared: Arc<Shared>,
    peer: [u8; 32],
    wake: Arc<Notify>,
}

impl Drop for LinkEnded {
    fn drop(&mut self) {
        let (shared, peer) = (&self.shared, self.peer);
        let mut state = lock(&shared.state);
        if let Some(device) = state.trust.get(&peer).map(|d| d.display_id.clone()) {
            state.presence.remove(&device);
        }
        state.link_refused.remove(&peer);
        state.link_unauthorized.remove(&peer);
        drop(state);
        let mut wakes = lock(&shared.presence_wake);
        if wakes.get(&peer).is_some_and(|w| Arc::ptr_eq(w, &self.wake)) {
            wakes.remove(&peer);
        }
    }
}

/// Drops the record of a link that has ended. Every redial makes a new
/// session; kept, an unreachable computer would add one a minute to the
/// controller's session list until the list was too long to read. A service
/// still running on it keeps the record until that service has ended.
fn release_link(shared: &Shared, id: u64) {
    let mut state = lock(&shared.state);
    super::services::synchronize(&mut state, &shared.events);
    let ended = state
        .outgoing
        .get(&id)
        .is_some_and(|session| finished(&session.phase));
    let in_use = state
        .services
        .values()
        .any(|record| record.info.connection_id == id && !record.info.phase.finished());
    if ended && !in_use {
        state.outgoing.remove(&id);
    }
}

/// Follows one link's session until it ends, waking when its phase moves.
/// Returns whether it was ever up.
async fn watch_link(shared: &Shared, id: u64, peer: &[u8; 32], display_id: &str) -> bool {
    let Some(mut changed) = lock(&shared.state)
        .outgoing
        .get(&id)
        .map(|session| session.changed.subscribe())
    else {
        return false;
    };
    let mut reached = false;
    loop {
        let phase = lock(&shared.state)
            .outgoing
            .get(&id)
            .map(|session| session.phase.clone());
        match phase {
            None => return reached,
            Some(phase) if finished(&phase) => return reached,
            Some(SessionPhase::Active) => {
                if !reached {
                    // How the link reaches the peer: directly or through a relay.
                    let link = lock(&shared.state)
                        .outgoing
                        .get(&id)
                        .and_then(|session| session.link.clone());
                    let (relayed, paths) = link.map_or((None, 0), |link| {
                        let paths = link.paths();
                        let relayed = paths
                            .iter()
                            .find(|path| path.is_selected())
                            .map(|path| path.is_relay());
                        (relayed, paths.iter().count())
                    });
                    tracing::info!(
                        target: crate::trace::RELAY,
                        peer = crate::diagnostics::fingerprint(peer),
                        relayed,
                        paths,
                        "a link to a paired computer is up"
                    );
                }
                reached = true;
                {
                    let mut state = lock(&shared.state);
                    state.link_refused.remove(peer);
                    state.link_unauthorized.remove(peer);
                }
                set_presence(shared, display_id, DevicePresence::Connected);
            }
            Some(SessionPhase::Reconnecting) => {
                set_presence(shared, display_id, DevicePresence::Reconnecting);
            }
            Some(_) => {}
        }
        if !running(shared) {
            return reached;
        }
        // The session is dropped from the record when its link is released,
        // which ends the wait.
        if changed.changed().await.is_err() {
            return reached;
        }
    }
}

fn set_presence(shared: &Shared, display_id: &str, presence: DevicePresence) {
    let mut state = lock(&shared.state);
    if !state.presence_links
        || state.presence.insert(display_id.to_owned(), presence) == Some(presence)
    {
        return;
    }
    tracing::debug!(
        target: crate::trace::RELAY,
        peer = super::parse_endpoint_id(display_id)
            .map(|key| crate::diagnostics::fingerprint(&key))
            .unwrap_or_default(),
        presence = ?presence,
        "presence"
    );
    // Published under the state, so no reader sees a presence before its
    // change is published.
    shared.events.publish(CoreEvent::PresenceChanged {
        endpoint_id: display_id.to_owned(),
        presence,
    });
}

impl Core {
    /// What a paired computer last reported it allows, from the live link to
    /// it. `None` until a link has delivered a report; links drop their
    /// report when they end, so an unreachable computer reports nothing.
    pub fn peer_availability(
        &self,
        endpoint_id: &str,
    ) -> Result<Option<Vec<(crate::Capability, crate::RemoteStatus)>>, Error> {
        let _engine = self.engine();
        let key = super::parse_endpoint_id(endpoint_id)?;
        let state = self.alive_state()?;
        if state.trust.get(&key).is_none() {
            return Err(Error::NotPaired);
        }
        Ok(state
            .outgoing
            .values()
            .find(|s| s.peer_key == key && s.phase == SessionPhase::Active)
            .and_then(|s| s.remote_availability.clone()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn waits_grow_to_ten_minutes_and_stay_within_their_jitter() {
        assert_eq!(backoff(1, 0.5), BACKOFF[0]);
        assert_eq!(backoff(50, 0.5), Duration::from_secs(600));
        let (low, high) = (backoff(7, 0.0), backoff(7, 1.0));
        assert_eq!(low, Duration::from_secs(240));
        assert_eq!(high, Duration::from_secs(360));
        assert!(backoff(3, 7.0) <= BACKOFF[2].mul_f64(1.0 + JITTER));
    }
}
