//! Presence links: one idle neutral connection to every paired computer.
//!
//! A computer is reachable when this side holds a live neutral link to it.
//! The link is the same `services_only` connection services open their
//! streams on, so "connected" means "I can use this computer now". It carries
//! no service by itself, and the host does not count it as someone connected.
//! The supervisor redials with backoff, sooner when the peer dials us or the
//! user asks for the link, and stops for unpaired computers.
// trace: records every ignored failure

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::Notify;

use super::connect::start_outgoing;
use super::{finished, lock, Core, Shared};
use crate::error::Error;
use crate::events::CoreEvent;
use crate::session::SessionPhase;
use crate::DevicePresence;

/// Waits between dials to an unreachable computer, the last one repeating.
const BACKOFF: [Duration; 5] = [
    Duration::from_secs(2),
    Duration::from_secs(5),
    Duration::from_secs(15),
    Duration::from_secs(30),
    Duration::from_secs(60),
];

/// How often a link's session record is read for phase changes.
const WATCH: Duration = Duration::from_millis(250);

/// How often the paired-device list is reconciled with running links.
const RECONCILE: Duration = Duration::from_secs(1);

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
        tokio::time::sleep(RECONCILE).await;
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
        match start_outgoing(&shared, &handle, &display_id, true, true, false) {
            Ok((info, _ready, _stop)) => {
                if watch_link(&shared, info.id, &peer, &display_id).await {
                    reached = true;
                    failures = 0;
                }
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
        let refused = lock(&shared.state).link_refused.contains(&peer);
        set_presence(
            &shared,
            &display_id,
            if refused {
                DevicePresence::NotAccepting
            } else {
                DevicePresence::Disconnected
            },
        );
        failures += 1;
        let wait = BACKOFF[(failures - 1).min(BACKOFF.len() - 1)];
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
    let mut state = lock(&shared.state);
    if let Some(device) = state.trust.get(&peer).map(|d| d.display_id.clone()) {
        state.presence.remove(&device);
    }
    state.link_refused.remove(&peer);
    drop(state);
    let mut wakes = lock(&shared.presence_wake);
    if wakes.get(&peer).is_some_and(|w| Arc::ptr_eq(w, &wake)) {
        wakes.remove(&peer);
    }
}

/// Follows one link's session until it ends. Returns whether it was ever up.
async fn watch_link(shared: &Shared, id: u64, peer: &[u8; 32], display_id: &str) -> bool {
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
                lock(&shared.state).link_refused.remove(peer);
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
        tokio::time::sleep(WATCH).await;
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
            .find(|s| s.peer_key == key && s.services_only && s.phase == SessionPhase::Active)
            .and_then(|s| s.remote_availability.clone()))
    }
}
