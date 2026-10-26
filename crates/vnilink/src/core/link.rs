//! The live link and the address refresh both roles share.
//!
//! The host and the controller each store the connection on the session and
//! exchange home relay URLs and direct addresses on the authenticated control
//! stream. The receiver keeps that list for that peer only; a later dial,
//! including a reconnect after a dropped link, uses the stored list.
// trace: records every ignored failure

use super::*;

use super::wire::write_frame;

use std::net::SocketAddr;

use iroh::{Endpoint, EndpointId, TransportAddr};

use crate::address::Announcement;
use crate::error::Error;
use crate::events::CoreEvent;
use crate::invitation::canonicalize_addrs;
use crate::protocol::ControlMessage;

/// The relay URLs and direct sockets the endpoint currently sees for `peer`.
pub(super) async fn peer_addresses(
    endpoint: &Endpoint,
    peer: EndpointId,
) -> (Vec<String>, Vec<SocketAddr>) {
    let Some(info) = endpoint.remote_info(peer).await else {
        return (Vec::new(), Vec::new());
    };
    let mut relays = Vec::new();
    let mut direct = Vec::new();
    for addr in info.addrs() {
        match addr.addr() {
            TransportAddr::Relay(url) => {
                if relays.len() >= crate::relay::MAX_CUSTOM_RELAYS {
                    continue;
                }
                let Ok(parsed) = crate::relay::parse_relay_url(url.as_str()) else {
                    continue;
                };
                let text = parsed.to_string();
                if !relays.contains(&text) {
                    relays.push(text);
                }
            }
            TransportAddr::Ip(socket) => direct.push(*socket),
            _ => {}
        }
    }
    let direct = canonicalize_addrs(&direct).unwrap_or_default();
    (relays, direct)
}

pub(super) fn remember_link(shared: &Shared, session_id: u64, link: iroh::endpoint::Connection) {
    let mut state = lock(&shared.state);
    if let Some(session) = state.incoming.get_mut(&session_id) {
        session.link = Some(link);
    }
}

pub(super) fn remember_peer_addresses(
    shared: &Shared,
    peer_key: &[u8; 32],
    announcement: Announcement,
) -> Result<(), Error> {
    let mut state = lock(&shared.state);
    let Some(mut device) = state.trust.get(peer_key).cloned() else {
        return Ok(());
    };
    if crate::address::unchanged(&device, &announcement) {
        return Ok(());
    }
    let previous = device.clone();
    crate::address::install(&mut device, &announcement)?;
    let endpoint_id = device.display_id.clone();
    state.trust.pair(device)?;
    if let Err(error) = persist(shared, &mut state) {
        if let Err(restore) = state.trust.pair(previous) {
            tracing::warn!(
                target: crate::trace::RELAY,
                failure = restore.name(),
                "the peer's previous addresses could not be put back after a failed save: memory holds the new one"
            );
        }
        return Err(error);
    }
    drop(state);
    shared
        .events
        .publish(CoreEvent::AddressesUpdated { endpoint_id });
    Ok(())
}

/// Stores a peer-reported display name for a paired device. Unknown peers
/// are ignored, like unknown address announcements: a name never creates
/// trust. An absent name keeps the previous one; unpairing drops it with
/// the device. Persist-or-rollback, like the address refresh.
pub(super) fn remember_peer_hostname(
    shared: &Shared,
    peer_key: &[u8; 32],
    hostname: Option<String>,
) -> Result<(), Error> {
    let Some(name) = hostname else {
        return Ok(());
    };
    let mut state = lock(&shared.state);
    let Some(device) = state.trust.get(peer_key).cloned() else {
        return Ok(());
    };
    if state.trust.hostname_of(peer_key).as_deref() == Some(name.as_str()) {
        return Ok(());
    }
    let previous = state.trust.hostname_of(peer_key);
    state.trust.set_hostname(peer_key, name.clone())?;
    if let Err(error) = persist(shared, &mut state) {
        match previous {
            Some(previous) => {
                if let Err(restore) = state.trust.set_hostname(peer_key, previous) {
                    tracing::warn!(
                        target: crate::trace::RELAY,
                        failure = restore.name(),
                        "the peer's previous name could not be put back after a failed save: memory holds the new one"
                    );
                }
            }
            None => state.trust.clear_hostname(peer_key),
        }
        return Err(error);
    }
    drop(state);
    shared.events.publish(CoreEvent::PeerRenamed {
        endpoint_id: device.display_id,
        hostname: Some(name),
    });
    Ok(())
}

pub(super) async fn write_addresses(
    shared: &Shared,
    send: &mut iroh::endpoint::SendStream,
) -> Result<(), Error> {
    let relays = shared.relay.relay_urls()?;
    let direct = canonicalize_addrs(&direct_addresses(&shared.endpoint))?;
    let announcement = crate::address::canonical(&relays, &direct)?;
    write_frame(
        send,
        &ControlMessage::Addresses {
            relays: announcement.relays,
            direct: announcement.direct,
        },
    )
    .await
}

impl Core {
    /// Local display override. Never sent to the peer and never changes trust.
    /// Pass None to return to the reported hostname.
    pub fn set_peer_pet_name(&self, endpoint_id: &str, name: Option<&str>) -> Result<(), Error> {
        let _engine = self.engine();
        let key = parse_endpoint_id(endpoint_id)?;
        let name = name
            .map(|name| {
                crate::clean_hostname(name).ok_or(Error::InvalidSetting("invalid computer name"))
            })
            .transpose()?;
        let mut state = self.alive_state()?;
        state.trust.get(&key).ok_or(Error::NotPaired)?;
        let previous = state.trust.pet_name_of(&key);
        if previous == name {
            return Ok(());
        }
        match name {
            Some(name) => state.trust.set_pet_name(&key, name)?,
            None => state.trust.clear_pet_name(&key),
        }
        if let Err(error) = persist(&self.shared, &mut state) {
            match previous {
                Some(name) => {
                    if let Err(restore) = state.trust.set_pet_name(&key, name) {
                        tracing::warn!(
                            target: crate::trace::RELAY,
                            failure = restore.name(),
                            "the peer's previous nickname could not be put back after a failed save: memory holds the new one"
                        );
                    }
                }
                None => state.trust.clear_pet_name(&key),
            }
            return Err(error);
        }
        let hostname = state.trust.hostname_of(&key);
        drop(state);
        self.shared.events.publish(CoreEvent::PeerRenamed {
            endpoint_id: endpoint_id.to_string(),
            hostname,
        });
        Ok(())
    }

    /// Display name learned during pairing or refreshed on connect, if any. The UI
    /// falls back to the key fragment when this is `None`. Unknown devices
    /// report `NotPaired`: a name never implies trust.
    pub fn peer_hostname(&self, endpoint_id: &str) -> Result<Option<String>, Error> {
        let _engine = self.engine();
        let key = parse_endpoint_id(endpoint_id)?;
        let state = self.alive_state()?;
        state.trust.get(&key).cloned().ok_or(Error::NotPaired)?;
        Ok(state.trust.hostname_of(&key))
    }

    /// Paths for a controller's local handle, independent of host id collisions.
    pub fn controller_session_paths(&self, id: u64) -> Result<Vec<SessionPath>, Error> {
        let _engine = self.engine();
        let link = self
            .alive_state()?
            .outgoing
            .get(&id)
            .ok_or(Error::NoSession)?
            .link
            .clone();
        Ok(link
            .map(|link| {
                link.paths()
                    .iter()
                    .map(|path| session_path(&path))
                    .collect()
            })
            .unwrap_or_default())
    }

    /// Open transport paths of one live session. Empty while the session has
    /// no connection yet. Each entry names whether the path goes through a
    /// relay, whether the connection currently sends there, and the
    /// round-trip estimate. For diagnostics and the measurement hooks; the
    /// list is a snapshot taken at call time.
    pub fn session_paths(&self, session_id: u64) -> Result<Vec<SessionPath>, Error> {
        let _engine = self.engine();
        let link = {
            let state = self.alive_state()?;
            let session = state
                .incoming
                .get(&session_id)
                .or_else(|| state.outgoing.get(&session_id))
                .ok_or(Error::NoSession)?;
            session.link.clone()
        };
        let Some(link) = link else {
            return Ok(Vec::new());
        };
        Ok(link
            .paths()
            .iter()
            .map(|path| session_path(&path))
            .collect())
    }
}

/// One path as diagnostics see it: its route, round trip, bytes and this
/// side's congestion state.
fn session_path(path: &iroh::endpoint::Path<'_>) -> SessionPath {
    let stats = path.stats();
    SessionPath {
        relay: path.is_relay(),
        selected: path.is_selected(),
        rtt_ms: u64::try_from(path.rtt().as_millis()).unwrap_or(u64::MAX),
        tx_bytes: stats.udp_tx.bytes,
        rx_bytes: stats.udp_rx.bytes,
        cwnd: stats.cwnd,
        congestion_events: stats.congestion_events,
        lost_packets: stats.lost_packets,
        black_holes_detected: stats.black_holes_detected,
    }
}

/// Multiple services and directions may share a peer. A live connection wins
/// over an older finished record; no relay or cached-address guess is involved.
pub(super) fn presence(state: &State, peer: &str) -> crate::DevicePresence {
    // While links are supervised, presence is reachability, not use.
    if state.presence_links {
        return state
            .presence
            .get(peer)
            .copied()
            .unwrap_or(crate::DevicePresence::Disconnected);
    }
    let phases: Vec<_> = state
        .incoming
        .values()
        .chain(state.outgoing.values())
        .filter(|session| session.peer_id == peer)
        .map(|session| &session.phase)
        .collect();
    if phases.contains(&&SessionPhase::Active) {
        crate::DevicePresence::Connected
    } else if phases.contains(&&SessionPhase::Reconnecting) {
        crate::DevicePresence::Reconnecting
    } else if phases.iter().any(|phase| {
        matches!(
            phase,
            SessionPhase::Connecting
                | SessionPhase::Authenticated
                | SessionPhase::Authorized
                | SessionPhase::Negotiating
        )
    }) {
        crate::DevicePresence::Connecting
    } else {
        crate::DevicePresence::Disconnected
    }
}

pub(super) fn remember_peer_os(
    shared: &Shared,
    peer: &[u8; 32],
    os: crate::OperatingSystem,
) -> Result<(), Error> {
    let mut state = lock(&shared.state);
    if state.trust.get(peer).is_none() || state.trust.os_of(peer) == Some(os) {
        return Ok(());
    }
    let previous = state.trust.clone();
    state.trust.set_os(peer, os)?;
    if let Err(error) = persist(shared, &mut state) {
        state.trust = previous;
        return Err(error);
    }
    Ok(())
}

#[cfg(test)]
mod metadata_tests {
    use super::*;

    #[test]
    fn failed_first_hostname_save_preserves_signed_os_metadata() {
        let store = crate::installation::MemoryStore::empty();
        let host = Core::open_direct(Arc::new(store.clone()), RelaySettings::public_n0()).unwrap();
        let peer = crate::test_support::loopback_core();
        host.accept_invitation(&peer.create_invitation(Duration::from_secs(60)).unwrap())
            .unwrap();
        let key = parse_endpoint_id(&peer.endpoint_id()).unwrap();
        {
            let mut state = host.shared.state.lock().unwrap();
            state.trust.clear_hostname(&key);
            state
                .trust
                .set_os(&key, crate::OperatingSystem::Windows)
                .unwrap();
        }
        store.fail_next_save(Error::StoreUnavailable);
        assert!(matches!(
            remember_peer_hostname(&host.shared, &key, Some("new name".into())),
            Err(Error::StoreUnavailable)
        ));
        let device = &host.snapshot().unwrap().devices[0];
        assert_eq!(device.hostname, None);
        assert_eq!(device.os, Some(crate::OperatingSystem::Windows));
    }
}

#[cfg(test)]
mod address_refresh_tests {
    use std::net::SocketAddr;
    use std::sync::Arc;
    use std::time::{Duration, SystemTime};

    use iroh::SecretKey;

    use crate::core::Core;
    use crate::error::Error;
    use crate::installation::MemoryStore;
    use crate::invitation;
    use crate::relay::RelaySettings;
    use crate::session::{Grants, SessionPhase};
    use crate::test_support::loopback_core;

    fn open_stored(store: &MemoryStore) -> Core {
        let core = Core::open_direct(Arc::new(store.clone()), RelaySettings::public_n0())
            .expect("store opens");
        core.set_video_codecs(crate::VideoCodecCaps::h264_full(4096, 4096, "test"))
            .expect("flags");
        core
    }

    fn stale() -> SocketAddr {
        "203.0.113.9:9".parse().unwrap()
    }

    fn invitation_with(core: &Core, extra: Option<SocketAddr>) -> (String, Vec<SocketAddr>) {
        let token = core.create_invitation(Duration::from_secs(60)).unwrap();
        let (opened, _) = invitation::open(&token, SystemTime::now()).unwrap();
        let real = opened.direct.clone();
        let mut direct = real.clone();
        if let Some(extra) = extra {
            direct.push(extra);
        }
        let secret = SecretKey::from_bytes(&core.export_identity());
        let (token, _) = invitation::issue(
            &secret,
            &opened.relays,
            &direct,
            Duration::from_secs(60),
            SystemTime::now(),
            opened.hostname,
        )
        .unwrap();
        (token, real)
    }

    fn pair(host: &Core, controller: &Core, extra: Option<SocketAddr>) -> Vec<SocketAddr> {
        host.start_hosting().unwrap();
        let (host_token, host_direct) = invitation_with(host, extra);
        controller.accept_invitation(&host_token).unwrap();
        let (controller_token, _) = invitation_with(controller, extra);
        host.accept_invitation(&controller_token).unwrap();
        host.set_desktop_sharing(Grants {
            view: true,
            control: true,
            clipboard: false,
            transfer: false,
        })
        .unwrap();
        host_direct
    }

    fn device(core: &Core, endpoint_id: &str) -> crate::DeviceInfo {
        core.snapshot()
            .unwrap()
            .devices
            .into_iter()
            .find(|device| device.endpoint_id == endpoint_id)
            .expect("paired device")
    }

    fn wait_until(mut ready: impl FnMut() -> bool) {
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            if ready() {
                return;
            }
            if std::time::Instant::now() > deadline {
                panic!("condition was not met");
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    #[test]
    fn a_session_replaces_stale_addresses_and_keeps_the_grant() {
        let host = loopback_core();
        let controller = loopback_core();
        let host_direct = pair(&host, &controller, Some(stale()));
        assert!(device(&controller, &host.endpoint_id())
            .direct
            .contains(&stale()));
        controller.connect(&host.endpoint_id()).unwrap();
        wait_until(|| {
            let saved = device(&controller, &host.endpoint_id());
            !saved.direct.contains(&stale()) && saved.direct == host_direct
        });
        wait_until(|| {
            !device(&host, &controller.endpoint_id())
                .direct
                .contains(&stale())
        });
        // The controller never granted the host. The host's control bit must
        // survive the address replacement, and the announcement must not add one.
        let saved = device(&controller, &host.endpoint_id());
        assert_eq!(saved.grants, Grants::default());
        assert!(saved.relays.is_empty());
        let granted = device(&host, &controller.endpoint_id());
        assert!(granted.grants.control);
        assert!(!granted.grants.clipboard);
        assert!(!granted.grants.transfer);
    }

    #[test]
    fn a_failed_save_keeps_the_previous_addresses() {
        let host_store = MemoryStore::empty();
        let controller_store = MemoryStore::empty();
        let host = open_stored(&host_store);
        let controller = open_stored(&controller_store);
        pair(&host, &controller, Some(stale()));
        controller_store.fail_next_save(Error::StoreLocked);
        controller.connect(&host.endpoint_id()).unwrap();
        wait_until(|| {
            !device(&host, &controller.endpoint_id())
                .direct
                .contains(&stale())
        });
        std::thread::sleep(Duration::from_millis(300));
        assert!(device(&controller, &host.endpoint_id())
            .direct
            .contains(&stale()));
        assert_eq!(
            controller.snapshot().unwrap().outgoing.unwrap().phase,
            SessionPhase::Active
        );
    }

    #[test]
    fn refreshed_addresses_survive_a_restart() {
        let host_store = MemoryStore::empty();
        let controller_store = MemoryStore::empty();
        let host = open_stored(&host_store);
        let controller = open_stored(&controller_store);
        let host_direct = pair(&host, &controller, Some(stale()));
        controller.connect(&host.endpoint_id()).unwrap();
        wait_until(|| {
            let saved = device(&controller, &host.endpoint_id());
            !saved.direct.contains(&stale()) && saved.direct == host_direct
        });
        wait_until(|| {
            !device(&host, &controller.endpoint_id())
                .direct
                .contains(&stale())
        });
        let saved = device(&controller, &host.endpoint_id());
        let granted = device(&host, &controller.endpoint_id());
        assert!(granted.grants.control);
        drop(controller);
        drop(host);
        let controller = open_stored(&controller_store);
        let restored = device(&controller, &saved.endpoint_id);
        assert_eq!(restored.direct, saved.direct);
        assert_eq!(restored.relays, saved.relays);
        assert_eq!(restored.grants, Grants::default());
        let host = open_stored(&host_store);
        let restored_grant = device(&host, &granted.endpoint_id);
        assert_eq!(restored_grant.direct, granted.direct);
        assert_eq!(restored_grant.relays, granted.relays);
        assert!(restored_grant.grants.control);
        assert!(!restored_grant.grants.clipboard);
        assert!(!restored_grant.grants.transfer);
    }
}
